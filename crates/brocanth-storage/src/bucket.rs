//! The raw contract a backend implements: a flat namespace of named objects.
//!
//! [`Storage`](crate::Storage) is what callers name. It holds the rules that
//! make an upload safe to serve: an address that cannot spell a path, a
//! media type sniffed from the bytes, the first upload owning the metadata.
//! [`Bucket`] is what a backend implements, and it holds none of them: put
//! bytes at a path, read them back, list a prefix, sign a URL. One
//! [`ObjectStorage`](crate::ObjectStorage) turns any `Bucket` into a
//! `Storage`, so those rules exist once, not once per cloud.
//! `docs/object-storage.md` has the design.
//!
//! Three implementations: [`FsBucket`](crate::FsBucket) on a disk, and
//! [`CloudBucket`](crate::cloud) on S3 or Google Cloud Storage behind the
//! `s3` and `gcs` features.
//!
//! # Paths
//!
//! An [`ObjectPath`] is checked once, when it is built: segments of
//! `A-Z a-z 0-9 . _ -` joined by `/`, no empty, `.` or `..` segment, at most
//! [`MAX_PATH_LEN`] bytes. On a disk that rule is the traversal defence; on
//! a cloud it keeps a key free of characters each provider escapes
//! differently. `ObjectStorage` builds every path from hex and a
//! [`VariantSpec`](crate::VariantSpec), never from a client's string.

use std::collections::BTreeMap;
use std::fmt;
use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures_core::Stream;

use crate::error::StorageError;
use crate::meta::{attachment, ContentType, Filename};

/// The longest [`ObjectPath`], in bytes. S3 and GCS take 1024.
pub const MAX_PATH_LEN: usize = 1024;

/// A stream of body chunks. `Send` and boxed, so a trait object can return
/// one.
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, StorageError>> + Send>>;

/// A checked object path: `objects/ab/cd/abcd…`. See the module
/// documentation for the rule.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectPath(String);

impl ObjectPath {
    /// Checks `text` and wraps it.
    ///
    /// # Errors
    /// [`StorageError::InvalidPath`] with the rule it broke. The text is not
    /// repeated in the error: it may have come from a client.
    pub fn parse(text: &str) -> Result<Self, StorageError> {
        check(text, false)?;
        Ok(Self(text.to_string()))
    }

    /// The path as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The path below `self`: `self/segment`.
    ///
    /// # Errors
    /// [`StorageError::InvalidPath`] when `segment` is not one segment.
    pub fn join(&self, segment: &str) -> Result<Self, StorageError> {
        if segment.contains('/') {
            return Err(StorageError::InvalidPath("a joined segment holds a `/`"));
        }
        Self::parse(&format!("{}/{segment}", self.0))
    }
}

impl fmt::Display for ObjectPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for ObjectPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ObjectPath({:?})", self.0)
    }
}

/// Checks a list prefix: an [`ObjectPath`], empty, or one with a trailing
/// `/`.
///
/// # Errors
/// [`StorageError::InvalidPath`].
pub fn check_prefix(prefix: &str) -> Result<(), StorageError> {
    if prefix.is_empty() {
        return Ok(());
    }
    check(prefix.strip_suffix('/').unwrap_or(prefix), true)
}

fn check(text: &str, prefix: bool) -> Result<(), StorageError> {
    if text.is_empty() {
        return Err(StorageError::InvalidPath("it is empty"));
    }
    if text.len() > MAX_PATH_LEN {
        return Err(StorageError::InvalidPath("it is longer than 1024 bytes"));
    }
    for segment in text.split('/') {
        if segment.is_empty() {
            return Err(StorageError::InvalidPath(
                "it has an empty segment, or starts or ends with `/`",
            ));
        }
        if segment == "." || segment == ".." {
            return Err(StorageError::InvalidPath("it has a `.` or `..` segment"));
        }
        if !segment
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        {
            return Err(StorageError::InvalidPath(if prefix {
                "the prefix holds a character outside `A-Z a-z 0-9 . _ -`"
            } else {
                "it holds a character outside `A-Z a-z 0-9 . _ -`"
            }));
        }
    }
    Ok(())
}

/// A byte range: `start..end`, end exclusive, or `start..` to the end.
///
/// A range that ends past the object is cut at its end. A range that starts
/// at or after the end is [`StorageError::InvalidRange`], as an empty one
/// is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    /// The first byte.
    pub start: u64,
    /// One past the last byte, or `None` for the end of the object.
    pub end: Option<u64>,
}

