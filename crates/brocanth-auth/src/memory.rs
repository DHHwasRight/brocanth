//! An in-memory [`AuthStore`], so the tests need no server and an adapter
//! author has something to compare against.
//!
//! It is not a cache and it is not for production: everything disappears with
//! the process and nothing is indexed. What it *is* is the executable
//! specification of the trait's two hard requirements — the email is unique,
//! and [`MemoryStore::consume_token`] deletes and returns under one lock.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

use crate::clock::UnixTime;
use crate::identity::{Account, AccountId, EmailAddress, ProviderLink};
use crate::session::SessionId;
use crate::storage::{
    AuthStore, NewAccount, NewProviderLink, SessionRecord, StoreError, TokenRecord,
};
use crate::tokens::TokenPurpose;

#[derive(Default)]
struct State {
    accounts: HashMap<AccountId, Account>,
    links: HashMap<(String, String), ProviderLink>,
    sessions: HashMap<SessionId, SessionRecord>,
    tokens: HashMap<(TokenPurpose, String), TokenRecord>,
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

    async fn account_by_email(
        &self,
        email: &EmailAddress,
    ) -> Result<Option<Account>, StoreError> {
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
            if state.accounts.values().any(|a| a.email.as_ref() == Some(email)) {
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

    async fn set_email_verified(
        &self,
        id: &AccountId,
        verified: bool,
    ) -> Result<(), StoreError> {
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

    async fn unlink_provider(
        &self,
        account: &AccountId,
        provider: &str,
    ) -> Result<(), StoreError> {
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

    async fn delete_sessions_for_account(
        &self,
        account: &AccountId,
    ) -> Result<u64, StoreError> {
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
