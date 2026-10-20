//! An in-memory [`AuthStore`], so the tests need no server and an adapter
//! author has something to compare against.
//!
//! It is not a cache and it is not for production: everything disappears with
//! the process and nothing is indexed. What it *is* is the executable
//! specification of the trait's two hard requirements — the email is unique,
//! and [`MemoryStore::consume_token`] deletes and returns under one lock.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use async_trait::async_trait;

use crate::clock::UnixTime;
use crate::identity::{Account, AccountId, EmailAddress, ProviderLink};
use crate::otp::OtpChallengeId;
use crate::session::SessionId;
use crate::storage::{
    AccountDirectory, AuthStore, MfaStore, NewAccount, NewProviderLink, OtpChallengeRecord,
    OtpStore, SessionRecord, SignInLocationStore, StoreError, TokenRecord, TotpSecretRecord,
};
use crate::tokens::TokenPurpose;

#[derive(Default)]
struct State {
    accounts: HashMap<AccountId, Account>,
    links: HashMap<(String, String), ProviderLink>,
    sessions: HashMap<SessionId, SessionRecord>,
    tokens: HashMap<(TokenPurpose, String), TokenRecord>,
    totp: HashMap<AccountId, TotpSecretRecord>,
    recovery: HashMap<AccountId, HashSet<[u8; 32]>>,
    otp: HashMap<OtpChallengeId, OtpChallengeRecord>,
    locations: HashMap<(AccountId, [u8; 32]), UnixTime>,
}

/// An [`AuthStore`] in a `Mutex<HashMap>`.
#[derive(Default)]
pub struct MemoryStore {
    state: Mutex<State>,
    now: Mutex<UnixTime>,
}

impl MemoryStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the timestamp this store writes into rows it creates.
    ///
    /// The store stamps `created_at` and `linked_at` itself, exactly as a
    /// database with a default would, so a test that wants a known timestamp
    /// tells the store rather than the caller.
    pub fn set_now(&self, now: UnixTime) {
        *self.now.lock().expect("memory store lock") = now;
    }

    /// How many sessions are stored. For assertions.
    pub fn session_count(&self) -> usize {
        self.state.lock().expect("memory store lock").sessions.len()
    }

    /// How many accounts are stored. For assertions.
    pub fn account_count(&self) -> usize {
        self.state.lock().expect("memory store lock").accounts.len()
    }

    /// How many one-time-code challenges are outstanding. For assertions.
    pub fn otp_challenge_count(&self) -> usize {
        self.state.lock().expect("memory store lock").otp.len()
    }

    fn stamp(&self) -> UnixTime {
        *self.now.lock().expect("memory store lock")
    }
}

impl std::fmt::Debug for MemoryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MemoryStore")
    }
}

#[async_trait]
impl AuthStore for MemoryStore {
    async fn account(&self, id: &AccountId) -> Result<Option<Account>, StoreError> {
        Ok(self
            .state
            .lock()
            .expect("memory store lock")
            .accounts
            .get(id)
            .cloned())
    }

    async fn account_by_email(&self, email: &EmailAddress) -> Result<Option<Account>, StoreError> {
        Ok(self
            .state
            .lock()
            .expect("memory store lock")
            .accounts
            .values()
            .find(|a| a.email.as_ref() == Some(email))
            .cloned())
    }

    async fn create_account(&self, new: NewAccount) -> Result<Account, StoreError> {
        let id = AccountId::generate().map_err(|e| StoreError::backend(e.to_string()))?;
        let created_at = self.stamp();
        let mut state = self.state.lock().expect("memory store lock");
        // The unique index, by hand. A store without one lets two concurrent
        // registrations of the same address both win.
        if let Some(email) = &new.email {
            if state
                .accounts
                .values()
                .any(|a| a.email.as_ref() == Some(email))
            {
                return Err(StoreError::EmailTaken);
            }
        }
        let account = Account {
            id: id.clone(),
            email: new.email,
            email_verified: new.email_verified,
            password_hash: new.password_hash,
            created_at,
        };
        state.accounts.insert(id, account.clone());
        Ok(account)
    }

    async fn set_password_hash(
        &self,
        id: &AccountId,
        hash: Option<&str>,
    ) -> Result<(), StoreError> {
        let mut state = self.state.lock().expect("memory store lock");
        let account = state
            .accounts
            .get_mut(id)
            .ok_or_else(|| StoreError::backend(format!("no account `{id}`")))?;
        account.password_hash = hash.map(str::to_string);
        Ok(())
    }

    async fn set_email_verified(&self, id: &AccountId, verified: bool) -> Result<(), StoreError> {
        let mut state = self.state.lock().expect("memory store lock");
        let account = state
            .accounts
            .get_mut(id)
            .ok_or_else(|| StoreError::backend(format!("no account `{id}`")))?;
        account.email_verified = verified;
        Ok(())
    }

    async fn link(
        &self,
        provider: &str,
        subject: &str,
    ) -> Result<Option<ProviderLink>, StoreError> {
        Ok(self
            .state
            .lock()
            .expect("memory store lock")
            .links
            .get(&(provider.to_string(), subject.to_string()))
            .cloned())
    }

