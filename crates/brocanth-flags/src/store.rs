//! Where the rules are kept: [`FlagStore`], and [`MemoryStore`] for a test
//! or a project with no database.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Mutex;

use async_trait::async_trait;

use crate::rule::Rule;

/// A store failed. The message says what, and for a setup mistake how to
/// fix it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreError(String);

impl StoreError {
    /// A failure with this message.
    pub fn new(message: impl Into<String>) -> StoreError {
        StoreError(message.into())
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StoreError {}

/// Where the rules live between two processes.
///
/// Small on purpose: an application has tens of flags, so the copy in
/// memory is every row, read in one query.
#[async_trait]
pub trait FlagStore: Send + Sync {
    /// Every stored rule, by key.
    async fn load(&self) -> Result<BTreeMap<String, Rule>, StoreError>;

    /// Writes the rule for `key`, replacing the one there.
    async fn save(&self, key: &str, rule: &Rule) -> Result<(), StoreError>;

    /// Removes the rule for `key`. The flag is then off.
    async fn remove(&self, key: &str) -> Result<(), StoreError>;
}

/// Rules in memory, lost at restart. For a test, and for a project that
/// registered the plugin without a database.
#[derive(Debug, Default)]
pub struct MemoryStore {
    rules: Mutex<BTreeMap<String, Rule>>,
}

impl MemoryStore {
    /// An empty store.
    pub fn new() -> MemoryStore {
        MemoryStore::default()
    }

    fn rules(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Rule>> {
        self.rules.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[async_trait]
impl FlagStore for MemoryStore {
    async fn load(&self) -> Result<BTreeMap<String, Rule>, StoreError> {
        Ok(self.rules().clone())
    }

    async fn save(&self, key: &str, rule: &Rule) -> Result<(), StoreError> {
        self.rules().insert(key.to_string(), rule.clone());
        Ok(())
    }

    async fn remove(&self, key: &str) -> Result<(), StoreError> {
        self.rules().remove(key);
        Ok(())
    }
}
