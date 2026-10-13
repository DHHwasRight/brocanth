//! The one shipped backend: a directory on a disk.
//!
//! # Layout
//!
//! ```text
//! <root>/objects/ab/cd/abcd…               the bytes
//! <root>/objects/ab/cd/abcd….meta          media type and original filename
//! <root>/variants/ab/cd/abcd…/300x200t     a derived image
//! <root>/tmp/<random>                      never served, see below
//! ```
//!
//! Two levels of two hex characters, because a hundred thousand objects in one
//! directory is a directory a human cannot `ls` and some filesystems cannot
//! index. Two levels give 65 536 buckets, which is enough for several million
//! objects at a few dozen entries each.
//!
//! Every path component above is drawn from `0-9a-f` or from
//! [`VariantSpec`]'s canonical spelling. Nothing a client sent appears in any
//! of them. That is the whole traversal defence and it is structural: see
//! [`crate::key`].
//!
//! # Every write is atomic
//!
//! Write to `<root>/tmp/<random>`, `fsync`, then `rename` onto the final path.
//! `rename` within a filesystem is atomic, so a reader sees the whole object
//! or no object, never a prefix — which matters most for a variant, where two
//! requests for an uncached thumbnail routinely arrive together and would
//! otherwise both write the same file while a third reads it. The `fsync` is
//! for the crash case: `rename` makes the name visible, it does not make the
//! content durable, and a power cut between the two leaves a zero-length file
//! under a content address, which is the one corruption content addressing
//! cannot self-heal from.
//!
//! The temporary directory is inside the root so the rename never crosses a
//! filesystem, and it is never served because nothing reachable through
//! [`Storage`] builds a path into it.
//!
//! # Permissions
//!
//! On Unix the root is `0700` and every file `0600`. A protected file is only
//! as protected as the directory it sits in, and the default umask on a shared
//! host leaves uploads world-readable.
//!
//! # Blocking
//!
//! These are `std::fs` calls. The `_blocking` methods are the real
//! implementation and the [`Storage`] impl calls them; the futures never
//! yield. On a page-cache hit that is microseconds and costs an async runtime
//! nothing; on a cold read of a large object it is a blocking syscall on a
//! reactor thread, so an application on tokio wraps the call in
//! `spawn_blocking` — which is why the synchronous methods are public rather
//! than private. A CLI, a migration and a test use them directly and need no
//! runtime at all.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::error::StorageError;
use crate::key::{HashKey, ObjectKey};
use crate::meta::{ContentType, Filename, ObjectInfo, ObjectMeta};
use crate::spec::VariantSpec;
use crate::storage::Storage;

const OBJECTS: &str = "objects";
const VARIANTS: &str = "variants";
const TMP: &str = "tmp";
const META_SUFFIX: &str = ".meta";

/// Objects in a directory tree.
pub struct LocalDisk {
    root: PathBuf,
    hash_key: Option<HashKey>,
}

