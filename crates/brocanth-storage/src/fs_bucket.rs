//! A [`Bucket`] in a directory: the disk backend of the raw contract.
//!
//! The same discipline as [`LocalDisk`](crate::LocalDisk), and the same
//! files: an object at `<root>/<path>`, its media type and metadata in a
//! sidecar at `<root>/<path>.meta` in `LocalDisk`'s format, temporary files
//! in `<root>/tmp`. So [`ObjectStorage`](crate::ObjectStorage) over an
//! `FsBucket` and a `LocalDisk` on one root read each other's objects, and a
//! project can move between them without moving a file.
//!
//! * Every write goes to `<root>/tmp`, is `fsync`ed, then renamed (or, for a
//!   create-only put, linked) onto its path: a reader sees the whole object
//!   or none.
//! * The root is `0700` and every file `0600` on Unix, and the root holds a
//!   `.gitignore` of `*`.
//! * The disk calls run on Tokio's blocking pool inside a runtime, in place
//!   outside one.
//!
//! Three names are reserved and refused as paths: a first segment `tmp`, a
//! last segment ending `.meta`, and `.gitignore` at the root. A list never
//! answers them.
//!
//! A get reads the bytes it covers in one call and answers them as one
//! chunk. Ask for a range to bound the memory a read takes. A disk cannot
//! sign a URL, so [`Bucket::signed_url`] answers `None`.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;

use crate::bucket::{
    check_prefix, next, once, Body, Bucket, ByteRange, GetObject, Listed, ObjectHead, ObjectPath,
    PutOptions, SignedGet, SignedMethod,
};
use crate::error::StorageError;
use crate::local::{offload, remove_if_present, restrict, restrict_file, temp_name};

const TMP: &str = "tmp";
const META_SUFFIX: &str = ".meta";
const SIDECAR_HEADER: &str = "brocanth-storage/1";

/// Objects under one directory. Cheap to clone: a clone is the same store.
#[derive(Clone, Debug)]
pub struct FsBucket {
    root: PathBuf,
}

impl FsBucket {
    /// Opens, and creates if needed, a bucket rooted at `root`.
    ///
    /// # Errors
    /// [`StorageError::Backend`] when the directories cannot be created or
    /// the root cannot be resolved.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, StorageError> {
        let root = root.as_ref();
        fs::create_dir_all(root).map_err(|e| StorageError::io("create the storage root", e))?;
        let root =
            fs::canonicalize(root).map_err(|e| StorageError::io("resolve the storage root", e))?;
        restrict(&root)?;
        let ignore = root.join(".gitignore");
        if !ignore.exists() {
            let mut file = fs::File::create(&ignore)
                .map_err(|e| StorageError::io("write the storage root's .gitignore", e))?;
            restrict_file(&file)?;
            file.write_all(b"# Uploads, written by brocanth-storage. Not source.\n*\n")
                .map_err(|e| StorageError::io("write the storage root's .gitignore", e))?;
        }
        let tmp = root.join(TMP);
        fs::create_dir_all(&tmp).map_err(|e| StorageError::io("create a storage directory", e))?;
        restrict(&tmp)?;
        Ok(Self { root })
    }

    /// The resolved root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn file(&self, path: &ObjectPath) -> Result<PathBuf, StorageError> {
        let text = path.as_str();
        if text == ".gitignore"
            || text.split('/').next() == Some(TMP)
            || text.ends_with(META_SUFFIX)
        {
            return Err(StorageError::InvalidPath(
                "it is reserved on a disk store (`tmp/…`, `….meta`, `.gitignore`)",
            ));
        }
        // Segments are `A-Z a-z 0-9 . _ -` and none is `.` or `..`, so the
        // join cannot leave the root.
        Ok(self.root.join(text))
    }

    fn head_blocking(path: &ObjectPath, file: &Path) -> Result<ObjectHead, StorageError> {
        let size = fs::metadata(file)
            .map_err(|e| StorageError::io("read an object's size", e))?
            .len();
        let (content_type, metadata) = match fs::read_to_string(sidecar_of(file)) {
            Ok(text) => parse_sidecar(&text),
            Err(_) => (None, BTreeMap::new()),
        };
        Ok(ObjectHead {
            path: path.clone(),
            size,
            content_type,
            content_disposition: None,
            metadata,
        })
    }
}

fn sidecar_of(file: &Path) -> PathBuf {
    let mut path = file.as_os_str().to_owned();
    path.push(META_SUFFIX);
    PathBuf::from(path)
}

/// `LocalDisk`'s sidecar, with every metadata entry as a line. `LocalDisk`
/// reads `content-type` and `filename` and passes over the rest. The values
/// hold no control character ([`PutOptions::check`]), so a line is a safe
/// frame.
fn sidecar(content_type: Option<&str>, metadata: &BTreeMap<String, String>) -> String {
    let mut out = format!("{SIDECAR_HEADER}\n");
    if let Some(ct) = content_type {
        out.push_str(&format!("content-type: {ct}\n"));
    }
    for (key, value) in metadata {
        out.push_str(&format!("{key}: {value}\n"));
    }
    out
}