impl ByteRange {
    /// `start..end`.
    ///
    /// # Errors
    /// [`StorageError::InvalidRange`] when `start >= end`.
    pub fn new(start: u64, end: u64) -> Result<Self, StorageError> {
        if start >= end {
            return Err(StorageError::InvalidRange);
        }
        Ok(Self {
            start,
            end: Some(end),
        })
    }

    /// `start..`, to the end of the object.
    pub fn from(start: u64) -> Self {
        Self { start, end: None }
    }

    /// The range inside an object of `size` bytes, as `start..end`.
    ///
    /// # Errors
    /// [`StorageError::InvalidRange`] when it starts at or after `size`.
    pub fn within(self, size: u64) -> Result<std::ops::Range<u64>, StorageError> {
        let end = self.end.map_or(size, |end| end.min(size));
        if self.start >= end {
            return Err(StorageError::InvalidRange);
        }
        Ok(self.start..end)
    }

    /// The same cut on bytes already in memory.
    ///
    /// # Errors
    /// [`StorageError::InvalidRange`], as [`ByteRange::within`].
    pub fn slice(self, bytes: &[u8]) -> Result<Vec<u8>, StorageError> {
        let range = self.within(bytes.len() as u64)?;
        Ok(bytes[range.start as usize..range.end as usize].to_vec())
    }
}

/// A body to store: bytes already in memory, or a stream of chunks.
pub enum Body {
    /// The whole body.
    Bytes(Bytes),
    /// Chunks, read as they come. A backend counts them against
    /// [`PutOptions::max_bytes`] and stops past it.
    Stream(ByteStream),
}

impl From<Vec<u8>> for Body {
    fn from(bytes: Vec<u8>) -> Self {
        Self::Bytes(Bytes::from(bytes))
    }
}

impl From<Bytes> for Body {
    fn from(bytes: Bytes) -> Self {
        Self::Bytes(bytes)
    }
}

impl From<&[u8]> for Body {
    fn from(bytes: &[u8]) -> Self {
        Self::Bytes(Bytes::copy_from_slice(bytes))
    }
}

impl fmt::Debug for Body {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bytes(b) => write!(f, "Body::Bytes({} bytes)", b.len()),
            Self::Stream(_) => f.write_str("Body::Stream"),
        }
    }
}

/// How to store a body.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PutOptions {
    /// The media type to store. A bucket stores what it is given: sniffing
    /// is [`ObjectStorage`](crate::ObjectStorage)'s job, not the backend's.
    pub content_type: Option<String>,
    /// The `Content-Disposition` to store, which S3 and GCS send with every
    /// read of the object, a signed URL's included. A disk serves nothing
    /// by URL, so [`FsBucket`](crate::FsBucket) does not keep it.
    pub content_disposition: Option<String>,
    /// User metadata. Keys are `a-z 0-9 -`; values are text with no control
    /// character. Each backend encodes them as its headers need.
    pub metadata: BTreeMap<String, String>,
    /// The most bytes to store. Past it the put stops with
    /// [`StorageError::TooLarge`] and nothing is stored.
    pub max_bytes: Option<u64>,
    /// Fail with [`StorageError::Exists`] when something is already stored
    /// at the path, rather than replace it.
    ///
    /// Atomic for a body in memory on every backend: S3 `If-None-Match: *`,
    /// GCS `ifGenerationMatch=0`, a link on a disk. For a stream large
    /// enough to go up in parts, the check is a `head` before the upload,
    /// and two concurrent uploads can both pass it.
    pub create_only: bool,
}

impl PutOptions {
    /// No media type, no metadata, no limit, replace.
    pub fn new() -> Self {
        Self::default()
    }

    /// With this media type.
    #[must_use]
    pub fn with_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    /// With this `Content-Disposition`.
    #[must_use]
    pub fn with_content_disposition(mut self, disposition: impl Into<String>) -> Self {
        self.content_disposition = Some(disposition.into());
        self
    }

