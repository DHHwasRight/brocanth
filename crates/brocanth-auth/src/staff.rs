//! Staff: the people who operate the application, and what each one may do.
//!
//! Two layers, as in Django's `contrib.auth`:
//!
//! * **A superuser may do everything.** It is a flag on an account, set by
//!   an operator with a command or at startup, never by a form a visitor can
//!   reach. It exists so a new backoffice is usable before anybody has
//!   written a policy.
//! * **A role grants permissions.** A permission is a string the application
//!   chooses. An account holds the union of its roles' permissions.
//!
//! [`Staff`] is the two read together for one signed-in account. The
//! backoffice receives it as its principal, and a page asks
//! [`Staff::allows`]. Rows are a different question: `brocanth-policy`
//! answers which rows, and [`Staff::gate`] (feature `policy`) turns a
//! permission into a predicate that policy can combine with a row rule.
//!
//! # Permission names
//!
//! The backoffice asks for these, and an application may use any others:
//!
//! | Screen | Permission |
//! |---|---|
//! | a model's list and detail | `<model>.view` |
//! | create | `<model>.add` |
//! | change | `<model>.change` |
//! | delete | `<model>.delete` |
//! | a panel | `<panel>.view` |
//! | a panel's buttons | `<panel>.act` |
//!
//! The words are Django's, so a team that knows `post.change_post` reads
//! them without a table.
//!
//! # An anonymous session is not staff
//!
//! [`Staff::from_cookie_header`] answers `None` for a pre-login session,
//! whatever account it names: [`Privilege::Anonymous`] carries no proof.
//!
//! # An impersonated session is not staff either
//!
//! An impersonated session acts for its subject in the application, but it
//! gets no staff rights, the subject's or the operator's. Otherwise an
//! operator with one narrow role who impersonates a superuser becomes one.
//! [`Staff::impersonate`] also refuses a staff subject and asks the
//! operator for the `accounts.impersonate` permission.

use std::collections::BTreeSet;

use crate::accounts::Accounts;
use crate::clock::Clock;
use crate::error::AuthError;
use crate::http_client::HttpClient;
use crate::identity::{Account, AccountId, EmailAddress};
use crate::plugin::Auth;
use crate::session::{Privilege, StartedSession};
use crate::storage::{SessionRecord, StaffStore};

/// One signed-in member of staff: the account, whether it is a superuser,
/// and what its roles grant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Staff {
    account: Account,
    superuser: bool,
    roles: Vec<String>,
    permissions: BTreeSet<String>,
}

impl Staff {
    /// Reads one account's staff record. `None` when no account has that id.
    ///
    /// Answers a value for every account, staff or not: an account with no
    /// role and no flag is a [`Staff`] whose [`Staff::is_staff`] is false.
    ///
    /// # Errors
    /// Fails when the store fails.
    pub async fn load<S: StaffStore>(
        store: &S,
        account: &AccountId,
    ) -> Result<Option<Self>, AuthError> {
        let Some(account) = store.account(account).await? else {
            return Ok(None);
        };
        let superuser = store.is_superuser(&account.id).await?;
        let roles = store.roles_of(&account.id).await?;
        let permissions = store.permissions_of(&account.id).await?;
        Ok(Some(Self {
            account,
            superuser,
            roles,
            permissions,
        }))
    }

    /// The staff record of the session a request's `Cookie` header carries.
    ///
    /// `None` for no session, a dead one, an anonymous one, or an account
    /// that is neither a superuser nor holds a role. So `Some` is exactly
    /// "a member of staff is signed in", which is what a backoffice asks.
    ///
    /// # Errors
    /// Fails when the store fails. A missing or forged cookie is not an
    /// error: it is `None`.
    pub async fn from_cookie_header<S, H, C>(
        auth: &Auth<S, H, C>,
        header: &str,
    ) -> Result<Option<Self>, AuthError>
    where
        S: StaffStore,
        H: HttpClient,
        C: Clock,
    {
        let session = match auth.sessions().from_cookie_header(header).await {
            Ok(session) => session,
            Err(AuthError::NoSession) => return Ok(None),
            Err(other) => return Err(other),
        };
        if session.privilege == Privilege::Anonymous || session.is_impersonated() {
            return Ok(None);
        }
        Ok(Self::load(&**auth.store(), &session.account)
            .await?
            .filter(Self::is_staff))
    }

    /// The account.
    pub fn account(&self) -> &Account {
        &self.account
    }