impl LocalDisk {
    /// Opens, and creates if needed, a store rooted at `root`.
    ///
    /// Addresses are plain SHA-256 content addresses. For user uploads prefer
    /// [`LocalDisk::with_hash_key`], which closes the existence oracle
    /// described in [`crate::key`].
    ///
    /// # Errors
    /// [`StorageError::Backend`] when the directories cannot be created or the
    /// root cannot be resolved.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, StorageError> {
        let root = root.as_ref();
        fs::create_dir_all(root).map_err(|e| StorageError::io("create the storage root", e))?;
        // Canonicalised once, here, so the stored root carries no `..`, no
        // symlink and no relative prefix. Every later path is this one plus
        // hex, and a process that changes its working directory does not move
        // the store out from under itself.
        let root =
            fs::canonicalize(root).map_err(|e| StorageError::io("resolve the storage root", e))?;
        restrict(&root)?;
        for sub in [OBJECTS, VARIANTS, TMP] {
            let dir = root.join(sub);
            fs::create_dir_all(&dir)
                .map_err(|e| StorageError::io("create a storage directory", e))?;
            restrict(&dir)?;
        }
        Ok(Self {
            root,
            hash_key: None,
        })
    }

    /// Addresses objects with HMAC-SHA256 under `key` instead of a plain
    /// hash.
    ///
    /// Dedupe is unchanged. What changes is that an attacker holding a copy of
    /// a file can no longer compute the address it is stored at, so neither a
    /// guessed URL nor [`Storage::exists`] tells them whether somebody
    /// uploaded it.
    #[must_use]
    pub fn with_hash_key(mut self, key: HashKey) -> Self {
        self.hash_key = Some(key);
        self
    }

    /// The resolved root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The address these bytes get in this store, without storing them.
    pub fn address_of(&self, content: &[u8]) -> ObjectKey {
        match &self.hash_key {
            Some(key) => ObjectKey::keyed(key, content),
            None => ObjectKey::of(content),
        }
    }

    // -- the synchronous implementation ------------------------------------

    /// [`Storage::put`], synchronously.
    ///
    /// # Errors
    /// See [`Storage::put`].
    pub fn put_blocking(
        &self,
        content: &[u8],
        about: &ObjectInfo,
    ) -> Result<ObjectKey, StorageError> {
        let key = self.address_of(content);
        self.put_at_blocking(&key, content, about)?;
        Ok(key)
    }

    /// [`Storage::put_at`], synchronously.
    ///
    /// # Errors
    /// See [`Storage::put_at`].
    pub fn put_at_blocking(
        &self,
        key: &ObjectKey,
        content: &[u8],
        about: &ObjectInfo,
    ) -> Result<(), StorageError> {
        let path = self.object_path(key);
        // Already there: same address, same bytes. Returning here is what
        // makes a second upload free, and it is also what stops the second
        // uploader rewriting the first one's media type.
        if path.exists() {
            return Ok(());
        }
        let content_type = resolve_type(content, about);
        self.write_atomic(&path, content)?;
        self.write_atomic(
            &self.meta_path(key),
            sidecar(&content_type, about.filename.as_ref()).as_bytes(),
        )?;
        Ok(())
    }

    /// [`Storage::get`], synchronously.
    ///
    /// # Errors
    /// See [`Storage::get`].
    pub fn get_blocking(&self, key: &ObjectKey) -> Result<Vec<u8>, StorageError> {
        fs::read(self.object_path(key)).map_err(|e| StorageError::io("read an object", e))
    }

    /// [`Storage::meta`], synchronously.
    ///
    /// # Errors
    /// See [`Storage::meta`].
    pub fn meta_blocking(&self, key: &ObjectKey) -> Result<ObjectMeta, StorageError> {
        let size = fs::metadata(self.object_path(key))
            .map_err(|e| StorageError::io("read an object's size", e))?
            .len();
        // A missing or unreadable sidecar is not a missing object: the bytes
        // are what matter and `application/octet-stream` is always a safe
        // answer.
        let (content_type, filename) = match fs::read_to_string(self.meta_path(key)) {
            Ok(text) => parse_sidecar(&text),
            Err(_) => (ContentType::octet_stream(), None),
        };
        Ok(ObjectMeta {
            key: *key,
            size,
            content_type,
            filename,
        })
    }

    /// [`Storage::exists`], synchronously.
    ///
    /// # Errors
    /// See [`Storage::exists`].
    pub fn exists_blocking(&self, key: &ObjectKey) -> Result<bool, StorageError> {
        self.object_path(key)
            .try_exists()
            .map_err(|e| StorageError::io("look for an object", e))
    }

    /// [`Storage::delete`], synchronously.
    ///
    /// # Errors
    /// See [`Storage::delete`].
    pub fn delete_blocking(&self, key: &ObjectKey) -> Result<(), StorageError> {
        // Variants first. The other order leaves derived images of an object
        // that is gone, which is a disclosure: a thumbnail of a deleted
        // document is still the document.
        remove_dir_if_present(&self.variant_dir(key), "remove an object's variants")?;
        remove_if_present(&self.meta_path(key), "remove an object's metadata")?;
        remove_if_present(&self.object_path(key), "remove an object")
    }

    /// [`Storage::get_variant`], synchronously.
    ///
    /// # Errors
    /// See [`Storage::get_variant`].
    pub fn get_variant_blocking(
        &self,
        key: &ObjectKey,
        spec: VariantSpec,
    ) -> Result<Option<Vec<u8>>, StorageError> {
        match fs::read(self.variant_path(key, spec)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(StorageError::io("read a variant", e)),
        }
    }

    /// [`Storage::put_variant`], synchronously.
    ///
    /// # Errors
    /// See [`Storage::put_variant`].
    pub fn put_variant_blocking(
        &self,
        key: &ObjectKey,
        spec: VariantSpec,
        content: &[u8],
    ) -> Result<(), StorageError> {
        self.write_atomic(&self.variant_path(key, spec), content)
    }

    // -- paths, built from nothing a client supplied -----------------------

    fn object_path(&self, key: &ObjectKey) -> PathBuf {
        let hex = key.to_hex();
        self.root
            .join(OBJECTS)
            .join(&hex[0..2])
            .join(&hex[2..4])
            .join(&hex)
    }

    fn meta_path(&self, key: &ObjectKey) -> PathBuf {
        let mut path = self.object_path(key).into_os_string();
        path.push(META_SUFFIX);
        PathBuf::from(path)
    }

    fn variant_dir(&self, key: &ObjectKey) -> PathBuf {
        let hex = key.to_hex();
        self.root
            .join(VARIANTS)
            .join(&hex[0..2])
            .join(&hex[2..4])
            .join(&hex)
    }

    fn variant_path(&self, key: &ObjectKey, spec: VariantSpec) -> PathBuf {
        self.variant_dir(key).join(spec.to_string())
    }

    fn write_atomic(&self, dest: &Path, content: &[u8]) -> Result<(), StorageError> {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| StorageError::io("create an object directory", e))?;
            restrict(parent)?;
        }
        let tmp = self.root.join(TMP).join(temp_name()?);
        let result = (|| -> Result<(), StorageError> {
            let mut file = fs::File::create(&tmp)
                .map_err(|e| StorageError::io("create a temporary file", e))?;
            restrict_file(&file)?;
            file.write_all(content)
                .map_err(|e| StorageError::io("write a temporary file", e))?;
            // See the module documentation: the rename publishes the name, the
            // sync publishes the bytes, and only this order survives a crash.
            file.sync_all()
                .map_err(|e| StorageError::io("flush a temporary file", e))?;
            drop(file);
            fs::rename(&tmp, dest).map_err(|e| StorageError::io("publish an object", e))
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }
}