fn parse_sidecar(text: &str) -> (Option<String>, BTreeMap<String, String>) {
    let mut content_type = None;
    let mut metadata = BTreeMap::new();
    for line in text.lines().skip(1) {
        if let Some((key, value)) = line.split_once(": ") {
            if key == "content-type" {
                content_type = Some(value.to_string());
            } else {
                metadata.insert(key.to_string(), value.to_string());
            }
        }
    }
    (content_type, metadata)
}

/// Writes `content` to `dest` through a temporary file: write, `fsync`,
/// rename.
fn write_atomic(root: &Path, dest: &Path, content: &[u8]) -> Result<(), StorageError> {
    let tmp = root.join(TMP).join(temp_name()?);
    let result = (|| {
        let mut file =
            fs::File::create(&tmp).map_err(|e| StorageError::io("create a temporary file", e))?;
        restrict_file(&file)?;
        file.write_all(content)
            .map_err(|e| StorageError::io("write a temporary file", e))?;
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

fn make_parent(root: &Path, dest: &Path) -> Result<(), StorageError> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| StorageError::io("create an object directory", e))?;
        // Each directory between the root and the object, so none is
        // world-readable whatever the umask.
        let mut dir = parent;
        while dir != root && dir.starts_with(root) {
            restrict(dir)?;
            match dir.parent() {
                Some(up) => dir = up,
                None => break,
            }
        }
    }
    Ok(())
}

