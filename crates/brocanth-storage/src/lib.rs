//! File storage for brocanth: one trait, one disk backend, addresses that
//! cannot spell a path, and image variants that are absent unless asked for.
//!
//! # Shape
//!
//! [`Storage`] is the whole public contract. [`LocalDisk`] implements it, and
//! so does [`Postgres`] behind the `postgres` feature — objects in `bytea`
//! columns, for a deployment that already runs Postgres for its queues and
//! would rather not also run a shared filesystem. An S3 backend is a later,
//! separate crate that implements the same trait — PLAN §4 cuts object stores
//! from v1 by name, so there is no `s3` feature here, no unimplemented arm and
//! no AWS SDK in the lock file. Nothing above the trait changes when that
//! crate arrives.
//!
//! [`Postgres`]: postgres::Postgres
//!
//! This crate does not depend on `brocanth`. It has no routes, parses no
//! multipart body and knows nothing about HTTP. The upload route, the download
//! route and the policy that guards them are the application's, and they
//! depend on this; it depends on nothing in the workspace.
//!
//! # Security decisions, each with its reason
//!
//! * **Path traversal is impossible rather than filtered.** A path is built
//!   from an [`ObjectKey`], which holds thirty-two bytes and renders as
//!   sixty-four lowercase hex characters. `..`, `/`, `%2e%2e`, a NUL and a
//!   drive letter are not values of that type, so no filename, URL segment or
//!   header reaches outside the root. A user-supplied name is a [`Filename`]:
//!   it is metadata, it is sanitised for the `Content-Disposition` header it
//!   ends up in, and it never touches the filesystem. See [`key`].
//! * **The address is a content hash, and which hash is a deployment
//!   decision.** Plain SHA-256 deduplicates and is an existence oracle to
//!   anybody holding the file; HMAC-SHA256 under a deployment secret
//!   deduplicates and is not. The trade — dedupe against deletion semantics
//!   against disclosure — is written out in [`key`], with a third option for
//!   when dedupe itself is the leak.
//! * **The stored media type comes from the bytes, not from the client.** A
//!   client that labels a file `text/html` is choosing what the origin serves
//!   back to the next visitor. Only four raster types are recognised, and
//!   everything else is `application/octet-stream` with
//!   [`ContentType::is_safe_to_inline`] answering `false`, which is the bit
//!   the serving half needs to force an attachment.
//! * **A second upload of the same bytes never rewrites the first one's
//!   metadata.** Otherwise uploading a copy of somebody else's file is a way
//!   to change what their file is served as.
//! * **Declared dimensions are capped before an image is decoded.** A fifty
//!   kilobyte PNG can declare 40 000 by 40 000, which is 6.4 GB of RGBA
//!   allocated before anything else happens. The header is read, the size is
//!   refused, and the decoder is never reached. See [`MAX_SOURCE_PIXELS`].
//! * **A file token is checked by its MAC before any of it is believed**, in
//!   constant time, and forged, malformed and expired are one error. See
//!   [`token`].
//! * **Every write is atomic and the store is `0700`.** A reader sees a whole
//!   object or none, and uploads are not world-readable on a shared host.
//!
//! # Features
//!
//! `variants` pulls in `image` and nothing else does. With it off, a size
//! specification still parses ([`VariantSpec`]) and a cached variant is still
//! served ([`Storage::get_variant`]); only the resizing is gone. §8.2 asks
//! that a feature which is off means the code is absent, and four image
//! decoders pointed at stranger-supplied bytes is the clearest case of it in
//! this workspace.
//!
//! `postgres` pulls in `sqlx` and the reactor it runs on, which is the largest
//! tree in this crate by a wide margin. Same rule: with the feature off the
//! module does not exist, and `cargo tree -p brocanth-storage -e normal
//! --depth 1` lists the same seven crates it listed before that backend was
//! written. A build with neither feature has no SQL driver, no TLS stack, no
//! async runtime and no image codec.
//!
//! # Example
//!
//! ```
//! use brocanth_storage::{HashKey, LocalDisk, ObjectInfo, VariantSpec};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # let dir = std::env::temp_dir().join(format!("brocanth-doc-{}", std::process::id()));
//! let store = LocalDisk::open(&dir)?.with_hash_key(HashKey::from_secret(b"deployment secret"));
//!
//! let about = ObjectInfo::new()
//!     .with_filename("../../etc/passwd")
//!     .with_declared_type("text/html");
//! let key = store.put_blocking(b"\x89PNG\r\n\x1a\nnot really", &about)?;
//!
//! let meta = store.meta_blocking(&key)?;
//! assert_eq!(meta.content_type.as_str(), "image/png"); // sniffed, not declared
//! assert_eq!(meta.filename.unwrap().as_str(), "etcpasswd"); // kept, defanged
//! assert!(store.root().join("objects").join(&key.to_hex()[0..2]).exists());
//!
//! let spec: VariantSpec = "300x200t".parse()?;
//! assert_eq!(spec.to_string(), "300x200t"); // parses with `variants` off
//! # std::fs::remove_dir_all(&dir).ok();
//! # Ok(())
//! # }
//! ```
//!
//! [`ContentType::is_safe_to_inline`]: meta::ContentType::is_safe_to_inline

#![forbid(unsafe_code)]

pub mod error;
pub mod key;
pub mod local;
pub mod meta;
pub mod spec;
pub mod storage;
pub mod token;

#[cfg(feature = "postgres")]
pub mod postgres;

#[cfg(feature = "variants")]
pub mod variants;

/// Behind the `scaffold` feature, which is off: the generator is the CLI's
/// business and not an application's. See the dependency comment in
/// `Cargo.toml`.
#[cfg(feature = "scaffold")]
pub mod scaffold;

#[cfg(test)]
mod rt;

#[cfg(test)]
mod tests;

pub use error::StorageError;
pub use key::{HashKey, ObjectKey, KEY_BYTES, KEY_HEX_LEN};
pub use local::LocalDisk;
pub use meta::{ContentType, Filename, ObjectInfo, ObjectMeta, MAX_FILENAME_LEN};
pub use spec::{CropAnchor, VariantMode, VariantSpec, MAX_VARIANT_DIM};

#[cfg(feature = "postgres")]
pub use postgres::{Postgres, DEFAULT_MAX_OBJECT_BYTES, OBJECT_SIZE_CEILING};
pub use storage::Storage;
pub use token::{FileToken, FileTokenKey, FileTokens, UnixTime, DEFAULT_TTL};

#[cfg(feature = "variants")]
pub use variants::{derive_variant, render, Variant};

/// The largest width or height a *source* image may declare before this crate
/// refuses to decode it.
///
/// Checked against the image header, not against the decoded result. Lives
/// here rather than in the `variants` module so the number is documented, and
/// enforceable by a caller, in a build with no image codecs in it.
pub const MAX_SOURCE_DIM: u32 = 20_000;

/// The largest pixel count a *source* image may declare: fifty megapixels.
///
/// A fifty kilobyte PNG can declare 40 000 by 40 000, which decodes to 6.4
/// gigabytes. The declaration is refused from the header, before the decoder
/// is reached. Above fifty megapixels, serving a thumbnail is not the job the
/// application thought it was doing.
pub const MAX_SOURCE_PIXELS: u64 = 50_000_000;
