//! Identity linking: one account, several providers, and the rule that one
//! matching email address is not a proof of anything.
//!
//! # The bug this module exists to not have
//!
//! A user signs in with a provider. The provider reports
//! `alice@example.com`. An account with that address already exists. The
//! convenient thing to do is attach the provider to that account and let them
//! in, and several real products have shipped exactly that.
//!
//! It is an account takeover. To use it, register an account at any provider
//! that lets you set an address without proving it — or that reports an
//! address its own user typed into a profile field — put the victim's address
//! on it, and sign in. You are now inside their account. The provider never
//! claimed the address was theirs; the application inferred it.
//!
//! Restricting the rule to *verified* addresses narrows the hole and does not
//! close it. "Verified" means verified **to the provider**, and the provider's
//! verification may be older than the address's current owner, may be a
//! corporate directory an administrator can edit, or may simply be a field the
//! provider does not document as verified at all — which is the case for three
//! of the eight providers in this crate's catalogue.
//!
//! # So the rule here is simple and has no exceptions
//!
//! **A provider is attached to an existing account only by a request made
//! from a session already authenticated as that account.** [`Linking::sign_in`]
//! never links; when the address collides it answers
//! [`SignInOutcome::LinkRequiresAuthentication`] and the application asks the
//! person to sign in the way they already can, and then calls
//! [`Linking::link`].
//!
//! The matching is on [`ProviderIdentity::subject`], never on the address. A
//! subject is the provider's own identifier and does not change hands; an
//! address does.
//!
//! # If you are here to change this
//!
//! The thing you are about to add is "link automatically when
//! `identity.email_verified` is true". Before you do: the address is
//! controlled by the provider, the provider is chosen by the person signing
//! in, and the account being taken over belongs to somebody else. The person
//! who benefits from the convenience is the attacker, because the legitimate
//! user already has a way in — that is what made the collision happen.

use std::sync::Arc;

use crate::clock::Clock;
use crate::error::AuthError;
use crate::identity::{Account, AccountId, ProviderIdentity, ProviderLink};
use crate::mfa::{FactorKind, FactorSet, Verified};
use crate::storage::{AuthStore, NewAccount, NewProviderLink};

/// Why a sign-in could not finish on its own.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LinkRefusal {
    /// The address the provider reported already belongs to an account, and
    /// this crate will not attach a provider to an account on that evidence.
    ///
    /// Note that the refusal does not depend on whether the provider called
    /// the address verified. See the module documentation.
    EmailBelongsToAnotherAccount,
}

/// What happened when somebody signed in with a provider.
///
/// Not `Clone`: the two successful variants carry a [`Verified`], and a proof
/// that can be copied is a proof that mints two sessions.
#[derive(Debug)]
pub enum SignInOutcome {
    /// The provider was already linked. Open a session.
    SignedIn {
        /// The account.
        account: Account,
        /// The link that matched.
        link: ProviderLink,
        /// Proof that a primary factor landed — completing an OAuth 2.0 or OIDC
        /// flow is as much a primary factor as typing a password, so this path
        /// mints the same witness the password path does. Without it, one route
        /// would be the hole the other closed. See [`crate::mfa`].
        proof: Verified,
    },
    /// Nothing matched and nothing collided, so an account was created and the
    /// provider attached to it.
    Registered {
        /// The new account.
        account: Account,
        /// The new link.
        link: ProviderLink,
        /// Proof that a primary factor landed. A brand-new account owes no
        /// second factor, so [`FactorPolicy::admit`] will admit it — but it is
        /// still the policy that says so, not this module.
        ///
        /// [`FactorPolicy::admit`]: crate::mfa::FactorPolicy::admit
        proof: Verified,
    },
    /// The sign-in stops here and the application must ask the person to
    /// authenticate as the existing account, then call [`Linking::link`].
    ///
    /// Deliberately carries no [`Account`]: the application does not need to
    /// know *which* account collided in order to say "sign in with your
    /// existing method and then connect this provider from your settings", and
    /// handing it over would make it easy to build the screen that leaks
    /// whether an address is registered.
    LinkRequiresAuthentication {
        /// The identity that was waiting to be attached. Carry it through the
        /// authentication — in a signed, short-lived cookie — and hand it back
        /// to [`Linking::link`].
        identity: ProviderIdentity,
        /// Why.
        reason: LinkRefusal,
    },
}

/// Finds, creates and attaches identities.
pub struct Linking<S: AuthStore, C: Clock> {
    store: Arc<S>,
    clock: Arc<C>,
}

impl<S: AuthStore, C: Clock> Linking<S, C> {
    /// Builds the service.
    pub fn new(store: Arc<S>, clock: Arc<C>) -> Self {
        Self { store, clock }
    }