/// Removes the empty directories above `file`, up to the root. A failure
/// stops it: a directory that is not empty is the usual answer.
fn prune(root: &Path, file: &Path) {
    let mut dir = file.parent();
    while let Some(d) = dir {
        if d == root || !d.starts_with(root) || fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

/// Moves the finished temporary file onto its path.
fn publish(
    root: &Path,
    tmp: &Path,
    dest: &Path,
    create_only: bool,
    side: &str,
) -> Result<(), StorageError> {
    make_parent(root, dest)?;
    let sidecar_path = sidecar_of(dest);
    if create_only {
        // A link fails on a name that exists, atomically, which is what a
        // rename cannot do: rename replaces.
        match fs::hard_link(tmp, dest) {
            Ok(()) => {
                let _ = fs::remove_file(tmp);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let _ = fs::remove_file(tmp);
                // A put that failed after the bytes and before the sidecar
                // left an object with no sidecar; this one repairs it, as
                // `LocalDisk` does.
                if !sidecar_path.exists() {
                    write_atomic(root, &sidecar_path, side.as_bytes())?;
                }
                return Err(StorageError::Exists);
            }
            // A filesystem with no hard links: check, then rename. Not
            // atomic, and said so on `PutOptions::create_only`.
            Err(_) => {
                if dest.exists() {
                    let _ = fs::remove_file(tmp);
                    return Err(StorageError::Exists);
                }
                fs::rename(tmp, dest).map_err(|e| StorageError::io("publish an object", e))?;
            }
        }
    } else {
        fs::rename(tmp, dest).map_err(|e| StorageError::io("publish an object", e))?;
    }
    write_atomic(root, &sidecar_path, side.as_bytes())
}

fn too_large(size: u64, limit: u64) -> StorageError {
    StorageError::TooLarge { size, limit }
}

#[async_trait]
impl Bucket for FsBucket {
    async fn put(
        &self,
        path: &ObjectPath,
        body: Body,
        options: PutOptions,
    ) -> Result<ObjectHead, StorageError> {
        options.check()?;
        let dest = self.file(path)?;
        let limit = options.max_bytes;
        let root = self.root.clone();
        let tmp = root.join(TMP).join(temp_name()?);
        let side = sidecar(options.content_type.as_deref(), &options.metadata);

        let opened = tmp.clone();
        let mut file = offload(move || {
            let file = fs::File::create(&opened)
                .map_err(|e| StorageError::io("create a temporary file", e))?;
            restrict_file(&file)?;
            Ok(file)
        })
        .await?;

        let written: Result<(fs::File, u64), StorageError> = async move {
            let mut size = 0u64;
            match body {
                Body::Bytes(bytes) => {
                    size = bytes.len() as u64;
                    if let Some(limit) = limit.filter(|l| size > *l) {
                        return Err(too_large(size, limit));
                    }
                    file = offload(move || {
                        file.write_all(&bytes)
                            .map_err(|e| StorageError::io("write a temporary file", e))?;
                        Ok(file)
                    })
                    .await?;
                }
                Body::Stream(mut stream) => {
                    while let Some(chunk) = next(&mut stream).await {
                        let chunk = chunk?;
                        size += chunk.len() as u64;
                        if let Some(limit) = limit.filter(|l| size > *l) {
                            return Err(too_large(size, limit));
                        }
                        file = offload(move || {
                            file.write_all(&chunk)
                                .map_err(|e| StorageError::io("write a temporary file", e))?;
                            Ok(file)
                        })
                        .await?;
                    }
                }
            }
            Ok((file, size))
        }
        .await;

        let (file, size) = match written {
            Ok(done) => done,
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                return Err(e);
            }
        };
        let create_only = options.create_only;
        let finished = tmp.clone();
        let result = offload(move || {
            // The rename publishes the name, the sync the bytes; only this
            // order survives a power cut (see `LocalDisk`).
            file.sync_all()
                .map_err(|e| StorageError::io("flush a temporary file", e))?;
            drop(file);
            publish(&root, &finished, &dest, create_only, &side)
        })
        .await;
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result?;
        Ok(ObjectHead {
            path: path.clone(),
            size,
            content_type: options.content_type,
            content_disposition: None,
            metadata: options.metadata,
        })
    }

    async fn get(
        &self,
        path: &ObjectPath,
        range: Option<ByteRange>,
    ) -> Result<GetObject, StorageError> {
        let file = self.file(path)?;
        let path = path.clone();
        offload(move || {
            let head = Self::head_blocking(&path, &file)?;
            let range = match range {
                Some(r) => r.within(head.size)?,
                None => 0..head.size,
            };
            let mut f = fs::File::open(&file).map_err(|e| StorageError::io("read an object", e))?;
            f.seek(SeekFrom::Start(range.start))
                .map_err(|e| StorageError::io("read an object", e))?;
            let mut out = Vec::with_capacity((range.end - range.start) as usize);
            f.take(range.end - range.start)
                .read_to_end(&mut out)
                .map_err(|e| StorageError::io("read an object", e))?;
            Ok(GetObject {
                head,
                range,
                body: once(Bytes::from(out)),
            })
        })
        .await
    }

    async fn head(&self, path: &ObjectPath) -> Result<ObjectHead, StorageError> {
        let file = self.file(path)?;
        let path = path.clone();
        offload(move || Self::head_blocking(&path, &file)).await
    }

    async fn delete(&self, path: &ObjectPath) -> Result<(), StorageError> {
        let file = self.file(path)?;
        let root = self.root.clone();
        offload(move || {
            remove_if_present(&sidecar_of(&file), "remove an object's metadata")?;
            remove_if_present(&file, "remove an object")?;
            prune(&root, &file);
            Ok(())
        })
        .await
    }

    async fn list(&self, prefix: &str) -> Result<Vec<Listed>, StorageError> {
        check_prefix(prefix)?;
        let root = self.root.clone();
        let prefix = prefix.to_string();
        offload(move || {
            // Walk only the deepest directory the prefix names.
            let dir = match prefix.rfind('/') {
                Some(i) => root.join(&prefix[..i]),
                None => root.clone(),
            };
            let mut out = Vec::new();
            walk(&root, &dir, &prefix, &mut out)?;
            Ok(out)
        })
        .await
    }

    async fn signed_url(
        &self,
        _method: SignedMethod,
        _path: &ObjectPath,
        _ttl: Duration,
        _overrides: &SignedGet,
    ) -> Result<Option<String>, StorageError> {
        Ok(None)
    }

    async fn probe(&self) -> Result<(), StorageError> {
        let root = self.root.clone();
        offload(move || {
            let probe = root.join(TMP).join(temp_name()?);
            fs::write(&probe, b"probe").map_err(|e| {
                StorageError::backend(format!(
                    "[storage] cannot write under the storage root: {e}\n\
                     help: set `root` under [storage] to a directory this process can write"
                ))
            })?;
            let _ = fs::remove_file(&probe);
            Ok(())
        })
        .await
    }
}

fn walk(root: &Path, dir: &Path, prefix: &str, out: &mut Vec<Listed>) -> Result<(), StorageError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(StorageError::io("list a storage directory", e)),
    };
    for entry in entries {
        let entry = entry.map_err(|e| StorageError::io("list a storage directory", e))?;
        // `file_type` does not follow a symlink, and a symlink is skipped:
        // nothing this store wrote is one.
        let kind = entry
            .file_type()
            .map_err(|e| StorageError::io("list a storage directory", e))?;
        let full = entry.path();
        let Ok(rel) = full.strip_prefix(root) else {
            continue;
        };
        let Some(rel) = rel.to_str().map(|r| r.replace('\\', "/")) else {
            continue;
        };
        if kind.is_dir() {
            if rel == TMP {
                continue;
            }
            // Descend only where the prefix can still match.
            let as_dir = format!("{rel}/");
            if as_dir.starts_with(prefix) || prefix.starts_with(&as_dir) {
                walk(root, &full, prefix, out)?;
            }
        } else if kind.is_file() {
            if rel == ".gitignore" || rel.ends_with(META_SUFFIX) || !rel.starts_with(prefix) {
                continue;
            }
            let Ok(path) = ObjectPath::parse(&rel) else {
                continue;
            };
            let size = entry
                .metadata()
                .map_err(|e| StorageError::io("read an object's size", e))?
                .len();
            out.push(Listed { path, size });
        }
    }
    Ok(())
}
