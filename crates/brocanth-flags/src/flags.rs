//! [`Flags`]: the in-process copy of every rule, and the one read a page
//! makes.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use crate::rule::{valid_key, Actor, Flag, Rule};
use crate::store::{FlagStore, StoreError};

/// Every rule, held in memory, and the store behind them.
///
/// A page reads a flag on every request, so the read must not wait:
/// [`Flags::enabled`] takes a read lock on a map and returns. The copy is
/// filled by [`Flags::refresh`], which the plugin calls once before the
/// server listens and then every `refresh_secs`. A change made through
/// [`Flags::set`] reaches this process at once and the others at their
/// next refresh.
///
/// Cheap to clone: an `Arc`.
#[derive(Clone)]
pub struct Flags {
    inner: Arc<Inner>,
}

struct Inner {
    rules: RwLock<BTreeMap<String, Rule>>,
    store: OnceLock<Arc<dyn FlagStore>>,
    last_error: Mutex<Option<String>>,
}

impl Default for Flags {
    fn default() -> Self {
        Flags::new()
    }
}

impl std::fmt::Debug for Flags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Flags")
            .field("rules", &self.read().len())
            .field("store", &self.inner.store.get().is_some())
            .finish()
    }
}

impl Flags {
    /// An empty copy with no store: every flag is off until a store is
    /// attached and read.
    pub fn new() -> Flags {
        Flags {
            inner: Arc::new(Inner {
                rules: RwLock::new(BTreeMap::new()),
                store: OnceLock::new(),
                last_error: Mutex::new(None),
            }),
        }
    }

    /// A copy over `store`. Call [`Flags::refresh`] to read it.
    pub fn with_store(store: Arc<dyn FlagStore>) -> Flags {
        let flags = Flags::new();
        flags.attach(store);
        flags
    }

    /// Sets the store, once. A second call keeps the first store and
    /// returns `false`.
    pub fn attach(&self, store: Arc<dyn FlagStore>) -> bool {
        self.inner.store.set(store).is_ok()
    }

    /// Whether a store is attached.
    pub fn has_store(&self) -> bool {
        self.inner.store.get().is_some()
    }

    /// Whether `flag` is on for `actor`. Never waits.
    pub fn enabled(&self, flag: Flag, actor: &Actor) -> bool {
        self.enabled_key(flag.key(), actor)
    }

    /// [`Flags::enabled`] for a key known only at run time. An unknown key
    /// is off.
    pub fn enabled_key(&self, key: &str, actor: &Actor) -> bool {
        self.read()
            .get(key)
            .is_some_and(|rule| rule.allows(key, actor))
    }

    /// The rule stored for `key`, if any.
    pub fn rule(&self, key: &str) -> Option<Rule> {
        self.read().get(key).cloned()
    }

    /// Every stored rule, sorted by key.
    pub fn rules(&self) -> Vec<(String, Rule)> {
        self.read()
            .iter()
            .map(|(key, rule)| (key.clone(), rule.clone()))
            .collect()
    }

    /// Reads every rule from the store and replaces the copy.
    ///
    /// # Errors
    ///
    /// The store failed. The copy is left as it was, so a database that
    /// goes away keeps the last rules it gave rather than turning every
    /// flag off. The message is kept for [`Flags::last_error`].
    pub async fn refresh(&self) -> Result<(), StoreError> {
        let Some(store) = self.inner.store.get() else {
            return Ok(());
        };
        match store.load().await {
            Ok(rules) => {
                *self.write() = rules;
                self.note(None);
                Ok(())
            }
            Err(e) => {
                self.note(Some(e.to_string()));
                Err(e)
            }
        }
    }