    async fn links_for_account(
        &self,
        account: &AccountId,
    ) -> Result<Vec<ProviderLink>, StoreError> {
        Ok(self
            .state
            .lock()
            .expect("memory store lock")
            .links
            .values()
            .filter(|l| &l.account == account)
            .cloned()
            .collect())
    }

    async fn link_provider(
        &self,
        account: &AccountId,
        new: NewProviderLink,
    ) -> Result<ProviderLink, StoreError> {
        let linked_at = self.stamp();
        let mut state = self.state.lock().expect("memory store lock");
        let key = (new.provider.clone(), new.subject.clone());
        if let Some(existing) = state.links.get(&key) {
            if &existing.account != account {
                return Err(StoreError::backend(
                    "that provider subject is already linked to another account",
                ));
            }
        }
        let link = ProviderLink {
            account: account.clone(),
            provider: new.provider,
            subject: new.subject,
            email: new.email,
            linked_at,
        };
        state.links.insert(key, link.clone());
        Ok(link)
    }

    async fn unlink_provider(&self, account: &AccountId, provider: &str) -> Result<(), StoreError> {
        let mut state = self.state.lock().expect("memory store lock");
        state
            .links
            .retain(|(p, _), l| !(p == provider && &l.account == account));
        Ok(())
    }

    async fn create_session(&self, session: SessionRecord) -> Result<(), StoreError> {
        self.state
            .lock()
            .expect("memory store lock")
            .sessions
            .insert(session.id.clone(), session);
        Ok(())
    }

    async fn session(&self, id: &SessionId) -> Result<Option<SessionRecord>, StoreError> {
        Ok(self
            .state
            .lock()
            .expect("memory store lock")
            .sessions
            .get(id)
            .cloned())
    }

    async fn touch_session(&self, id: &SessionId, at: UnixTime) -> Result<(), StoreError> {
        if let Some(s) = self
            .state
            .lock()
            .expect("memory store lock")
            .sessions
            .get_mut(id)
        {
            s.last_seen_at = at;
        }
        Ok(())
    }

    async fn delete_session(&self, id: &SessionId) -> Result<(), StoreError> {
        self.state
            .lock()
            .expect("memory store lock")
            .sessions
            .remove(id);
        Ok(())
    }

    async fn delete_sessions_for_account(&self, account: &AccountId) -> Result<u64, StoreError> {
        let mut state = self.state.lock().expect("memory store lock");
        let before = state.sessions.len();
        state.sessions.retain(|_, s| &s.account != account);
        Ok((before - state.sessions.len()) as u64)
    }

    async fn store_token(&self, token: TokenRecord) -> Result<(), StoreError> {
        self.state
            .lock()
            .expect("memory store lock")
            .tokens
            .insert((token.purpose, token.selector.clone()), token);
        Ok(())
    }

    async fn consume_token(
        &self,
        purpose: TokenPurpose,
        selector: &str,
    ) -> Result<Option<TokenRecord>, StoreError> {
        // One lock, remove-and-return. This is the atomicity the trait
        // demands; in SQL it is `DELETE ... RETURNING`.
        Ok(self
            .state
            .lock()
            .expect("memory store lock")
            .tokens
            .remove(&(purpose, selector.to_string())))
    }

    async fn delete_tokens_for_account(
        &self,
        account: &AccountId,
        purpose: TokenPurpose,
    ) -> Result<(), StoreError> {
        let mut state = self.state.lock().expect("memory store lock");
        state
            .tokens
            .retain(|(p, _), t| !(*p == purpose && &t.account == account));
        Ok(())
    }
}

#[async_trait]
impl AccountDirectory for MemoryStore {
    async fn accounts(&self, limit: u32, offset: u32) -> Result<Vec<Account>, StoreError> {
        // Sorted on every call, because nothing here is indexed and the trait
        // promises one order. A real adapter writes `ORDER BY created_at DESC,
        // id` and lets the database do it.
        let mut all: Vec<Account> = self
            .state
            .lock()
            .expect("memory store lock")
            .accounts
            .values()
            .cloned()
            .collect();
        all.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(all
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect())
    }

    async fn count_accounts(&self) -> Result<u64, StoreError> {
        Ok(self.state.lock().expect("memory store lock").accounts.len() as u64)
    }

    async fn sessions_for_account(
        &self,
        account: &AccountId,
    ) -> Result<Vec<SessionRecord>, StoreError> {
        let mut found: Vec<SessionRecord> = self
            .state
            .lock()
            .expect("memory store lock")
            .sessions
            .values()
            .filter(|s| &s.account == account)
            .cloned()
            .collect();
        found.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(found)
    }

    fn is_durable(&self) -> bool {
        // It is a `HashMap` behind a `Mutex`. Everything in it goes when the
        // process does, and a screen listing these accounts has to say so.
        false
    }
}

#[async_trait]
impl MfaStore for MemoryStore {
    async fn totp_secret(
        &self,
        account: &AccountId,
    ) -> Result<Option<TotpSecretRecord>, StoreError> {
        Ok(self
            .state
            .lock()
            .expect("memory store lock")
            .totp
            .get(account)
            .cloned())
    }

