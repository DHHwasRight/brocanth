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
//! whatever account it names: [`Privilege::Anonymous`] carries no proof. An
//! impersonated session carries the impersonated account's powers and not
//! the operator's, because that is what impersonation means.

use std::collections::BTreeSet;

use crate::accounts::{Accounts, RegistrationNotice};
use crate::clock::Clock;
use crate::error::AuthError;
use crate::http_client::HttpClient;
use crate::identity::{Account, AccountId, EmailAddress};
use crate::plugin::Auth;
use crate::session::Privilege;
use crate::storage::StaffStore;

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
        if session.privilege == Privilege::Anonymous {
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
    /// An account had the address and was not a superuser. Now it is. Its
    /// password did not change.
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
/// # Errors
/// Returns [`AuthError::InvalidEmail`] for an address that is not one, and
/// fails when the store or the hasher fails.
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
        Some(account) => SuperuserOutcome::Promoted(account.id),
        None => match accounts.register(email, password).await?.notice {
            RegistrationNotice::Created { account, .. } => SuperuserOutcome::Created(account.id),
            // Registered between the read above and the insert: promote the
            // account that won.
            RegistrationNotice::AlreadyRegistered { email } => {
                let account = store
                    .account_by_email(&email)
                    .await?
                    .ok_or_else(|| AuthError::Store("an address is taken by no account".into()))?;
                SuperuserOutcome::Promoted(account.id)
            }
        },
    };
    store.set_superuser(outcome.account(), true).await?;
    Ok(outcome)
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