#[async_trait]
impl Storage for LocalDisk {
    async fn put(&self, content: &[u8], about: &ObjectInfo) -> Result<ObjectKey, StorageError> {
        self.put_blocking(content, about)
    }

    async fn put_at(
        &self,
        key: &ObjectKey,
        content: &[u8],
        about: &ObjectInfo,
    ) -> Result<(), StorageError> {
        self.put_at_blocking(key, content, about)
    }

    async fn get(&self, key: &ObjectKey) -> Result<Vec<u8>, StorageError> {
        self.get_blocking(key)
    }

    async fn meta(&self, key: &ObjectKey) -> Result<ObjectMeta, StorageError> {
        self.meta_blocking(key)
    }

    async fn exists(&self, key: &ObjectKey) -> Result<bool, StorageError> {
        self.exists_blocking(key)
    }

    async fn delete(&self, key: &ObjectKey) -> Result<(), StorageError> {
        self.delete_blocking(key)
    }

    async fn get_variant(
        &self,
        key: &ObjectKey,
        spec: VariantSpec,
    ) -> Result<Option<Vec<u8>>, StorageError> {
        self.get_variant_blocking(key, spec)
    }

    async fn put_variant(
        &self,
        key: &ObjectKey,
        spec: VariantSpec,
        content: &[u8],
    ) -> Result<(), StorageError> {
        self.put_variant_blocking(key, spec, content)
    }
}

/// The media type to store: what the bytes are, then what the client said,
/// then nothing. See [`Storage::put`].
fn resolve_type(content: &[u8], about: &ObjectInfo) -> ContentType {
    ContentType::sniff(content)
        .or_else(|| about.declared_type.clone())
        .unwrap_or_else(ContentType::octet_stream)
}

/// The sidecar. A two-line text file, not JSON, because adding a JSON parser
/// to a storage plugin to hold two strings is the kind of dependency §8 is
/// about. Both values are already free of control characters by construction,
/// so a line is a safe frame.
fn sidecar(content_type: &ContentType, filename: Option<&Filename>) -> String {
    let mut out = format!("brocanth-storage/1\ncontent-type: {content_type}\n");
    if let Some(name) = filename {
        out.push_str(&format!("filename: {name}\n"));
    }
    out
}

fn parse_sidecar(text: &str) -> (ContentType, Option<Filename>) {
    let mut content_type = None;
    let mut filename = None;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("content-type: ") {
            content_type = ContentType::parse(value);
        } else if let Some(value) = line.strip_prefix("filename: ") {
            filename = Filename::sanitise(value);
        }
    }
    (
        content_type.unwrap_or_else(ContentType::octet_stream),
        filename,
    )
}

fn temp_name() -> Result<String, StorageError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|e| StorageError::backend(format!("no OS randomness: {e}")))?;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut name = String::with_capacity(32);
    for b in bytes {
        name.push(HEX[(b >> 4) as usize] as char);
        name.push(HEX[(b & 0x0f) as usize] as char);
    }
    Ok(name)
}

fn remove_if_present(path: &Path, context: &'static str) -> Result<(), StorageError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(StorageError::backend(format!("{context}: {e}"))),
    }
}

fn remove_dir_if_present(path: &Path, context: &'static str) -> Result<(), StorageError> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(StorageError::backend(format!("{context}: {e}"))),
    }
}

#[cfg(unix)]
fn restrict(dir: &Path) -> Result<(), StorageError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
        .map_err(|e| StorageError::io("restrict a storage directory", e))
}

#[cfg(not(unix))]
fn restrict(_dir: &Path) -> Result<(), StorageError> {
    Ok(())
}

#[cfg(unix)]
fn restrict_file(file: &fs::File) -> Result<(), StorageError> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|e| StorageError::io("restrict a stored file", e))
}

#[cfg(not(unix))]
fn restrict_file(_file: &fs::File) -> Result<(), StorageError> {
    Ok(())
}