    async fn put_totp_secret(
        &self,
        account: &AccountId,
        record: TotpSecretRecord,
    ) -> Result<(), StoreError> {
        self.state
            .lock()
            .expect("memory store lock")
            .totp
            .insert(account.clone(), record);
        Ok(())
    }

    async fn confirm_totp_secret(
        &self,
        account: &AccountId,
        at: UnixTime,
    ) -> Result<(), StoreError> {
        if let Some(record) = self
            .state
            .lock()
            .expect("memory store lock")
            .totp
            .get_mut(account)
        {
            record.confirmed_at = Some(at);
        }
        Ok(())
    }

    async fn delete_totp_secret(&self, account: &AccountId) -> Result<(), StoreError> {
        self.state
            .lock()
            .expect("memory store lock")
            .totp
            .remove(account);
        Ok(())
    }

    async fn claim_totp_step(&self, account: &AccountId, step: i64) -> Result<bool, StoreError> {
        // One lock, compare-and-set. This is the replay defence: strictly
        // greater, so the step a code was accepted for cannot be accepted
        // again. In SQL it is a conditional `UPDATE` and a row count.
        let mut state = self.state.lock().expect("memory store lock");
        let Some(record) = state.totp.get_mut(account) else {
            return Ok(false);
        };
        if step <= record.last_step {
            return Ok(false);
        }
        record.last_step = step;
        Ok(true)
    }

    async fn replace_recovery_codes(
        &self,
        account: &AccountId,
        digests: Vec<[u8; 32]>,
    ) -> Result<(), StoreError> {
        let mut state = self.state.lock().expect("memory store lock");
        if digests.is_empty() {
            state.recovery.remove(account);
        } else {
            state
                .recovery
                .insert(account.clone(), digests.into_iter().collect());
        }
        Ok(())
    }

    async fn consume_recovery_code(
        &self,
        account: &AccountId,
        digest: &[u8; 32],
    ) -> Result<bool, StoreError> {
        // Remove-and-report under one lock. Deleting is the check.
        Ok(self
            .state
            .lock()
            .expect("memory store lock")
            .recovery
            .get_mut(account)
            .is_some_and(|set| set.remove(digest)))
    }

    async fn count_recovery_codes(&self, account: &AccountId) -> Result<u32, StoreError> {
        Ok(self
            .state
            .lock()
            .expect("memory store lock")
            .recovery
            .get(account)
            .map_or(0, |set| set.len() as u32))
    }
}

#[async_trait]
impl OtpStore for MemoryStore {
    async fn create_otp_challenge(&self, challenge: OtpChallengeRecord) -> Result<(), StoreError> {
        self.state
            .lock()
            .expect("memory store lock")
            .otp
            .insert(challenge.id.clone(), challenge);
        Ok(())
    }

    async fn claim_otp_attempt(
        &self,
        id: &OtpChallengeId,
        now: UnixTime,
    ) -> Result<Option<OtpChallengeRecord>, StoreError> {
        // Increment and conditionally delete, under one lock. Two guesses
        // cannot share one attempt.
        let mut state = self.state.lock().expect("memory store lock");
        let Some(record) = state.otp.get_mut(id) else {
            return Ok(None);
        };
        record.attempts += 1;
        let claimed = record.clone();
        if claimed.attempts >= claimed.max_attempts || now >= claimed.expires_at {
            state.otp.remove(id);
        }
        Ok(Some(claimed))
    }

    async fn delete_otp_challenge(&self, id: &OtpChallengeId) -> Result<(), StoreError> {
        self.state.lock().expect("memory store lock").otp.remove(id);
        Ok(())
    }

    async fn purge_otp_challenges(&self, now: UnixTime) -> Result<u64, StoreError> {
        let mut state = self.state.lock().expect("memory store lock");
        let before = state.otp.len();
        state.otp.retain(|_, c| c.expires_at > now);
        Ok((before - state.otp.len()) as u64)
    }
}

#[async_trait]
impl SignInLocationStore for MemoryStore {
    async fn record_sign_in_location(
        &self,
        account: &AccountId,
        fingerprint: &[u8; 32],
        at: UnixTime,
    ) -> Result<Option<UnixTime>, StoreError> {
        // Read the old value and write the new one under one lock, so two
        // sign-ins at once cannot both call the network new.
        Ok(self
            .state
            .lock()
            .expect("memory store lock")
            .locations
            .insert((account.clone(), *fingerprint), at))
    }

    async fn count_sign_in_locations(&self, account: &AccountId) -> Result<u64, StoreError> {
        Ok(self
            .state
            .lock()
            .expect("memory store lock")
            .locations
            .keys()
            .filter(|(who, _)| who == account)
            .count() as u64)
    }

    async fn purge_sign_in_locations(&self, unseen_since: UnixTime) -> Result<u64, StoreError> {
        let mut state = self.state.lock().expect("memory store lock");
        let before = state.locations.len();
        state.locations.retain(|_, seen| *seen >= unseen_since);
        Ok((before - state.locations.len()) as u64)
    }
}
