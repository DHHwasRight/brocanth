//! The storage contract, which is the whole public surface of this crate.
//!
//! §8 of the plan says a capability is a crate, the core depends on none of
//! them, and a plugin that needs another depends on it in the open. So there is
//! one trait here, [`LocalDisk`] implements it, and everything above — an
//! upload route, a download route, an admin screen — talks to the trait.
//!
//! # Object stores sit below the trait
//!
//! S3 and Google Cloud Storage arrived as the trait promised: nothing above
//! it changed. They implement the raw [`Bucket`](crate::bucket::Bucket)
//! contract, and [`ObjectStorage`](crate::ObjectStorage) turns any bucket
//! into a `Storage`, so the address, dedupe and variant rules below exist
//! once (`docs/object-storage.md`, ADR 13). The trait was shaped for it:
//!
//! * Every method is `&self`, so an implementation holds a client and a pool.
//! * Nothing returns a `Path`, a `File` or anything else only a filesystem
//!   has.
//! * `delete` is idempotent, which a remote store gives for free and a local
//!   one has to be told.
//! * Derived variants are stored through the trait
//!   ([`Storage::put_variant`]), so the cache lives wherever the objects do
//!   rather than on a disk the next instance does not have.
//!
//! # Object safety
//!
//! [`Storage`] is object-safe: no generic method, no `Self` in a return
//! position, and `async_trait` boxes the futures. `Arc<dyn Storage>` is the
//! expected way to hold one, and
//! `variants::derive_variant` takes `&dyn Storage` to prove it — that
//! function is behind the `variants` feature, which is why this paragraph
//! does not link to it.
//!
//! # What this trait does not do, and why
//!
//! * **No streaming.** Bodies are `&[u8]` and `Vec<u8>`. The HTTP half reads
//!   a whole upload under its own limit before it calls the store, so a
//!   streaming method here would stream nothing today. Code that moves large
//!   files uses [`Bucket`](crate::bucket::Bucket), which streams both ways.
//!   A range read is here ([`Storage::get_range`]).
//! * **No listing.** An enumeration endpoint over a content-addressed store is
//!   an enumeration endpoint over everything anybody uploaded. The application
//!   lists its own records; this crate does not know which rows exist.
//! * **No copy or rename.** Under content addressing a copy is the same
//!   address and a rename is a metadata edit on a record.
//!
//! [`LocalDisk`]: crate::local::LocalDisk

use async_trait::async_trait;

use std::time::Duration;

use crate::bucket::ByteRange;
use crate::error::StorageError;
use crate::key::ObjectKey;
use crate::meta::{ObjectInfo, ObjectMeta};
use crate::spec::VariantSpec;

