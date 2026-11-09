//! Account lockout: after too many wrong passwords, an account refuses
//! password sign-in for a while.
//!
//! [`Lockout::login`] wraps [`Accounts::login`]. It counts each wrong
//! password against the account, and at [`LockoutPolicy::max_failures`]
//! within [`LockoutPolicy::window`] it locks the account for
//! [`LockoutPolicy::lock_for`].
//!
//! # Decisions
//!
//! * **A locked account answers like a wrong password.** Even the right
//!   password gets [`Attempt::Refused`]. A page that said "locked" would
//!   tell a stranger that the address has an account. The owner learns it
//!   from the mail that [`Attempt::JustLocked`] asks the caller to send.
//! * **The lock ends by itself.** A lock that only an administrator lifts
//!   lets anybody who knows an address keep its owner out for ever. A lock
//!   for minutes makes a guessing run slow and costs the owner minutes.
//! * **A password reset lifts the lock.** It proves the mailbox, and the
//!   mail of a lock sends the owner there. Call [`Lockout::clear`] after
//!   [`Accounts::reset_password`].
//! * **An unknown address does the same store work.** Its failures count
//!   against a sentinel id, so the time of an answer does not tell a known
//!   address from an unknown one. The sentinel can lock; nobody signs in
//!   as it.
//! * **The count is per account, not per address.** A guessing run from
//!   many addresses still locks the account. Limit each address with the
//!   rate limiter (`middleware-rate-limit`) beside this.

use std::sync::Arc;

use crate::accounts::{Accounts, Authenticated};
use crate::clock::{Clock, UnixTime};
use crate::error::AuthError;
use crate::identity::{Account, AccountId, EmailAddress};
use crate::storage::LockoutStore;

/// How many failures lock an account, in what time, and for how long.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LockoutPolicy {
    max_failures: u32,
    window: i64,
    lock_for: i64,
}

impl LockoutPolicy {
    /// Ten failures in fifteen minutes lock the account for fifteen
    /// minutes: a person who forgot a password rarely tries ten, and a
    /// guessing run gets forty tries an hour.
    pub const fn recommended() -> Self {
        Self {
            max_failures: 10,
            window: 15 * 60,
            lock_for: 15 * 60,
        }
    }

    /// A policy of your own. Times are in seconds.
    ///
    /// # Errors
    /// [`AuthError::Misconfigured`] when `max_failures` is below 3, or a
    /// time is below 60 seconds or above one day.
    pub fn new(max_failures: u32, window: i64, lock_for: i64) -> Result<Self, AuthError> {
        if max_failures < 3 {
            return Err(AuthError::misconfigured(format!(
                "a lockout needs 3 failures or more, not {max_failures}: fewer locks out \
                 people who mistype"
            )));
        }
        for (name, value) in [("window", window), ("lock_for", lock_for)] {
            if !(60..=86_400).contains(&value) {
                return Err(AuthError::misconfigured(format!(
                    "the lockout {name} is between 60 and 86400 seconds, not {value}"
                )));
            }
        }
        Ok(Self {
            max_failures,
            window,
            lock_for,
        })
    }

    /// The failures that lock an account.
    pub fn max_failures(&self) -> u32 {
        self.max_failures
    }

    /// The seconds in which those failures count.
    pub fn window(&self) -> i64 {
        self.window
    }

    /// The seconds a lock lasts.
    pub fn lock_for(&self) -> i64 {
        self.lock_for
    }
}

impl Default for LockoutPolicy {
    fn default() -> Self {
        Self::recommended()
    }
}

/// What a sign-in through [`Lockout::login`] came to.
///
/// **Render [`Attempt::Refused`] and [`Attempt::JustLocked`] alike.** The
/// second one only adds a mail to the owner.
#[derive(Debug)]
pub enum Attempt {
    /// The password matched and the account is not locked.
    Admitted(Authenticated),
    /// A wrong password, an unknown address, or a locked account.
    Refused,
    /// A wrong password, and this failure locked the account. Mail its
    /// owner: the account is locked until `until`, and a password reset
    /// lifts the lock.
    JustLocked {
        /// The account that is now locked.
        account: Account,
        /// When the lock ends.
        until: UnixTime,
    },
}