    /// Opens a session in which this member of staff acts as `subject`.
    ///
    /// [`Sessions::impersonate`] with the two checks it cannot make, because
    /// it does not read staff records: the operator holds
    /// [`IMPERSONATE`](Self::IMPERSONATE) (a superuser holds every
    /// permission), and the subject is not staff. Impersonating a member of
    /// staff would hand the operator rights that nobody gave them.
    ///
    /// [`Sessions::impersonate`]: crate::session::Sessions::impersonate
    ///
    /// # Errors
    /// Returns [`AuthError::Misconfigured`] when the operator session is not
    /// this member of staff's, when the permission is missing, or when the
    /// subject is staff or does not exist, and whatever
    /// [`Sessions::impersonate`] returns.
    pub async fn impersonate<S, H, C>(
        &self,
        auth: &Auth<S, H, C>,
        operator_session: &SessionRecord,
        subject: &AccountId,
        reason: impl Into<String>,
    ) -> Result<StartedSession, AuthError>
    where
        S: StaffStore,
        H: HttpClient,
        C: Clock,
    {
        if operator_session.account != self.account.id {
            return Err(AuthError::misconfigured(
                "that operator session belongs to another account\n\
                 help: pass the session this Staff record was read from",
            ));
        }
        if !self.allows(Self::IMPERSONATE) {
            return Err(AuthError::misconfigured(format!(
                "impersonation needs the `{}` permission\n\
                 help: give the operator a role that holds it",
                Self::IMPERSONATE
            )));
        }
        let target = Self::load(&**auth.store(), subject).await?.ok_or_else(|| {
            AuthError::misconfigured(
                "no account has that id\nhelp: pick the subject from the accounts list",
            )
        })?;
        if target.is_staff() {
            return Err(AuthError::misconfigured(
                "a member of staff cannot be impersonated\n\
                 help: ask them to act, or change their roles first",
            ));
        }
        auth.sessions()
            .impersonate(operator_session, subject, reason)
            .await
    }

    /// The permission [`Staff::impersonate`] asks for.
    pub const IMPERSONATE: &'static str = "accounts.impersonate";

    /// The account's address, or its id when it has none. For a screen.
    pub fn name(&self) -> String {
        self.account
            .email
            .as_ref()
            .map_or_else(|| self.account.id.to_string(), ToString::to_string)
    }

    /// Whether the account is a superuser.
    pub fn is_superuser(&self) -> bool {
        self.superuser
    }

    /// The names of the account's roles, sorted.
    pub fn roles(&self) -> &[String] {
        &self.roles
    }

    /// Every permission the account's roles grant. A superuser's set is
    /// whatever its roles grant, and [`Staff::allows`] ignores it.
    pub fn permissions(&self) -> &BTreeSet<String> {
        &self.permissions
    }

    /// Whether the account is staff at all: a superuser, or the holder of a
    /// role.
    pub fn is_staff(&self) -> bool {
        self.superuser || !self.roles.is_empty()
    }

    /// Whether the account may do what `permission` names. Always for a
    /// superuser.
    pub fn allows(&self, permission: &str) -> bool {
        self.superuser || self.permissions.contains(permission)
    }

    /// A permission as a row predicate: every row when the account holds it,
    /// none when it does not.
    ///
    /// For a `brocanth_policy::Policy` that combines a staff permission with
    /// a row rule, so both travel in one `condition()`:
    ///
    /// ```ignore
    /// fn update(&self, who: &Visitor) -> EntityPred<Post> {
    ///     // Editors change every post; everybody else changes their own.
    ///     who.staff.gate("post.change").or(Pred::eq(PostColumn::AuthorId, who.id))
    /// }
    /// ```
    #[cfg(feature = "policy")]
    pub fn gate<C: brocanth_policy::Column>(&self, permission: &str) -> brocanth_policy::Pred<C> {
        if self.allows(permission) {
            brocanth_policy::Pred::Always
        } else {
            brocanth_policy::Pred::Never
        }
    }
}

/// What [`ensure_superuser`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SuperuserOutcome {
    /// No account had the address: one was created with the password, and
    /// made a superuser.
    Created(AccountId),
    /// An account had the address, proven, and was not a superuser. Now it
    /// is. Its password did not change.
    Promoted(AccountId),
    /// The account was a superuser already. Nothing changed.
    Unchanged(AccountId),
}

impl SuperuserOutcome {
    /// The account, whatever happened to it.
    pub fn account(&self) -> &AccountId {
        match self {
            Self::Created(id) | Self::Promoted(id) | Self::Unchanged(id) => id,
        }
    }
}

/// Makes the account with this address a superuser, and creates it first
/// when there is none.
///
/// The password is used only to create the account. An existing account
/// keeps its own: a command run twice must not reset a password somebody
/// already changed. Idempotent, so a server can call it at every start.
///
/// An existing account is promoted only when its address is verified. A
/// stranger who signs up with the superuser address before the first start
/// owns an unverified account and its password; promoting it would make
/// them superuser.
///
/// # Errors
/// Returns [`AuthError::InvalidEmail`] for an address that is not one,
/// [`AuthError::Misconfigured`] when an account with an unverified address
/// holds it, and fails when the store or the hasher fails.
pub async fn ensure_superuser<S: StaffStore, C: Clock>(
    accounts: &Accounts<S, C>,
    store: &S,
    email: &str,
    password: &str,
) -> Result<SuperuserOutcome, AuthError> {
    let address = EmailAddress::parse(email)?;
    let outcome = match store.account_by_email(&address).await? {
        Some(account) if store.is_superuser(&account.id).await? => {
            return Ok(SuperuserOutcome::Unchanged(account.id))
        }
        Some(account) => promoted(account)?,
        // Verified from the start: the operator's configuration names the
        // address, and a racing replica must see it proven.
        None => match accounts.create_vouched(&address, password).await? {
            Some(account) => SuperuserOutcome::Created(account.id),
            // Registered between the read above and the insert: promote the
            // account that won, when its address is proven.
            None => {
                let account = store
                    .account_by_email(&address)
                    .await?
                    .ok_or_else(|| AuthError::Store("an address is taken by no account".into()))?;
                promoted(account)?
            }
        },
    };
    store.set_superuser(outcome.account(), true).await?;
    Ok(outcome)
}