    /// With one metadata entry.
    #[must_use]
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// With a size limit.
    #[must_use]
    pub fn with_max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = Some(max_bytes);
        self
    }

    /// Create only: see [`PutOptions::create_only`].
    #[must_use]
    pub fn create_only(mut self) -> Self {
        self.create_only = true;
        self
    }

    /// Refuses a metadata key or value no backend can carry.
    ///
    /// # Errors
    /// [`StorageError::Backend`] with a `help:` line.
    pub fn check(&self) -> Result<(), StorageError> {
        for (key, value) in &self.metadata {
            let key_ok = !key.is_empty()
                && key != "content-type"
                && key
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
            if !key_ok {
                return Err(StorageError::backend(
                    "a metadata key is not `a-z 0-9 -`, or is `content-type`\n\
                     help: name metadata with lowercase letters, digits and `-`; \
                     set the media type with `content_type`",
                ));
            }
            if value.chars().any(char::is_control) {
                return Err(StorageError::backend(
                    "a metadata value holds a control character\n\
                     help: remove line breaks and other control characters from the value",
                ));
            }
        }
        if let Some(ct) = &self.content_type {
            if ct.chars().any(|c| c.is_control() || !c.is_ascii()) {
                return Err(StorageError::backend(
                    "the media type holds a character a header cannot carry\n\
                     help: give a media type such as `image/png`",
                ));
            }
        }
        if let Some(cd) = &self.content_disposition {
            if cd.chars().any(|c| c.is_control() || !c.is_ascii()) {
                return Err(StorageError::backend(
                    "the content disposition holds a character a header cannot carry\n\
                     help: build it with `brocanth_storage::meta::attachment`",
                ));
            }
        }
        Ok(())
    }
}

/// What a bucket knows about one object, without its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectHead {
    /// Where it is.
    pub path: ObjectPath,
    /// Its size in bytes.
    pub size: u64,
    /// The media type it was stored with, if any.
    pub content_type: Option<String>,
    /// The `Content-Disposition` it was stored with, if the backend keeps
    /// one: S3 and GCS do, a disk does not.
    pub content_disposition: Option<String>,
    /// The user metadata it was stored with.
    pub metadata: BTreeMap<String, String>,
}

/// One entry of [`Bucket::list`]: a path and a size. A list answers no
/// metadata, because S3 and GCS do not return it there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    /// Where it is.
    pub path: ObjectPath,
    /// Its size in bytes.
    pub size: u64,
}

/// A body being read: what is known about the object, and the chunks.
pub struct GetObject {
    /// The object.
    pub head: ObjectHead,
    /// The bytes this read covers, as `start..end`: the whole object, or
    /// the range asked for, cut at the end.
    pub range: std::ops::Range<u64>,
    /// The chunks.
    pub body: ByteStream,
}

impl GetObject {
    /// Reads the whole body into memory.
    ///
    /// # Errors
    /// What the stream failed with.
    pub async fn bytes(self) -> Result<Vec<u8>, StorageError> {
        let mut out = Vec::with_capacity((self.range.end - self.range.start) as usize);
        let mut body = self.body;
        while let Some(chunk) = next(&mut body).await {
            out.extend_from_slice(&chunk?);
        }
        Ok(out)
    }
}

impl fmt::Debug for GetObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GetObject")
            .field("head", &self.head)
            .field("range", &self.range)
            .finish_non_exhaustive()
    }
}

/// The request a signed URL allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignedMethod {
    /// Read the object.
    Get,
    /// Store a body at the path.
    Put,
}

/// The response headers a signed `GET` makes the bucket send, inside the
/// signature: `response-content-type` and `response-content-disposition`.
///
/// The bearer of the URL cannot remove them: the signature then fails. An
/// empty one signs a plain URL. [`SignedGet::for_object`] is the rule a
/// download follows (`docs/object-storage.md` D29).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct SignedGet {
    /// The `Content-Type` the bucket answers with.
    pub content_type: Option<String>,
    /// The `Content-Disposition` the bucket answers with.
    pub content_disposition: Option<String>,
}

impl SignedGet {
    /// No override: the bucket answers with what it stored.
    pub fn none() -> Self {
        Self::default()
    }

    /// The rule for a download (D29). A sniffed PNG, JPEG, GIF or WebP goes
    /// inline, as its type. Every other object goes as an attachment of
    /// type `application/octet-stream`, so a browser saves it and renders
    /// nothing: not HTML, not SVG, not a PDF with script.
    pub fn for_object(content_type: &ContentType, filename: Option<&Filename>) -> Self {
        if content_type.is_safe_to_inline() {
            Self {
                content_type: Some(content_type.essence().to_string()),
                content_disposition: None,
            }
        } else {
            Self {
                content_type: Some(ContentType::octet_stream().as_str().to_string()),
                content_disposition: Some(attachment(filename)),
            }
        }
    }

    /// Whether it overrides nothing.
    pub fn is_empty(&self) -> bool {
        self.content_type.is_none() && self.content_disposition.is_none()
    }

    /// The query pairs S3 and the GCS XML API read: `response-content-type`
    /// and `response-content-disposition`.
    pub fn query(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if let Some(ct) = &self.content_type {
            out.push(("response-content-type".to_string(), ct.clone()));
        }
        if let Some(cd) = &self.content_disposition {
            out.push(("response-content-disposition".to_string(), cd.clone()));
        }
        out
    }
}