/// Password sign-in that counts failures. Build it with
/// [`Auth::lockout`](crate::plugin::Auth::lockout).
pub struct Lockout<S: LockoutStore, C: Clock> {
    store: Arc<S>,
    clock: Arc<C>,
    policy: LockoutPolicy,
    sentinel: AccountId,
}

impl<S: LockoutStore, C: Clock> Lockout<S, C> {
    /// A lockout over `store`, with `policy`.
    pub fn new(store: Arc<S>, clock: Arc<C>, policy: LockoutPolicy) -> Self {
        Self {
            store,
            clock,
            policy,
            // Fixed, not random: every process counts the unknown addresses
            // in one row, so the table does not grow with them.
            sentinel: AccountId::new("brocanth-lockout-sentinel"),
        }
    }

    /// The policy.
    pub fn policy(&self) -> &LockoutPolicy {
        &self.policy
    }

    /// [`Accounts::login`], with the failures counted.
    ///
    /// The password is always checked, so a locked account costs the same
    /// time as an open one.
    ///
    /// # Errors
    /// Fails only on a store or hashing failure. A wrong password is
    /// [`Attempt::Refused`], not an error.
    pub async fn login(
        &self,
        accounts: &Accounts<S, C>,
        email: &str,
        password: &str,
    ) -> Result<Attempt, AuthError> {
        let account = match EmailAddress::parse(email) {
            Ok(parsed) => self.store.account_by_email(&parsed).await?,
            Err(_) => None,
        };
        let checked = accounts.login(email, password).await;
        let now = self.clock.now();
        let id = account.as_ref().map_or(&self.sentinel, |a| &a.id);
        let locked = self.store.locked_until(id).await?.is_some_and(|t| t > now);
        match checked {
            Ok(_) if locked => Ok(Attempt::Refused),
            Ok(signed) => {
                self.store.clear_failed_logins(&signed.account().id).await?;
                Ok(Attempt::Admitted(signed))
            }
            Err(AuthError::InvalidCredentials) if locked => Ok(Attempt::Refused),
            Err(AuthError::InvalidCredentials) => {
                let failures = self
                    .store
                    .record_failed_login(id, now, now - self.policy.window)
                    .await?;
                if failures < self.policy.max_failures {
                    return Ok(Attempt::Refused);
                }
                let until = now + self.policy.lock_for;
                self.store.lock_account(id, until).await?;
                Ok(match account {
                    Some(account) => Attempt::JustLocked { account, until },
                    None => Attempt::Refused,
                })
            }
            Err(other) => Err(other),
        }
    }

    /// Forgets the failures and lifts the lock: after a password reset, or
    /// from an administrator.
    ///
    /// # Errors
    /// Fails on a store failure.
    pub async fn clear(&self, account: &AccountId) -> Result<(), AuthError> {
        Ok(self.store.clear_failed_logins(account).await?)
    }