/// An existing account becomes superuser only with a proven address.
fn promoted(account: Account) -> Result<SuperuserOutcome, AuthError> {
    if !account.email_verified {
        let email = account
            .email
            .as_ref()
            .map_or_else(String::new, ToString::to_string);
        return Err(AuthError::misconfigured(format!(
            "the account with the superuser address {email} has not proven that address, \
             so it is not made superuser\n\
             help: verify the address of that account, or delete it and start again"
        )));
    }
    Ok(SuperuserOutcome::Promoted(account.id))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::clock::TestClock;
    use crate::memory::MemoryStore;
    use crate::password::{PasswordCost, Passwords};
    use crate::storage::{AuthStore, Role};

    fn accounts(store: &Arc<MemoryStore>) -> Accounts<MemoryStore, TestClock> {
        let passwords = Passwords::new(PasswordCost::insecure_for_tests()).unwrap();
        Accounts::new(
            store.clone(),
            Arc::new(TestClock::at(1_700_000_000)),
            Arc::new(passwords),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn a_superuser_is_created_then_left_alone() {
        let store = Arc::new(MemoryStore::new());
        let accounts = accounts(&store);
        let first = ensure_superuser(&accounts, &store, "ops@example.com", "a long passphrase")
            .await
            .unwrap();
        let SuperuserOutcome::Created(id) = first.clone() else {
            panic!("expected a new account, got {first:?}");
        };
        assert!(store.is_superuser(&id).await.unwrap());
        let again = ensure_superuser(&accounts, &store, "ops@example.com", "another one")
            .await
            .unwrap();
        assert_eq!(again, SuperuserOutcome::Unchanged(id));
        assert!(
            accounts
                .login("ops@example.com", "a long passphrase")
                .await
                .is_ok(),
            "the second call kept the first password"
        );
    }

    #[tokio::test]
    async fn an_existing_account_is_promoted() {
        let store = Arc::new(MemoryStore::new());
        let accounts = accounts(&store);
        accounts
            .register("ada@example.com", "a long passphrase")
            .await
            .unwrap();
        let ada = store
            .account_by_email(&EmailAddress::parse("ada@example.com").unwrap())
            .await
            .unwrap()
            .unwrap();

        // Not while the address is unproven: whoever signed up first owns it.
        let refused = ensure_superuser(&accounts, &store, "ada@example.com", "ignored")
            .await
            .expect_err("an unverified address is not promoted");
        assert!(refused.to_string().contains("help:"), "{refused}");
        assert!(!store.is_superuser(&ada.id).await.unwrap());

        store.set_email_verified(&ada.id, true).await.unwrap();
        let outcome = ensure_superuser(&accounts, &store, "ada@example.com", "ignored")
            .await
            .unwrap();
        assert!(matches!(outcome, SuperuserOutcome::Promoted(_)));
        let staff = Staff::load(&*store, outcome.account())
            .await
            .unwrap()
            .unwrap();
        assert!(staff.is_superuser() && staff.allows("anything.at.all"));
    }

    #[tokio::test]
    async fn a_role_holder_is_staff_and_holds_only_what_it_was_given() {
        let store = Arc::new(MemoryStore::new());
        let accounts = accounts(&store);
        accounts
            .register("ed@example.com", "a long passphrase")
            .await
            .unwrap();
        let ed = store
            .account_by_email(&EmailAddress::parse("ed@example.com").unwrap())
            .await
            .unwrap()
            .unwrap()
            .id;
        let plain = Staff::load(&*store, &ed).await.unwrap().unwrap();
        assert!(!plain.is_staff(), "an account with no role is not staff");

        store
            .put_role(&Role::new("editor", ["post.change"]))
            .await
            .unwrap();
        store.assign_role(&ed, "editor").await.unwrap();
        let editor = Staff::load(&*store, &ed).await.unwrap().unwrap();
        assert!(editor.is_staff());
        assert!(editor.allows("post.change"));
        assert!(!editor.allows("post.delete"));
        assert_eq!(editor.name(), "ed@example.com");
    }

    #[tokio::test]
    async fn a_bad_address_is_refused() {
        let store = Arc::new(MemoryStore::new());
        let accounts = accounts(&store);
        let error = ensure_superuser(&accounts, &store, "not an address", "x")
            .await
            .expect_err("refused");
        assert!(matches!(error, AuthError::InvalidEmail(_)), "{error:?}");
    }
}
