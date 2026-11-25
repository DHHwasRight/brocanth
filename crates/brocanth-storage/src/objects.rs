//! [`Storage`] over any [`Bucket`]: the content-addressed rules, once.
//!
//! A cloud backend implements [`Bucket`], which knows nothing about
//! addresses, sniffing or variants. [`ObjectStorage`] adds them, with
//! [`LocalDisk`](crate::LocalDisk)'s layout:
//!
//! ```text
//! objects/ab/cd/<hex>             the bytes; media type and filename as metadata
//! variants/ab/cd/<hex>/<spec>     a derived image
//! ```
//!
//! * **The address.** SHA-256, or HMAC-SHA256 under the deployment's
//!   [`HashKey`], exactly as `LocalDisk` computes it.
//! * **The first upload owns the metadata.** A put that finds the object
//!   there does nothing, and the write itself is create-only
//!   ([`PutOptions::create_only`]), so two instances storing one file at once
//!   cannot let the second rewrite the first's media type.
//! * **The media type comes from the bytes**, through
//!   [`ContentType::resolve`], as on every backend.
//! * **A delete takes the variants first**, listed under the object's
//!   prefix: a thumbnail of a deleted document is still the document. A
//!   variant written while the delete ran is taken back, as `LocalDisk` and
//!   the Postgres foreign key do.
//! * **A size limit**, [`DEFAULT_MAX_OBJECT_BYTES`] unless set, checked
//!   before a byte leaves the process.
//!
//! The filename is stored as the metadata entry `filename`. On a disk that
//! is the `filename:` line `LocalDisk` writes, so the two read each other's
//! objects.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::bucket::{Bucket, ByteRange, ObjectPath, PutOptions, SignedMethod};
use crate::error::StorageError;
use crate::key::{HashKey, ObjectKey};
use crate::meta::{ContentType, Filename, ObjectInfo, ObjectMeta};
use crate::spec::VariantSpec;
use crate::storage::Storage;

/// The largest object a new [`ObjectStorage`] stores: sixty-four mebibytes.
///
/// The trait hands the whole object over as a slice, so this is memory at
/// both ends of every concurrent request. Raise it with
/// [`ObjectStorage::with_max_object_bytes`]; a file larger than a few
/// hundred mebibytes wants [`Bucket::put`] with a stream instead.
pub const DEFAULT_MAX_OBJECT_BYTES: u64 = 64 * 1024 * 1024;

const FILENAME: &str = "filename";

/// [`Storage`] on a [`Bucket`]. Cheap to clone: a clone is the same store.
#[derive(Clone)]
pub struct ObjectStorage {
    bucket: Arc<dyn Bucket>,
    hash_key: Option<HashKey>,
    max_object_bytes: u64,
}

impl ObjectStorage {
    /// A store on `bucket`, with plain SHA-256 addresses and the default
    /// limit.
    pub fn new(bucket: Arc<dyn Bucket>) -> Self {
        Self {
            bucket,
            hash_key: None,
            max_object_bytes: DEFAULT_MAX_OBJECT_BYTES,
        }
    }

    /// Addresses objects with HMAC-SHA256 under `key`, as
    /// [`LocalDisk::with_hash_key`](crate::LocalDisk::with_hash_key) does.
    #[must_use]
    pub fn with_hash_key(mut self, key: HashKey) -> Self {
        self.hash_key = Some(key);
        self
    }

    /// Stores objects of at most `limit` bytes.
    #[must_use]
    pub fn with_max_object_bytes(mut self, limit: u64) -> Self {
        self.max_object_bytes = limit;
        self
    }

    /// The bucket below.
    pub fn bucket(&self) -> &Arc<dyn Bucket> {
        &self.bucket
    }

    /// The address these bytes get in this store, without storing them.
    pub fn address_of(&self, content: &[u8]) -> ObjectKey {
        match &self.hash_key {
            Some(key) => ObjectKey::keyed(key, content),
            None => ObjectKey::of(content),
        }
    }

    /// The start-time check of the bucket below. See [`Bucket::probe`].
    ///
    /// # Errors
    /// What the bucket answered, with a `help:` line.
    pub async fn probe(&self) -> Result<(), StorageError> {
        self.bucket.probe().await
    }
}

/// `objects/ab/cd/<hex>`.
pub(crate) fn object_path(key: &ObjectKey) -> ObjectPath {
    let hex = key.to_hex();
    ObjectPath::parse(&format!("objects/{}/{}/{hex}", &hex[0..2], &hex[2..4]))
        .expect("hex is a valid path")
}

/// `variants/ab/cd/<hex>/`, the prefix of every variant of one object.
fn variant_prefix(key: &ObjectKey) -> String {
    let hex = key.to_hex();
    format!("variants/{}/{}/{hex}/", &hex[0..2], &hex[2..4])
}