/// Everything the rest of the system needs from storage.
///
/// Implementations must be safe to call concurrently; every method takes
/// `&self` for the same reason a connection pool does.
#[async_trait]
pub trait Storage: Send + Sync + 'static {
    /// Stores the bytes at the address the backend computes for them, and
    /// answers that address.
    ///
    /// # What a second put of the same bytes does
    ///
    /// Nothing, and it answers the same address. That is the whole of dedupe,
    /// and it also makes `put` idempotent under a retry and safe under two
    /// concurrent uploads of one file.
    ///
    /// It also means **the stored metadata belongs to the first upload, not
    /// the latest.** That is deliberate: if the second uploader could rewrite
    /// the media type, then uploading a copy of a file somebody else already
    /// stored would let an attacker change what that person's file is served
    /// as. Stored XSS for the price of a duplicate upload. The filename a
    /// *record* should display belongs on the record.
    ///
    /// # What it believes about [`ObjectInfo`]
    ///
    /// The media type is taken from the content when the content is a
    /// recognisable image, and from `declared_type` only when it is not. A
    /// client that labels a PNG `text/html` is labelling the response it wants
    /// the origin to serve back to the next visitor. Where neither answers,
    /// the type is `application/octet-stream`.
    ///
    /// # Errors
    /// [`StorageError::Backend`] when the backend could not store the bytes.
    async fn put(&self, content: &[u8], about: &ObjectInfo) -> Result<ObjectKey, StorageError>;

    /// Stores the bytes at an address the *caller* chose — in practice
    /// [`ObjectKey::random`], when dedupe is the wrong behaviour.
    ///
    /// Also a no-op when something is already stored there, for the reason
    /// above. Replacing the content under an existing address is not
    /// expressible: `delete` then `put_at`.
    ///
    /// # Errors
    /// [`StorageError::Backend`] when the backend could not store the bytes.
    async fn put_at(
        &self,
        key: &ObjectKey,
        content: &[u8],
        about: &ObjectInfo,
    ) -> Result<(), StorageError>;

    /// Reads an object whole.
    ///
    /// # Errors
    /// [`StorageError::NotFound`] when nothing is stored there.
    async fn get(&self, key: &ObjectKey) -> Result<Vec<u8>, StorageError>;

    /// The size, media type and original filename, without reading the
    /// content.
    ///
    /// This is what a `HEAD` answers and what a `GET` needs before it starts
    /// writing a body.
    ///
    /// # Errors
    /// [`StorageError::NotFound`] when nothing is stored there.
    async fn meta(&self, key: &ObjectKey) -> Result<ObjectMeta, StorageError>;

    /// Whether anything is stored there.
    ///
    /// Treat the answer as privileged. Under plain content addressing this is
    /// an oracle: anybody holding a file can compute its address and learn
    /// whether somebody uploaded it. Keyed addressing
    /// ([`ObjectKey::keyed`]) closes that, and until it is turned on an
    /// unauthenticated route must not expose this.
    ///
    /// # Errors
    /// [`StorageError::Backend`] when the backend could not answer.
    async fn exists(&self, key: &ObjectKey) -> Result<bool, StorageError>;

    /// Removes an object and every variant derived from it.
    ///
    /// **This removes the bytes that every holder of the address shares.**
    /// Content addressing deduplicates, this crate has no database, and
    /// therefore it cannot count references. An application that attaches one
    /// upload to two records either counts them itself or stores with
    /// [`ObjectKey::random`], where the addresses differ and so does the
    /// deletion.
    ///
    /// Idempotent: deleting what is not there succeeds. A delete that is
    /// retried after a timeout must not fail the second time.
    ///
    /// # Errors
    /// [`StorageError::Backend`] when the backend could not remove it.
    async fn delete(&self, key: &ObjectKey) -> Result<(), StorageError>;

    /// Reads a cached derived variant, or `None` if it has not been computed.
    ///
    /// A miss is ordinary control flow, not an error, which is why this one
    /// answers `Option` where [`Storage::get`] answers
    /// [`StorageError::NotFound`].
    ///
    /// Present with the `variants` feature off, and that is on purpose: moving
    /// derived bytes needs no codec, so a build with no image library can
    /// still serve a variant another process computed.
    ///
    /// # Errors
    /// [`StorageError::Backend`] when the backend could not answer.
    async fn get_variant(
        &self,
        key: &ObjectKey,
        spec: VariantSpec,
    ) -> Result<Option<Vec<u8>>, StorageError>;

    /// Caches a derived variant.
    ///
    /// Writing the same variant twice must be safe and must never expose a
    /// half-written one: two requests for an uncached thumbnail arrive
    /// together far more often than not.
    ///
    /// # Errors
    /// [`StorageError::Backend`] when the backend could not store it.
    async fn put_variant(
        &self,
        key: &ObjectKey,
        spec: VariantSpec,
        content: &[u8],
    ) -> Result<(), StorageError>;

    /// Reads the part of an object `range` covers. A range that ends past
    /// the object is cut at its end.
    ///
    /// The default reads the whole object and slices it. A backend that can
    /// read a range on its own overrides it: [`ObjectStorage`] does.
    ///
    /// [`ObjectStorage`]: crate::ObjectStorage
    ///
    /// # Errors
    /// [`StorageError::NotFound`] when nothing is stored there,
    /// [`StorageError::InvalidRange`] when the range starts at or after the
    /// end.
    async fn get_range(&self, key: &ObjectKey, range: ByteRange) -> Result<Vec<u8>, StorageError> {
        range.slice(&self.get(key).await?)
    }

    /// A URL that reads the object for `ttl` with no other credential, or
    /// `None` when this backend cannot sign one. A disk and Postgres cannot;
    /// S3 and GCS can.
    ///
    /// The URL is a bearer credential, like a [`FileToken`]: mint it only
    /// after the policy allowed the read. It reaches the bucket directly, so
    /// the bucket answers it, not the download route. The rule of
    /// [`SignedGet::for_object`] is signed into every URL: a sniffed image
    /// comes back inline as its type, anything else as an attachment of
    /// type `application/octet-stream` (`docs/object-storage.md` D29, D31).
    /// `nosniff` and the sandbox policy cannot travel in a URL; §9.4 of that
    /// document says why the rule covers them.
    ///
    /// [`FileToken`]: crate::FileToken
    /// [`SignedGet::for_object`]: crate::SignedGet::for_object
    ///
    /// # Errors
    /// [`StorageError::NotFound`] when nothing is stored there, and
    /// [`StorageError::Backend`] when the backend can sign and failed to.
    async fn signed_url(
        &self,
        key: &ObjectKey,
        ttl: Duration,
    ) -> Result<Option<String>, StorageError> {
        let _ = (key, ttl);
        Ok(None)
    }

    /// Whether [`Storage::signed_url`] answers a URL, known without a
    /// request. A setting that needs a signed URL reads it at start, so a
    /// disk or Postgres store fails there and not at the first download
    /// (`docs/object-storage.md` D33).
    fn can_sign(&self) -> bool {
        false
    }

    /// Signs one URL for an address nothing is stored at, and answers the
    /// origin it points at: `https://bucket.s3.eu-west-1.amazonaws.com`.
    /// `None` when this backend cannot sign.
    ///
    /// The start check of a redirecting download route calls it: a key
    /// that cannot sign fails there, and the origin is what the
    /// Content-Security-Policy `img-src` must allow (D34).
    ///
    /// # Errors
    /// [`StorageError::Backend`] when the backend can sign and failed to.
    async fn signing_origin(&self) -> Result<Option<String>, StorageError> {
        Ok(None)
    }
}