/// The next chunk of a stream, with no combinator crate: `poll_next` under
/// `poll_fn`.
pub(crate) async fn next(stream: &mut ByteStream) -> Option<Result<Bytes, StorageError>> {
    std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await
}

/// A stream of one chunk.
pub(crate) fn once(bytes: Bytes) -> ByteStream {
    struct Once(Option<Bytes>);
    impl Stream for Once {
        type Item = Result<Bytes, StorageError>;
        fn poll_next(
            mut self: Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Self::Item>> {
            std::task::Poll::Ready(self.0.take().filter(|b| !b.is_empty()).map(Ok))
        }
    }
    Box::pin(Once(Some(bytes)))
}

/// A flat namespace of named objects: a directory, an S3 bucket, a GCS
/// bucket.
///
/// Every method takes `&self` and the trait is object-safe, so
/// `Arc<dyn Bucket>` is the way to hold one. Errors name the operation,
/// never the path: a path holds an object address, and an address in an
/// error page is an address in a log aggregator.
#[async_trait]
pub trait Bucket: Send + Sync + 'static {
    /// Stores `body` at `path`, replacing what is there unless
    /// [`PutOptions::create_only`].
    ///
    /// # Errors
    /// [`StorageError::TooLarge`] past [`PutOptions::max_bytes`],
    /// [`StorageError::Exists`] for a create-only put on a taken path,
    /// [`StorageError::Backend`] otherwise.
    async fn put(
        &self,
        path: &ObjectPath,
        body: Body,
        options: PutOptions,
    ) -> Result<ObjectHead, StorageError>;

    /// Reads an object, or the part of it `range` covers.
    ///
    /// # Errors
    /// [`StorageError::NotFound`], [`StorageError::InvalidRange`].
    async fn get(
        &self,
        path: &ObjectPath,
        range: Option<ByteRange>,
    ) -> Result<GetObject, StorageError>;

    /// The size, media type and metadata, without the body.
    ///
    /// # Errors
    /// [`StorageError::NotFound`].
    async fn head(&self, path: &ObjectPath) -> Result<ObjectHead, StorageError>;

    /// Removes an object. Removing what is not there succeeds.
    ///
    /// # Errors
    /// [`StorageError::Backend`] when the backend could not remove it.
    async fn delete(&self, path: &ObjectPath) -> Result<(), StorageError>;

    /// Every object whose path starts with `prefix`, in no promised order.
    /// `prefix` is empty, a path, or a path with a trailing `/`
    /// ([`check_prefix`]).
    ///
    /// # Errors
    /// [`StorageError::InvalidPath`], [`StorageError::Backend`].
    async fn list(&self, prefix: &str) -> Result<Vec<Listed>, StorageError>;

    /// A URL that allows `method` on `path` for `ttl`, with no other
    /// credential. `None` when this backend cannot sign: a disk cannot.
    ///
    /// For a `GET`, `overrides` are signed into the URL, and the bucket
    /// answers with them. A `PUT` takes none.
    ///
    /// # Errors
    /// [`StorageError::Backend`] when the backend can sign and failed to,
    /// or when a `PUT` is given overrides.
    async fn signed_url(
        &self,
        method: SignedMethod,
        path: &ObjectPath,
        ttl: Duration,
        overrides: &SignedGet,
    ) -> Result<Option<String>, StorageError>;

    /// Whether [`Bucket::signed_url`] answers a URL. Known when the bucket
    /// is built, so a setting that needs one fails at start
    /// (`docs/object-storage.md` D33). A disk cannot sign.
    fn can_sign(&self) -> bool {
        false
    }

    /// The start-time check: the bucket exists and the credentials can
    /// read it. Cheap: one list of at most one object.
    ///
    /// # Errors
    /// [`StorageError::Backend`] with a `help:` line that names the setting
    /// to check.
    async fn probe(&self) -> Result<(), StorageError>;
}

#[async_trait]
impl<T: Bucket + ?Sized> Bucket for std::sync::Arc<T> {
    async fn put(
        &self,
        path: &ObjectPath,
        body: Body,
        options: PutOptions,
    ) -> Result<ObjectHead, StorageError> {
        (**self).put(path, body, options).await
    }

    async fn get(
        &self,
        path: &ObjectPath,
        range: Option<ByteRange>,
    ) -> Result<GetObject, StorageError> {
        (**self).get(path, range).await
    }

    async fn head(&self, path: &ObjectPath) -> Result<ObjectHead, StorageError> {
        (**self).head(path).await
    }

    async fn delete(&self, path: &ObjectPath) -> Result<(), StorageError> {
        (**self).delete(path).await
    }