fn variant_path(key: &ObjectKey, spec: VariantSpec) -> ObjectPath {
    ObjectPath::parse(&format!("{}{spec}", variant_prefix(key)))
        .expect("hex and a variant spec are a valid path")
}

impl ObjectStorage {
    async fn delete_variants(&self, key: &ObjectKey) -> Result<(), StorageError> {
        for listed in self.bucket.list(&variant_prefix(key)).await? {
            self.bucket.delete(&listed.path).await?;
        }
        Ok(())
    }
}

#[async_trait]
impl Storage for ObjectStorage {
    async fn put(&self, content: &[u8], about: &ObjectInfo) -> Result<ObjectKey, StorageError> {
        let key = self.address_of(content);
        self.put_at(&key, content, about).await?;
        Ok(key)
    }

    async fn put_at(
        &self,
        key: &ObjectKey,
        content: &[u8],
        about: &ObjectInfo,
    ) -> Result<(), StorageError> {
        let size = content.len() as u64;
        if size > self.max_object_bytes {
            return Err(StorageError::TooLarge {
                size,
                limit: self.max_object_bytes,
            });
        }
        let path = object_path(key);
        // Already there: same address, same bytes, and the first upload's
        // metadata. A `head` is cheaper than sending the bytes to be refused.
        match self.bucket.head(&path).await {
            Ok(_) => return Ok(()),
            Err(StorageError::NotFound) => {}
            Err(e) => return Err(e),
        }
        let content_type = ContentType::resolve(content, about.declared_type.as_ref());
        let mut options = PutOptions::new()
            .with_content_type(content_type.as_str())
            .with_max_bytes(self.max_object_bytes)
            .create_only();
        if let Some(name) = &about.filename {
            options = options.with_metadata(FILENAME, name.as_str());
        }
        match self.bucket.put(&path, content.into(), options).await {
            Ok(_) | Err(StorageError::Exists) => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn get(&self, key: &ObjectKey) -> Result<Vec<u8>, StorageError> {
        self.bucket
            .get(&object_path(key), None)
            .await?
            .bytes()
            .await
    }

    async fn get_range(&self, key: &ObjectKey, range: ByteRange) -> Result<Vec<u8>, StorageError> {
        self.bucket
            .get(&object_path(key), Some(range))
            .await?
            .bytes()
            .await
    }

    async fn meta(&self, key: &ObjectKey) -> Result<ObjectMeta, StorageError> {
        let head = self.bucket.head(&object_path(key)).await?;
        Ok(ObjectMeta {
            key: *key,
            size: head.size,
            content_type: head
                .content_type
                .as_deref()
                .and_then(ContentType::parse)
                .unwrap_or_else(ContentType::octet_stream),
            filename: head
                .metadata
                .get(FILENAME)
                .and_then(|name| Filename::sanitise(name)),
        })
    }

    async fn exists(&self, key: &ObjectKey) -> Result<bool, StorageError> {
        match self.bucket.head(&object_path(key)).await {
            Ok(_) => Ok(true),
            Err(StorageError::NotFound) => Ok(false),
            Err(e) => Err(e),
        }
    }

    async fn delete(&self, key: &ObjectKey) -> Result<(), StorageError> {
        // Variants first; see the module documentation.
        self.delete_variants(key).await?;
        self.bucket.delete(&object_path(key)).await
    }

    async fn get_variant(
        &self,
        key: &ObjectKey,
        spec: VariantSpec,
    ) -> Result<Option<Vec<u8>>, StorageError> {
        match self.bucket.get(&variant_path(key, spec), None).await {
            Ok(object) => Ok(Some(object.bytes().await?)),
            Err(StorageError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn put_variant(
        &self,
        key: &ObjectKey,
        spec: VariantSpec,
        content: &[u8],
    ) -> Result<(), StorageError> {
        // A variant of an object that is gone is the object again.
        if !self.exists(key).await? {
            return Err(StorageError::NotFound);
        }
        let options = match ContentType::sniff(content) {
            Some(ct) => PutOptions::new().with_content_type(ct.as_str()),
            None => PutOptions::new(),
        };
        self.bucket
            .put(&variant_path(key, spec), content.into(), options)
            .await?;
        // A delete that ran during the write removed the variants before
        // this one landed. Look again, and take it back.
        if !self.exists(key).await? {
            self.delete_variants(key).await?;
            return Err(StorageError::NotFound);
        }
        Ok(())
    }

    async fn signed_url(
        &self,
        key: &ObjectKey,
        ttl: Duration,
    ) -> Result<Option<String>, StorageError> {
        self.bucket
            .signed_url(SignedMethod::Get, &object_path(key), ttl)
            .await
    }
}
