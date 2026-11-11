//! Where connections live: one row per (account, provider).
//!
//! A store keeps [`StoredToken`]s and never sees a token in clear. The
//! [`crate::OauthClient`] seals the access and the refresh token before
//! `put` and opens them after `get`. So every store, the ones written
//! outside this crate too, keeps tokens encrypted at rest without code of
//! its own for it.
//!
//! [`MemoryTokenStore`] is for tests and development: a restart loses every
//! connection. `PgTokenStore` (feature `postgres`) is for production.
//! [`crate::conformance`] (feature `conformance`) holds both to one contract.

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;

/// One connection, as a store keeps it. The tokens are sealed.
#[derive(Clone, PartialEq, Eq)]
pub struct StoredToken {
    /// The account that connected.
    pub account: String,
    /// The provider id.
    pub provider: String,
    /// The access token, sealed.
    pub access: Vec<u8>,
    /// The refresh token, sealed, when the provider gave one.
    pub refresh: Option<Vec<u8>>,
    /// `Bearer`, most of the time.
    pub token_type: String,
    /// The scopes the provider granted.
    pub scopes: Vec<String>,
    /// When the access token runs out, in seconds since 1970. `None` when
    /// the provider did not say.
    pub expires_at: Option<i64>,
    /// When the person connected, in seconds since 1970.
    pub connected_at: i64,
    /// When the token was last written: the connect, or the last refresh.
    pub refreshed_at: i64,
}

impl std::fmt::Debug for StoredToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredToken")
            .field("account", &self.account)
            .field("provider", &self.provider)
            .field(
                "access",
                &format_args!("<{} sealed bytes>", self.access.len()),
            )
            .field("refresh", &self.refresh.as_ref().map(|r| r.len()))
            .field("token_type", &self.token_type)
            .field("scopes", &self.scopes)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

/// A connection without its tokens: what a list, a page or the admin
/// shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connection {
    /// The account that connected.
    pub account: String,
    /// The provider id.
    pub provider: String,
    /// The scopes the provider granted.
    pub scopes: Vec<String>,
    /// When the access token runs out.
    pub expires_at: Option<i64>,
    /// Whether a refresh token is kept.
    pub has_refresh: bool,
    /// When the person connected.
    pub connected_at: i64,
    /// When the token was last written.
    pub refreshed_at: i64,
}

impl From<&StoredToken> for Connection {
    fn from(t: &StoredToken) -> Self {
        Self {
            account: t.account.clone(),
            provider: t.provider.clone(),
            scopes: t.scopes.clone(),
            expires_at: t.expires_at,
            has_refresh: t.refresh.is_some(),
            connected_at: t.connected_at,
            refreshed_at: t.refreshed_at,
        }
    }
}

/// A store failed. The text never holds a token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreError(pub String);

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "token store: {}", self.0)
    }
}

impl std::error::Error for StoreError {}

/// Where connections live.
///
/// Every method is keyed by `(account, provider)`. `put` replaces the row
/// whole, in one step: a reader sees the old row or the new one, never half
/// of each.
#[async_trait]
pub trait TokenStore: Send + Sync + 'static {
    /// The row, or `None`.
    async fn get(&self, account: &str, provider: &str) -> Result<Option<StoredToken>, StoreError>;

    /// Inserts the row, or replaces the row with the same key.
    async fn put(&self, token: StoredToken) -> Result<(), StoreError>;

    /// Deletes the row. `true` when there was one.
    async fn delete(&self, account: &str, provider: &str) -> Result<bool, StoreError>;

    /// The account's connections, by provider id.
    async fn for_account(&self, account: &str) -> Result<Vec<Connection>, StoreError>;

    /// Every connection, by account then provider, `limit` from `offset`.
    async fn list(&self, limit: usize, offset: usize) -> Result<Vec<Connection>, StoreError>;
}

/// Connections in this process's memory. For tests and development: a
/// restart loses every connection, and two processes do not share one.
#[derive(Debug, Default)]
pub struct MemoryTokenStore {
    rows: Mutex<BTreeMap<(String, String), StoredToken>>,
}

impl MemoryTokenStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    fn rows(&self) -> std::sync::MutexGuard<'_, BTreeMap<(String, String), StoredToken>> {
        // A panic while the lock was held leaves whole rows: `put` swaps a
        // row in one step.
        self.rows.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[async_trait]
impl TokenStore for MemoryTokenStore {
    async fn get(&self, account: &str, provider: &str) -> Result<Option<StoredToken>, StoreError> {
        Ok(self
            .rows()
            .get(&(account.to_string(), provider.to_string()))
            .cloned())
    }

    async fn put(&self, token: StoredToken) -> Result<(), StoreError> {
        self.rows()
            .insert((token.account.clone(), token.provider.clone()), token);
        Ok(())
    }

    async fn delete(&self, account: &str, provider: &str) -> Result<bool, StoreError> {
        Ok(self
            .rows()
            .remove(&(account.to_string(), provider.to_string()))
            .is_some())
    }

    async fn for_account(&self, account: &str) -> Result<Vec<Connection>, StoreError> {
        Ok(self
            .rows()
            .values()
            .filter(|t| t.account == account)
            .map(Connection::from)
            .collect())
    }

    async fn list(&self, limit: usize, offset: usize) -> Result<Vec<Connection>, StoreError> {
        Ok(self
            .rows()
            .values()
            .skip(offset)
            .take(limit)
            .map(Connection::from)
            .collect())
    }
}