    async fn list(&self, prefix: &str) -> Result<Vec<Listed>, StorageError> {
        (**self).list(prefix).await
    }

    async fn signed_url(
        &self,
        method: SignedMethod,
        path: &ObjectPath,
        ttl: Duration,
        overrides: &SignedGet,
    ) -> Result<Option<String>, StorageError> {
        (**self).signed_url(method, path, ttl, overrides).await
    }

    fn can_sign(&self) -> bool {
        (**self).can_sign()
    }

    async fn probe(&self) -> Result<(), StorageError> {
        (**self).probe().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_segments_of_a_small_alphabet() {
        for good in ["a", "objects/ab/cd/0f", "variants/x/300x200t", "a.b_c-d"] {
            assert!(ObjectPath::parse(good).is_ok(), "{good}");
        }
        for bad in [
            "", "/a", "a/", "a//b", "..", "a/../b", ".", "a/./b", "a b", "a\\b", "é", "%2e", "a\0b",
        ] {
            assert!(
                matches!(ObjectPath::parse(bad), Err(StorageError::InvalidPath(_))),
                "{bad:?}"
            );
        }
        assert!(ObjectPath::parse(&"a".repeat(MAX_PATH_LEN + 1)).is_err());
        let p = ObjectPath::parse("a/b").unwrap();
        assert_eq!(p.join("c").unwrap().as_str(), "a/b/c");
        assert!(p.join("c/d").is_err());
        assert!(p.join("..").is_err());
    }

    #[test]
    fn a_prefix_may_be_empty_or_end_with_a_slash() {
        assert!(check_prefix("").is_ok());
        assert!(check_prefix("variants/ab/").is_ok());
        assert!(check_prefix("variants/a").is_ok());
        assert!(check_prefix("/").is_err());
        assert!(check_prefix("../").is_err());
    }

    #[test]
    fn a_range_is_cut_at_the_end_and_refused_past_it() {
        let body = b"0123456789";
        assert_eq!(ByteRange::new(2, 5).unwrap().slice(body).unwrap(), b"234");
        assert_eq!(ByteRange::new(8, 50).unwrap().slice(body).unwrap(), b"89");
        assert_eq!(ByteRange::from(7).slice(body).unwrap(), b"789");
        assert!(matches!(
            ByteRange::from(10).slice(body),
            Err(StorageError::InvalidRange)
        ));
        assert!(matches!(
            ByteRange::new(3, 3),
            Err(StorageError::InvalidRange)
        ));
    }

    #[test]
    fn a_download_signs_an_image_inline_and_anything_else_as_an_attachment() {
        let png = ContentType::parse("image/png").unwrap();
        assert_eq!(
            SignedGet::for_object(&png, None),
            SignedGet {
                content_type: Some("image/png".into()),
                content_disposition: None,
            }
        );
        let name = Filename::sanitise("résumé.html");
        for declared in ["text/html", "image/svg+xml", "application/pdf"] {
            let ct = ContentType::parse(declared).unwrap();
            let rule = SignedGet::for_object(&ct, name.as_ref());
            assert_eq!(
                rule.content_type.as_deref(),
                Some("application/octet-stream")
            );
            assert_eq!(
                rule.content_disposition.as_deref(),
                Some(
                    "attachment; filename=\"r_sum_.html\"; filename*=UTF-8''r%C3%A9sum%C3%A9.html"
                )
            );
        }
        let pairs = SignedGet::for_object(&ContentType::octet_stream(), None).query();
        assert_eq!(
            pairs,
            [
                (
                    "response-content-type".to_string(),
                    "application/octet-stream".to_string()
                ),
                (
                    "response-content-disposition".to_string(),
                    "attachment; filename=\"download\"".to_string()
                ),
            ]
        );
        assert!(SignedGet::none().query().is_empty());
    }

    #[test]
    fn metadata_is_checked_before_any_backend_sees_it() {
        assert!(PutOptions::new()
            .with_metadata("filename", "ünïcode % ok.png")
            .check()
            .is_ok());
        assert!(PutOptions::new()
            .with_metadata("Filename", "x")
            .check()
            .is_err());
        assert!(PutOptions::new()
            .with_metadata("content-type", "x")
            .check()
            .is_err());
        assert!(PutOptions::new()
            .with_metadata("filename", "a\nb")
            .check()
            .is_err());
        assert!(PutOptions::new()
            .with_content_type("text/plain\r\nx: y")
            .check()
            .is_err());
        assert!(PutOptions::new()
            .with_content_disposition("attachment\r\nx: y")
            .check()
            .is_err());
    }
}