    /// When the lock of `account` ends, or `None` when it is not locked now.
    ///
    /// # Errors
    /// Fails on a store failure.
    pub async fn locked_until(&self, account: &AccountId) -> Result<Option<UnixTime>, AuthError> {
        let now = self.clock.now();
        Ok(self
            .store
            .locked_until(account)
            .await?
            .filter(|until| *until > now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::TestClock;
    use crate::memory::MemoryStore;
    use crate::password::{PasswordCost, Passwords};

    struct Fixture {
        accounts: Accounts<MemoryStore, TestClock>,
        lockout: Lockout<MemoryStore, TestClock>,
        clock: Arc<TestClock>,
    }

    async fn fixture(policy: LockoutPolicy) -> Fixture {
        let store = Arc::new(MemoryStore::new());
        let clock = Arc::new(TestClock::default());
        let accounts = Accounts::new(
            store.clone(),
            clock.clone(),
            Arc::new(Passwords::new(PasswordCost::insecure_for_tests()).expect("hasher")),
        )
        .expect("accounts");
        accounts
            .register("ada@example.com", "the right password")
            .await
            .expect("register");
        Fixture {
            lockout: Lockout::new(store, clock.clone(), policy),
            accounts,
            clock,
        }
    }

    async fn wrong(f: &Fixture, email: &str) -> Attempt {
        f.lockout
            .login(&f.accounts, email, "a wrong password")
            .await
            .expect("no store failure")
    }

    async fn right(f: &Fixture) -> Attempt {
        f.lockout
            .login(&f.accounts, "ada@example.com", "the right password")
            .await
            .expect("no store failure")
    }

    fn policy() -> LockoutPolicy {
        LockoutPolicy::new(3, 600, 300).expect("a valid policy")
    }

    #[tokio::test]
    async fn the_third_failure_locks_and_then_the_right_password_is_refused() {
        let f = fixture(policy()).await;
        assert!(matches!(
            wrong(&f, "ada@example.com").await,
            Attempt::Refused
        ));
        assert!(matches!(
            wrong(&f, "ada@example.com").await,
            Attempt::Refused
        ));
        let Attempt::JustLocked { account, until } = wrong(&f, "ada@example.com").await else {
            panic!("the third failure locks");
        };
        assert_eq!(until, f.clock.now() + 300);
        assert!(matches!(right(&f).await, Attempt::Refused), "locked");
        // Failures during the lock do not report a new lock.
        assert!(matches!(
            wrong(&f, "ada@example.com").await,
            Attempt::Refused
        ));

        f.clock.advance(301);
        assert!(
            matches!(right(&f).await, Attempt::Admitted(_)),
            "the lock ends"
        );
        assert_eq!(f.lockout.locked_until(&account.id).await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_success_and_an_old_window_both_start_the_count_again() {
        let f = fixture(policy()).await;
        wrong(&f, "ada@example.com").await;
        wrong(&f, "ada@example.com").await;
        assert!(matches!(right(&f).await, Attempt::Admitted(_)));
        wrong(&f, "ada@example.com").await;
        wrong(&f, "ada@example.com").await;
        f.clock.advance(601);
        assert!(
            matches!(wrong(&f, "ada@example.com").await, Attempt::Refused),
            "the first two failures are out of the window"
        );
    }

    #[tokio::test]
    async fn clear_lifts_a_lock() {
        let f = fixture(policy()).await;
        for _ in 0..2 {
            wrong(&f, "ada@example.com").await;
        }
        let Attempt::JustLocked { account, .. } = wrong(&f, "ada@example.com").await else {
            panic!("locked");
        };
        f.lockout.clear(&account.id).await.unwrap();
        assert!(matches!(right(&f).await, Attempt::Admitted(_)));
    }

    #[tokio::test]
    async fn an_unknown_address_never_reports_a_lock() {
        let f = fixture(policy()).await;
        for _ in 0..5 {
            assert!(matches!(
                wrong(&f, "nobody@example.com").await,
                Attempt::Refused
            ));
        }
        assert!(
            matches!(right(&f).await, Attempt::Admitted(_)),
            "ada is not locked"
        );
    }

    #[tokio::test]
    async fn a_failure_on_the_last_second_of_the_window_still_counts() {
        let f = fixture(policy()).await;
        wrong(&f, "ada@example.com").await;
        f.clock.advance(300);
        wrong(&f, "ada@example.com").await;
        // The window began 600 seconds ago: this second is still inside it.
        f.clock.advance(300);
        assert!(
            matches!(
                wrong(&f, "ada@example.com").await,
                Attempt::JustLocked { .. }
            ),
            "three failures in 600 seconds lock"
        );

        let g = fixture(policy()).await;
        wrong(&g, "ada@example.com").await;
        g.clock.advance(300);
        wrong(&g, "ada@example.com").await;
        g.clock.advance(301);
        assert!(
            matches!(wrong(&g, "ada@example.com").await, Attempt::Refused),
            "one second later the first failure is out of the window"
        );
    }

    #[tokio::test]
    async fn the_lock_ends_on_its_last_second_and_not_before() {
        let f = fixture(policy()).await;
        for _ in 0..2 {
            wrong(&f, "ada@example.com").await;
        }
        let Attempt::JustLocked { account, until } = wrong(&f, "ada@example.com").await else {
            panic!("locked");
        };
        f.clock.advance(299);
        assert!(
            matches!(right(&f).await, Attempt::Refused),
            "one second left"
        );
        assert_eq!(
            f.lockout.locked_until(&account.id).await.unwrap(),
            Some(until),
            "the right password during a lock neither lifts nor stretches it"
        );
        f.clock.advance(1);
        assert_eq!(f.clock.now(), until);
        assert!(matches!(right(&f).await, Attempt::Admitted(_)));
    }

    #[tokio::test]
    async fn after_a_lock_ends_the_count_starts_from_zero() {
        let f = fixture(policy()).await;
        for _ in 0..2 {
            wrong(&f, "ada@example.com").await;
        }
        assert!(matches!(
            wrong(&f, "ada@example.com").await,
            Attempt::JustLocked { .. }
        ));
        // The lock (300 s) ends inside the window (600 s).
        f.clock.advance(300);
        assert!(matches!(
            wrong(&f, "ada@example.com").await,
            Attempt::Refused
        ));
        assert!(matches!(
            wrong(&f, "ada@example.com").await,
            Attempt::Refused
        ));
        assert!(
            matches!(
                wrong(&f, "ada@example.com").await,
                Attempt::JustLocked { .. }
            ),
            "three new failures lock again, and the owner gets a new mail"
        );
    }

    #[tokio::test]
    async fn another_spelling_of_the_address_counts_against_the_same_account() {
        let f = fixture(policy()).await;
        wrong(&f, "ADA@Example.com").await;
        wrong(&f, "  ada@EXAMPLE.COM ").await;
        assert!(
            matches!(
                wrong(&f, "Ada@example.com").await,
                Attempt::JustLocked { .. }
            ),
            "case and spaces must not give a guessing run a fresh count"
        );
    }

    #[tokio::test]
    async fn an_unknown_or_malformed_address_counts_against_the_sentinel() {
        let f = fixture(policy()).await;
        let sentinel = f.lockout.sentinel.clone();
        assert!(matches!(
            wrong(&f, "nobody@example.com").await,
            Attempt::Refused
        ));
        assert!(
            matches!(wrong(&f, "not an address").await, Attempt::Refused),
            "a malformed address is a refusal, not an error a page shows apart"
        );
        assert_eq!(f.lockout.locked_until(&sentinel).await.unwrap(), None);
        assert!(matches!(wrong(&f, "").await, Attempt::Refused));
        assert!(
            f.lockout.locked_until(&sentinel).await.unwrap().is_some(),
            "the unknown addresses did the store work a known one does"
        );
        assert!(
            matches!(
                f.lockout
                    .login(&f.accounts, "nobody@example.com", "anything")
                    .await
                    .unwrap(),
                Attempt::Refused
            ),
            "the sentinel never admits"
        );
    }

    #[tokio::test]
    async fn one_locked_account_leaves_another_open() {
        let f = fixture(policy()).await;
        f.accounts
            .register("eve@example.com", "eve's own password")
            .await
            .expect("register");
        for _ in 0..3 {
            wrong(&f, "ada@example.com").await;
        }
        assert!(matches!(right(&f).await, Attempt::Refused));
        assert!(matches!(
            f.lockout
                .login(&f.accounts, "eve@example.com", "eve's own password")
                .await
                .unwrap(),
            Attempt::Admitted(_)
        ));
    }

    #[test]
    fn a_policy_takes_each_bound_and_refuses_one_past_it() {
        assert!(LockoutPolicy::new(3, 60, 60).is_ok());
        assert!(LockoutPolicy::new(3, 86_400, 86_400).is_ok());
        assert!(LockoutPolicy::new(3, 59, 60).is_err());
        assert!(LockoutPolicy::new(3, 60, 59).is_err());
        assert!(LockoutPolicy::new(3, 86_401, 60).is_err());
        assert!(LockoutPolicy::new(3, 60, 86_401).is_err());
        let p = LockoutPolicy::new(4, 120, 180).unwrap();
        assert_eq!((p.max_failures(), p.window(), p.lock_for()), (4, 120, 180));
        let r = LockoutPolicy::recommended();
        assert_eq!((r.max_failures(), r.window(), r.lock_for()), (10, 900, 900));
    }

    #[test]
    fn a_policy_that_locks_out_typos_or_lasts_for_days_is_refused() {
        assert!(LockoutPolicy::new(2, 600, 600).is_err());
        assert!(LockoutPolicy::new(5, 10, 600).is_err());
        assert!(LockoutPolicy::new(5, 600, 2 * 86_400).is_err());
        assert_eq!(LockoutPolicy::default(), LockoutPolicy::recommended());
    }
}
