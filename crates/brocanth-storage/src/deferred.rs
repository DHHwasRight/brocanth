//! A store that is opened after the plugins that hold it are built.
//!
//! The file routes are built at `configure`, before any plugin's
//! `before_start`. A store on the `db` plugin's pool can open only after
//! that plugin's `before_start` published the pool. [`Deferred`] closes the
//! gap: the storage plugin publishes a `Deferred` at `configure`, every
//! reader holds it, and the plugin [`set`](Deferred::set)s the real store
//! at `before_start`, before the server takes a request.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;

use crate::error::StorageError;
use crate::key::ObjectKey;
use crate::meta::{ObjectInfo, ObjectMeta};
use crate::spec::VariantSpec;
use crate::storage::Storage;

/// A [`Storage`] that forwards to a store set later. Cheap to clone: every
/// clone sees the store [`Deferred::set`] gives any of them.
#[derive(Clone, Default)]
pub struct Deferred(Arc<OnceLock<Arc<dyn Storage>>>);

impl Deferred {
    /// A store with nothing behind it yet. Every call fails until
    /// [`Deferred::set`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Puts `store` behind every clone, and answers whether it was the
    /// first. A second call changes nothing.
    pub fn set(&self, store: Arc<dyn Storage>) -> bool {
        self.0.set(store).is_ok()
    }

    fn store(&self) -> Result<&Arc<dyn Storage>, StorageError> {
        self.0.get().ok_or_else(|| {
            StorageError::backend(
                "the file store is not open yet\n\
                 help: the storage plugin opens it at `before_start`; call it from a request, \
                 not from an earlier startup stage",
            )
        })
    }
}

impl std::fmt::Debug for Deferred {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Deferred")
            .field("open", &self.0.get().is_some())
            .finish()
    }
}

#[async_trait]
impl Storage for Deferred {
    async fn put(&self, content: &[u8], about: &ObjectInfo) -> Result<ObjectKey, StorageError> {
        self.store()?.put(content, about).await
    }

    async fn put_at(
        &self,
        key: &ObjectKey,
        content: &[u8],
        about: &ObjectInfo,
    ) -> Result<(), StorageError> {
        self.store()?.put_at(key, content, about).await
    }

    async fn get(&self, key: &ObjectKey) -> Result<Vec<u8>, StorageError> {
        self.store()?.get(key).await
    }

    async fn meta(&self, key: &ObjectKey) -> Result<ObjectMeta, StorageError> {
        self.store()?.meta(key).await
    }

    async fn exists(&self, key: &ObjectKey) -> Result<bool, StorageError> {
        self.store()?.exists(key).await
    }

    async fn delete(&self, key: &ObjectKey) -> Result<(), StorageError> {
        self.store()?.delete(key).await
    }

    async fn get_variant(
        &self,
        key: &ObjectKey,
        spec: VariantSpec,
    ) -> Result<Option<Vec<u8>>, StorageError> {
        self.store()?.get_variant(key, spec).await
    }

    async fn put_variant(
        &self,
        key: &ObjectKey,
        spec: VariantSpec,
        content: &[u8],
    ) -> Result<(), StorageError> {
        self.store()?.put_variant(key, spec, content).await
    }

    async fn get_range(
        &self,
        key: &ObjectKey,
        range: crate::bucket::ByteRange,
    ) -> Result<Vec<u8>, StorageError> {
        self.store()?.get_range(key, range).await
    }

    async fn signed_url(
        &self,
        key: &ObjectKey,
        ttl: std::time::Duration,
    ) -> Result<Option<String>, StorageError> {
        self.store()?.signed_url(key, ttl).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::LocalDisk;
    use crate::rt::block_on;

    #[test]
    fn a_deferred_store_fails_until_it_is_set_and_then_forwards() {
        let deferred = Deferred::new();
        let reader = deferred.clone();
        assert!(matches!(
            block_on(reader.get(&ObjectKey::of(b"x"))),
            Err(StorageError::Backend(why)) if why.contains("help:")
        ));
        let dir = tempfile::tempdir().unwrap();
        assert!(deferred.set(Arc::new(LocalDisk::open(dir.path()).unwrap())));
        let key = block_on(reader.put(b"bytes", &ObjectInfo::new())).unwrap();
        assert_eq!(block_on(reader.get(&key)).unwrap(), b"bytes");
        assert!(!deferred.set(Arc::new(LocalDisk::open(dir.path()).unwrap())));
    }
}