#[async_trait]
impl<T: Storage + ?Sized> Storage for std::sync::Arc<T> {
    async fn put(&self, content: &[u8], about: &ObjectInfo) -> Result<ObjectKey, StorageError> {
        (**self).put(content, about).await
    }

    async fn put_at(
        &self,
        key: &ObjectKey,
        content: &[u8],
        about: &ObjectInfo,
    ) -> Result<(), StorageError> {
        (**self).put_at(key, content, about).await
    }

    async fn get(&self, key: &ObjectKey) -> Result<Vec<u8>, StorageError> {
        (**self).get(key).await
    }

    async fn meta(&self, key: &ObjectKey) -> Result<ObjectMeta, StorageError> {
        (**self).meta(key).await
    }

    async fn exists(&self, key: &ObjectKey) -> Result<bool, StorageError> {
        (**self).exists(key).await
    }

    async fn delete(&self, key: &ObjectKey) -> Result<(), StorageError> {
        (**self).delete(key).await
    }

    async fn get_variant(
        &self,
        key: &ObjectKey,
        spec: VariantSpec,
    ) -> Result<Option<Vec<u8>>, StorageError> {
        (**self).get_variant(key, spec).await
    }

    async fn put_variant(
        &self,
        key: &ObjectKey,
        spec: VariantSpec,
        content: &[u8],
    ) -> Result<(), StorageError> {
        (**self).put_variant(key, spec, content).await
    }

    async fn get_range(&self, key: &ObjectKey, range: ByteRange) -> Result<Vec<u8>, StorageError> {
        (**self).get_range(key, range).await
    }

    async fn signed_url(
        &self,
        key: &ObjectKey,
        ttl: Duration,
    ) -> Result<Option<String>, StorageError> {
        (**self).signed_url(key, ttl).await
    }

    fn can_sign(&self) -> bool {
        (**self).can_sign()
    }

    async fn signing_origin(&self) -> Result<Option<String>, StorageError> {
        (**self).signing_origin().await
    }
}