    /// Stores `rule` for `key`, then updates this process's copy.
    ///
    /// # Errors
    ///
    /// The key is not a valid one (see [`valid_key`]), no store is
    /// attached, or the store failed. The copy changes only after the
    /// store accepted the rule.
    pub async fn set(&self, key: &str, rule: Rule) -> Result<(), StoreError> {
        if !valid_key(key) {
            return Err(StoreError::new(format!(
                "`{key}` is not a flag key\n\
                 help: use 1 to 64 characters of a-z, 0-9, `_`, `-` and `.`"
            )));
        }
        let rule = Rule {
            percent: rule.percent.min(100),
            ..rule
        };
        self.store()?.save(key, &rule).await?;
        self.write().insert(key.to_string(), rule);
        Ok(())
    }

    /// Removes the rule for `key`, so the flag is off, in the store and in
    /// this process's copy.
    ///
    /// # Errors
    ///
    /// No store is attached, or the store failed.
    pub async fn remove(&self, key: &str) -> Result<(), StoreError> {
        self.store()?.remove(key).await?;
        self.write().remove(key);
        Ok(())
    }

    /// What the last refresh failed with, while it is still failing.
    pub fn last_error(&self) -> Option<String> {
        self.inner
            .last_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn store(&self) -> Result<&Arc<dyn FlagStore>, StoreError> {
        self.inner.store.get().ok_or_else(|| {
            StoreError::new(
                "the flags have no store\n\
                 help: register `brocanth_flags::FlagsPlugin` after the `db` plugin, \
                 or build `Flags::with_store`",
            )
        })
    }

    fn note(&self, error: Option<String>) {
        *self
            .inner
            .last_error
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = error;
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, BTreeMap<String, Rule>> {
        self.inner.rules.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, BTreeMap<String, Rule>> {
        self.inner.rules.write().unwrap_or_else(|e| e.into_inner())
    }
}

impl brocanth::plugin_context::ContextItem for Flags {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemoryStore;

    const BETA: Flag = Flag::new("beta", "The beta.");

    #[tokio::test]
    async fn a_set_rule_is_read_at_once_and_survives_a_refresh() {
        let store = Arc::new(MemoryStore::new());
        let flags = Flags::with_store(store.clone());
        let ada = Actor::new("ada");
        assert!(!flags.enabled(BETA, &ada));
        flags.set("beta", Rule::everyone()).await.unwrap();
        assert!(flags.enabled(BETA, &ada));
        flags.refresh().await.unwrap();
        assert!(flags.enabled(BETA, &ada));

        // Another process wrote the store; this one sees it on refresh.
        store.save("beta", &Rule::default()).await.unwrap();
        assert!(flags.enabled(BETA, &ada));
        flags.refresh().await.unwrap();
        assert!(!flags.enabled(BETA, &ada));
    }

    #[tokio::test]
    async fn a_bad_key_and_a_missing_store_are_refused_with_help() {
        let flags = Flags::new();
        let e = flags.set("beta", Rule::everyone()).await.unwrap_err();
        assert!(e.to_string().contains("help:"), "{e}");
        let flags = Flags::with_store(Arc::new(MemoryStore::new()));
        let e = flags.set("Beta!", Rule::everyone()).await.unwrap_err();
        assert!(e.to_string().contains("help:"), "{e}");
    }

    #[tokio::test]
    async fn a_failed_refresh_keeps_the_last_rules() {
        struct Broken;
        #[async_trait::async_trait]
        impl FlagStore for Broken {
            async fn load(&self) -> Result<BTreeMap<String, Rule>, StoreError> {
                Err(StoreError::new("down"))
            }
            async fn save(&self, _: &str, _: &Rule) -> Result<(), StoreError> {
                Ok(())
            }
            async fn remove(&self, _: &str) -> Result<(), StoreError> {
                Ok(())
            }
        }
        let flags = Flags::with_store(Arc::new(Broken));
        flags.set("beta", Rule::everyone()).await.unwrap();
        assert!(flags.refresh().await.is_err());
        assert!(flags.enabled(BETA, &Actor::anonymous()));
        assert_eq!(flags.last_error().as_deref(), Some("down"));
    }
}