    /// Resolves a provider identity to an account, without ever linking by
    /// email.
    ///
    /// Order:
    /// 1. A link on `(provider, subject)` — sign in.
    /// 2. An address that belongs to somebody — stop, and ask for proof.
    /// 3. Otherwise — create an account and attach the provider.
    ///
    /// Step 3 records the address as verified only when the provider asserted
    /// it. An unverified address on a *new* account is not a takeover risk —
    /// there is nobody to take over — so it is stored, unverified, and the
    /// application can send a verification mail like any other.
    ///
    /// # Errors
    /// Fails when the store fails.
    pub async fn sign_in(&self, identity: ProviderIdentity) -> Result<SignInOutcome, AuthError> {
        if let Some(link) = self
            .store
            .link(&identity.provider, &identity.subject)
            .await?
        {
            let account = self
                .store
                .account(&link.account)
                .await?
                .ok_or_else(|| AuthError::Store("a link points at no account".into()))?;
            let proof = Verified::minted(account.id.clone(), FactorSet::of([FactorKind::Provider]));
            return Ok(SignInOutcome::SignedIn {
                account,
                link,
                proof,
            });
        }

        if let Some(email) = &identity.email {
            if self.store.account_by_email(email).await?.is_some() {
                // No branch on `identity.email_verified`. See the module
                // documentation: a verified address is verified to the
                // provider, and that is not consent from the account holder.
                return Ok(SignInOutcome::LinkRequiresAuthentication {
                    identity,
                    reason: LinkRefusal::EmailBelongsToAnotherAccount,
                });
            }
        }

        let account = self
            .store
            .create_account(NewAccount {
                email: identity.email.clone(),
                email_verified: identity.email_verified,
                // No password. An account created this way signs in through
                // its provider until its owner sets one, and a `None` hash is
                // what stops `Accounts::login` from accepting anything.
                password_hash: None,
            })
            .await?;
        let link = self.attach(&account.id, &identity).await?;
        let proof = Verified::minted(account.id.clone(), FactorSet::of([FactorKind::Provider]));
        Ok(SignInOutcome::Registered {
            account,
            link,
            proof,
        })
    }

    /// Attaches a provider to an account.
    ///
    /// **The caller must already have authenticated the session as
    /// `account`.** This function cannot check that — it has no session — and
    /// calling it from an unauthenticated handler reintroduces exactly the
    /// takeover the rest of this module prevents. There is one caller in a
    /// correct application and it is the settings page.
    ///
    /// # Errors
    /// Returns [`AuthError::AlreadyLinked`] when the provider identity belongs
    /// to a different account. Re-linking the same identity to the same
    /// account succeeds and refreshes the stored address.
    pub async fn link(
        &self,
        account: &AccountId,
        identity: &ProviderIdentity,
    ) -> Result<ProviderLink, AuthError> {
        if let Some(existing) = self
            .store
            .link(&identity.provider, &identity.subject)
            .await?
        {
            if &existing.account != account {
                return Err(AuthError::AlreadyLinked);
            }
        }
        self.attach(account, identity).await
    }

    /// Detaches a provider.
    ///
    /// Refuses to remove the last way into an account: an account with no
    /// password and one provider that unlinks it is an account nobody can
    /// reach, and the support ticket that follows is unanswerable.
    ///
    /// # Errors
    /// Returns [`AuthError::Misconfigured`] when this is the last credential,
    /// which is a programming error in the calling screen rather than a user
    /// error.
    pub async fn unlink(&self, account: &AccountId, provider: &str) -> Result<(), AuthError> {
        let links = self.store.links_for_account(account).await?;
        let has_password = self
            .store
            .account(account)
            .await?
            .and_then(|a| a.password_hash)
            .is_some();
        let remaining = links.iter().filter(|l| l.provider != provider).count();
        if remaining == 0 && !has_password {
            return Err(AuthError::misconfigured(
                "that is the only way into this account; set a password before unlinking",
            ));
        }
        self.store.unlink_provider(account, provider).await?;
        Ok(())
    }

    /// Every provider attached to an account.
    ///
    /// # Errors
    /// Fails when the store fails.
    pub async fn links(&self, account: &AccountId) -> Result<Vec<ProviderLink>, AuthError> {
        Ok(self.store.links_for_account(account).await?)
    }

    async fn attach(
        &self,
        account: &AccountId,
        identity: &ProviderIdentity,
    ) -> Result<ProviderLink, AuthError> {
        let _ = self.clock.now();
        Ok(self
            .store
            .link_provider(
                account,
                NewProviderLink {
                    provider: identity.provider.clone(),
                    subject: identity.subject.clone(),
                    email: identity.email.clone(),
                },
            )
            .await?)
    }
}
