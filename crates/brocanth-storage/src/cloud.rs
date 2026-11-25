//! Buckets in the cloud: S3 (and S3-compatible stores) behind `s3`, Google
//! Cloud Storage behind `gcs`.
//!
//! One type, [`CloudBucket`], over the `object_store` crate from the Apache
//! Arrow project: one maintained client for both clouds, on the tokio and
//! reqwest the workspace already links. `docs/object-storage.md` §4 says
//! why it and not the two vendor SDKs.
//!
//! ```no_run
//! # #[cfg(feature = "s3")]
//! # fn main() -> Result<(), brocanth_storage::StorageError> {
//! use std::sync::Arc;
//! use brocanth_storage::cloud::{CloudBucket, S3Config};
//! use brocanth_storage::{HashKey, ObjectStorage};
//!
//! let bucket = CloudBucket::s3(&S3Config {
//!     bucket: "uploads".into(),
//!     endpoint: Some("http://localhost:9000".into()), // MinIO; `None` for AWS
//!     ..S3Config::default()
//! })?;
//! let store = ObjectStorage::new(Arc::new(bucket))
//!     .with_hash_key(HashKey::from_secret(b"deployment secret"));
//! # let _ = store;
//! # Ok(())
//! # }
//! # #[cfg(not(feature = "s3"))]
//! # fn main() {}
//! ```
//!
//! # Credentials
//!
//! A key in the config, or none. With none, the platform's chain answers:
//! for S3 the `AWS_*` variables, a web identity token (EKS), the container
//! credentials (ECS) and the instance role; for GCS the application default
//! credentials (`GOOGLE_APPLICATION_CREDENTIALS`, `gcloud`), and the
//! metadata server (GKE workload identity, Cloud Run). A deployed service
//! should have no key to give.
//!
//! # Errors
//!
//! Every error names the operation and the bucket, never the object path,
//! and carries a `help:` line. The client's own message is kept with the
//! path cut out of it.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::StreamExt as _;
use object_store::path::Path as OsPath;
use object_store::signer::Signer;
use object_store::{
    Attribute, AttributeValue, Attributes, GetOptions, GetRange, ObjectStore, ObjectStoreExt as _,
    PutMode, PutMultipartOptions, PutPayload, WriteMultipart,
};

use crate::bucket::{
    check_prefix, next, Body, Bucket, ByteRange, ByteStream, GetObject, Listed, ObjectHead,
    ObjectPath, PutOptions, SignedMethod,
};
use crate::error::StorageError;

/// A body smaller than this goes up in one request, create-only when asked.
/// A larger stream goes up in parts.
const SINGLE_PUT_BYTES: usize = 8 * 1024 * 1024;

/// The longest a signed URL may live: seven days, the limit of both S3 and
/// GCS.
pub const MAX_SIGNED_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

/// An S3 bucket, or one on an S3-compatible server (MinIO, R2).
#[derive(Clone, Default)]
pub struct S3Config {
    /// The bucket. Required.
    pub bucket: String,
    /// The region. Empty means `us-east-1`; R2 takes `auto`.
    pub region: String,
    /// The server, for an S3-compatible store: `http://localhost:9000`.
    /// `None` means AWS. An `http://` URL allows plain HTTP.
    pub endpoint: Option<String>,
    /// Path-style requests (`endpoint/bucket/key`) rather than
    /// virtual-hosted (`bucket.endpoint/key`). MinIO wants path style.
    pub path_style: bool,
    /// The access key. With [`S3Config::secret_access_key`], or neither: then
    /// the AWS credential chain answers.
    pub access_key_id: Option<String>,
    /// The secret key.
    pub secret_access_key: Option<String>,
    /// A folder inside the bucket: every path goes under it.
    pub prefix: Option<String>,
}

impl std::fmt::Debug for S3Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Config")
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("endpoint", &self.endpoint)
            .field("path_style", &self.path_style)
            .field("access_key_id", &self.access_key_id)
            .field(
                "secret_access_key",
                &self.secret_access_key.as_ref().map(|_| "<redacted>"),
            )
            .field("prefix", &self.prefix)
            .finish()
    }
}

/// A Google Cloud Storage bucket.
#[derive(Clone, Default)]
pub struct GcsConfig {
    /// The bucket. Required.
    pub bucket: String,
    /// A service-account key, as its JSON text. `None`: the application
    /// default credentials answer.
    pub credentials: Option<String>,
    /// An emulator's URL (`http://localhost:4443` for fake-gcs-server).
    /// With it, requests carry no OAuth token: an emulator takes none. A
    /// signed URL still needs the key in [`GcsConfig::credentials`].
    pub endpoint: Option<String>,
    /// A folder inside the bucket: every path goes under it.
    pub prefix: Option<String>,
}

impl std::fmt::Debug for GcsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GcsConfig")
            .field("bucket", &self.bucket)
            .field(
                "credentials",
                &self.credentials.as_ref().map(|_| "<redacted>"),
            )
            .field("endpoint", &self.endpoint)
            .field("prefix", &self.prefix)
            .finish()
    }
}

/// A [`Bucket`] on S3 or Google Cloud Storage. Cheap to clone.
#[derive(Clone)]
pub struct CloudBucket {
    store: Arc<dyn ObjectStore>,
    signer: Arc<dyn Signer>,
    /// `s3` or `gcs`, for messages.
    service: &'static str,
    bucket: String,
    /// The folder every path goes under, without a trailing `/`.
    prefix: Option<String>,
    /// Where the requests go, for the "cannot reach" message.
    endpoint: String,
}

impl std::fmt::Debug for CloudBucket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudBucket")
            .field("service", &self.service)
            .field("bucket", &self.bucket)
            .field("prefix", &self.prefix)
            .field("endpoint", &self.endpoint)
            .finish()
    }
}

fn clean_prefix(prefix: Option<&str>) -> Result<Option<String>, StorageError> {
    let Some(prefix) = prefix
        .map(|p| p.trim_matches('/'))
        .filter(|p| !p.is_empty())
    else {
        return Ok(None);
    };
    ObjectPath::parse(prefix).map_err(|_| {
        StorageError::backend(
            "[storage] prefix is not a path\n\
             help: set `prefix` under [storage] to segments of `A-Z a-z 0-9 . _ -` joined by `/`",
        )
    })?;
    Ok(Some(prefix.to_string()))
}

fn required_bucket(bucket: &str) -> Result<(), StorageError> {
    if bucket.trim().is_empty() {
        return Err(StorageError::backend(
            "[storage] bucket is required\n\
             help: set `bucket` under [storage], or pass --storage.bucket=NAME",
        ));
    }
    Ok(())
}

impl CloudBucket {
    /// A bucket on S3 or an S3-compatible server. Builds the client; sends
    /// nothing. Call [`Bucket::probe`] to check the bucket and the
    /// credentials.
    ///
    /// # Errors
    /// [`StorageError::Backend`] with a `help:` line when the config cannot
    /// make a client: no bucket, half a key, a bad endpoint.
    #[cfg(feature = "s3")]
    pub fn s3(config: &S3Config) -> Result<Self, StorageError> {
        use object_store::aws::AmazonS3Builder;

        required_bucket(&config.bucket)?;
        let region = if config.region.trim().is_empty() {
            "us-east-1".to_string()
        } else {
            config.region.trim().to_string()
        };
        // The environment first, then the config on top, so a setting wins
        // and an empty one leaves the platform's answer.
        let mut builder = AmazonS3Builder::from_env()
            .with_bucket_name(&config.bucket)
            .with_region(&region)
            .with_virtual_hosted_style_request(!config.path_style);
        let mut endpoint = format!("https://s3.{region}.amazonaws.com");
        if let Some(url) = config.endpoint.as_deref().filter(|u| !u.trim().is_empty()) {
            let url = url.trim().trim_end_matches('/');
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(StorageError::backend(
                    "[storage] endpoint is not an http:// or https:// URL\n\
                     help: set `endpoint` under [storage] to the server's URL, such as \
                     http://localhost:9000, or leave it empty for AWS",
                ));
            }
            builder = builder
                .with_endpoint(url)
                .with_allow_http(url.starts_with("http://"));
            endpoint = url.to_string();
        }
        let key = config.access_key_id.as_deref().filter(|k| !k.is_empty());
        let secret = config
            .secret_access_key
            .as_deref()
            .filter(|k| !k.is_empty());
        match (key, secret) {
            (Some(key), Some(secret)) => {
                builder = builder
                    .with_access_key_id(key)
                    .with_secret_access_key(secret);
            }
            (None, None) => {}
            _ => {
                return Err(StorageError::backend(
                    "[storage] access_key_id and secret_access_key go together\n\
                     help: set both, or neither to use the AWS credential chain \
                     (environment, web identity, instance role)",
                ))
            }
        }
        let s3 = Arc::new(builder.build().map_err(|e| {
            StorageError::backend(format!(
                "[storage] cannot build the S3 client: {e}\n\
                 help: check `bucket`, `region` and `endpoint` under [storage]"
            ))
        })?);
        Ok(Self {
            store: s3.clone(),
            signer: s3,
            service: "s3",
            bucket: config.bucket.clone(),
            prefix: clean_prefix(config.prefix.as_deref())?,
            endpoint,
        })
    }

    /// A Google Cloud Storage bucket. Builds the client; sends nothing.
    ///
    /// # Errors
    /// [`StorageError::Backend`] with a `help:` line when the config cannot
    /// make a client: no bucket, a key that is not a service-account JSON.
    #[cfg(feature = "gcs")]
    pub fn gcs(config: &GcsConfig) -> Result<Self, StorageError> {
        use object_store::gcp::{GcpCredential, GoogleCloudStorageBuilder};
        use object_store::StaticCredentialProvider;

        required_bucket(&config.bucket)?;
        let mut builder = GoogleCloudStorageBuilder::from_env().with_bucket_name(&config.bucket);
        if let Some(json) = config
            .credentials
            .as_deref()
            .filter(|c| !c.trim().is_empty())
        {
            builder = builder.with_service_account_key(json);
        }
        let mut endpoint = "https://storage.googleapis.com".to_string();
        if let Some(url) = config.endpoint.as_deref().filter(|u| !u.trim().is_empty()) {
            let url = url.trim().trim_end_matches('/');
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(StorageError::backend(
                    "[storage] endpoint is not an http:// or https:// URL\n\
                     help: set `endpoint` under [storage] to the emulator's URL, such as \
                     http://localhost:4443, or leave it empty for Google",
                ));
            }
            builder = builder
                .with_base_url(url)
                .with_credentials(Arc::new(StaticCredentialProvider::new(GcpCredential {
                    bearer: String::new(),
                })))
                .with_client_options(
                    object_store::ClientOptions::new().with_allow_http(url.starts_with("http://")),
                );
            endpoint = url.to_string();
        }
        let gcs = Arc::new(builder.build().map_err(|e| {
            StorageError::backend(format!(
                "[storage] cannot build the Google Cloud Storage client: {e}\n\
                 help: check `bucket` under [storage], and that `credentials` is a \
                 service-account key in JSON (or empty, for application default credentials)"
            ))
        })?);
        Ok(Self {
            store: gcs.clone(),
            signer: gcs,
            service: "gcs",
            bucket: config.bucket.clone(),
            prefix: clean_prefix(config.prefix.as_deref())?,
            endpoint,
        })
    }

    /// The bucket's name.
    pub fn name(&self) -> &str {
        &self.bucket
    }

    fn full(&self, path: &ObjectPath) -> OsPath {
        match &self.prefix {
            Some(prefix) => OsPath::from(format!("{prefix}/{path}")),
            None => OsPath::from(path.as_str()),
        }
    }

    /// The client's path back as ours, without the prefix.
    fn ours(&self, path: &OsPath) -> Option<ObjectPath> {
        let text = path.as_ref();
        let rel = match &self.prefix {
            Some(prefix) => text.strip_prefix(prefix.as_str())?.strip_prefix('/')?,
            None => text,
        };
        ObjectPath::parse(rel).ok()
    }

    /// Maps a client error, with the object path cut out of its text.
    fn error(
        &self,
        op: &'static str,
        path: Option<&OsPath>,
        e: object_store::Error,
    ) -> StorageError {
        let bucket = &self.bucket;
        let text = scrub(&e.to_string(), path);
        match e {
            object_store::Error::NotFound { .. } => StorageError::NotFound,
            object_store::Error::AlreadyExists { .. }
            | object_store::Error::Precondition { .. } => StorageError::Exists,
            object_store::Error::PermissionDenied { .. }
            | object_store::Error::Unauthenticated { .. } => StorageError::backend(format!(
                "[storage] {op}: the credentials were refused for bucket {bucket:?}: {text}\n\
                 help: {}",
                self.credentials_help()
            )),
            // Some refusals reach the client as a plain status: S3 answers a
            // wrong secret with 403 `SignatureDoesNotMatch`.
            _ if REFUSED.iter().any(|code| text.contains(code)) => StorageError::backend(format!(
                "[storage] {op}: the credentials were refused for bucket {bucket:?}: {text}\n\
                 help: {}",
                self.credentials_help()
            )),
            _ if MISSING.iter().any(|code| text.contains(code)) => self.missing_bucket(),
            _ => StorageError::backend(format!(
                "[storage] {op} on {} bucket {bucket:?} failed: {text}\n\
                 help: check that {} answers; to start without it, set --storage.probe=false",
                self.service, self.endpoint
            )),
        }
    }

    fn credentials_help(&self) -> &'static str {
        if self.service == "s3" {
            "check `access_key_id` and `secret_access_key_file` under [storage], or the role's policy on the bucket"
        } else {
            "check `credentials_file` under [storage], or the service account's role on the bucket"
        }
    }

    fn missing_bucket(&self) -> StorageError {
        StorageError::backend(format!(
            "[storage] {} bucket {:?} does not exist\n\
             help: create it, or set --storage.bucket to one that does",
            self.service, self.bucket
        ))
    }

    fn head_from(&self, path: &ObjectPath, size: u64, attributes: &Attributes) -> ObjectHead {
        let mut metadata = std::collections::BTreeMap::new();
        let mut content_type = None;
        for (key, value) in attributes {
            match key {
                Attribute::ContentType => content_type = Some(value.as_ref().to_string()),
                Attribute::Metadata(name) => {
                    metadata.insert(name.to_ascii_lowercase(), unescape(value.as_ref()));
                }
                _ => {}
            }
        }
        ObjectHead {
            path: path.clone(),
            size,
            content_type,
            metadata,
        }
    }

    fn attributes(options: &PutOptions) -> Attributes {
        let mut attributes = Attributes::new();
        if let Some(ct) = &options.content_type {
            attributes.insert(Attribute::ContentType, AttributeValue::from(ct.clone()));
        }
        for (key, value) in &options.metadata {
            attributes.insert(
                Attribute::Metadata(key.clone().into()),
                AttributeValue::from(escape(value)),
            );
        }
        attributes
    }

    async fn put_whole(
        &self,
        full: &OsPath,
        bytes: Bytes,
        options: &PutOptions,
    ) -> Result<(), StorageError> {
        let put = object_store::PutOptions {
            mode: if options.create_only {
                PutMode::Create
            } else {
                PutMode::Overwrite
            },
            attributes: Self::attributes(options),
            ..Default::default()
        };
        self.store
            .put_opts(full, PutPayload::from(bytes), put)
            .await
            .map(drop)
            .map_err(|e| self.error("put", Some(full), e))
    }
}

/// What S3 and GCS say when a credential is refused, when the client
/// passes it on as a plain status.
const REFUSED: &[&str] = &[
    "SignatureDoesNotMatch",
    "InvalidAccessKeyId",
    "AccessDenied",
    "403 Forbidden",
    "401 Unauthorized",
];

/// What S3, GCS and the emulators say about a bucket that is not there.
const MISSING: &[&str] = &["NoSuchBucket", "bucket does not exist", "no bucket named"];

/// Removes the object path from a client message: a path holds an object
/// address, and an address in an error is an address in a log. Both the
/// path and its percent-encoded form, which is what a URL in the message
/// holds.
fn scrub(text: &str, path: Option<&OsPath>) -> String {
    let Some(p) = path.map(|p| p.as_ref()).filter(|p| !p.is_empty()) else {
        return text.to_string();
    };
    let mut encoded = String::with_capacity(p.len() * 3);
    for b in p.bytes() {
        if b.is_ascii_alphanumeric() {
            encoded.push(b as char);
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    text.replace(p, "<object>").replace(&encoded, "<object>")
}

/// Header-safe metadata: visible ASCII stays, `%` and everything else is
/// percent-encoded as UTF-8. A filename may hold any letter; an HTTP header
/// may not.
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        if (0x20..0x7f).contains(&b) && b != b'%' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn unescape(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[async_trait]
impl Bucket for CloudBucket {
    async fn put(
        &self,
        path: &ObjectPath,
        body: Body,
        options: PutOptions,
    ) -> Result<ObjectHead, StorageError> {
        options.check()?;
        let full = self.full(path);
        let limit = options.max_bytes;
        let over = |size: u64| {
            limit
                .filter(|l| size > *l)
                .map(|limit| StorageError::TooLarge { size, limit })
        };
        let head = |size: u64| ObjectHead {
            path: path.clone(),
            size,
            content_type: options.content_type.clone(),
            metadata: options.metadata.clone(),
        };
        let mut stream = match body {
            Body::Bytes(bytes) => {
                if let Some(e) = over(bytes.len() as u64) {
                    return Err(e);
                }
                let size = bytes.len() as u64;
                self.put_whole(&full, bytes, &options).await?;
                return Ok(head(size));
            }
            Body::Stream(stream) => stream,
        };
        // Read up to one request's worth. A stream that ends there goes up
        // whole, create-only when asked.
        let mut first: Vec<u8> = Vec::new();
        let mut size = 0u64;
        let mut spill: Option<Bytes> = None;
        while let Some(chunk) = next(&mut stream).await {
            let chunk = chunk?;
            size += chunk.len() as u64;
            if let Some(e) = over(size) {
                return Err(e);
            }
            if first.len() + chunk.len() > SINGLE_PUT_BYTES {
                spill = Some(chunk);
                break;
            }
            first.extend_from_slice(&chunk);
        }
        let Some(spill) = spill else {
            self.put_whole(&full, Bytes::from(first), &options).await?;
            return Ok(head(size));
        };
        // In parts. A multipart upload cannot be create-only, so the check
        // is a head first; `PutOptions::create_only` says so.
        if options.create_only {
            match self.head(path).await {
                Ok(_) => return Err(StorageError::Exists),
                Err(StorageError::NotFound) => {}
                Err(e) => return Err(e),
            }
        }
        let upload = self
            .store
            .put_multipart_opts(
                &full,
                PutMultipartOptions {
                    attributes: Self::attributes(&options),
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| self.error("start an upload", Some(&full), e))?;
        let mut writer = WriteMultipart::new(upload);
        writer.put(Bytes::from(first));
        writer.put(spill);
        loop {
            let chunk = match next(&mut stream).await {
                None => break,
                Some(Ok(chunk)) => chunk,
                Some(Err(e)) => {
                    let _ = writer.abort().await;
                    return Err(e);
                }
            };
            size += chunk.len() as u64;
            if let Some(e) = over(size) {
                let _ = writer.abort().await;
                return Err(e);
            }
            if let Err(e) = writer.wait_for_capacity(4).await {
                return Err(self.error("upload a part", Some(&full), e));
            }
            writer.put(chunk);
        }
        writer
            .finish()
            .await
            .map_err(|e| self.error("finish an upload", Some(&full), e))?;
        Ok(head(size))
    }

    async fn get(
        &self,
        path: &ObjectPath,
        range: Option<ByteRange>,
    ) -> Result<GetObject, StorageError> {
        let full = self.full(path);
        let options = GetOptions {
            range: range.map(|r| match r.end {
                Some(end) => GetRange::Bounded(r.start..end),
                None => GetRange::Offset(r.start),
            }),
            ..Default::default()
        };
        let result = match self.store.get_opts(&full, options).await {
            Ok(result) => result,
            Err(object_store::Error::NotFound { .. }) => return Err(StorageError::NotFound),
            // A range past the end is a 416, which the client does not
            // name the same way on every service. The head says which it
            // was: a missing object stays a 404, a range past the end is a
            // range error, and anything else is the client's error.
            Err(e) => {
                if let Some(r) = range {
                    let head = self.head(path).await?;
                    r.within(head.size)?;
                }
                return Err(self.error("get", Some(&full), e));
            }
        };
        let head = self.head_from(path, result.meta.size, &result.attributes);
        let range_read = result.range.clone();
        if let Some(r) = range {
            // The bounds the client asked for, against the size the server
            // answered: an empty answer is a range past the end.
            r.within(head.size)?;
        }
        let this = self.clone();
        let scrubbed = full.clone();
        let body: ByteStream = Box::pin(
            result
                .into_stream()
                .map(move |chunk| chunk.map_err(|e| this.error("read", Some(&scrubbed), e))),
        );
        Ok(GetObject {
            head,
            range: range_read,
            body,
        })
    }

    async fn head(&self, path: &ObjectPath) -> Result<ObjectHead, StorageError> {
        let full = self.full(path);
        // `head` alone answers no attributes; a head-only get does.
        let result = self
            .store
            .get_opts(
                &full,
                GetOptions {
                    head: true,
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| self.error("head", Some(&full), e))?;
        Ok(self.head_from(path, result.meta.size, &result.attributes))
    }

    async fn delete(&self, path: &ObjectPath) -> Result<(), StorageError> {
        let full = self.full(path);
        match self.store.delete(&full).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(e) => Err(self.error("delete", Some(&full), e)),
        }
    }

    async fn list(&self, prefix: &str) -> Result<Vec<Listed>, StorageError> {
        check_prefix(prefix)?;
        // `object_store` lists by whole segments, so list the deepest
        // directory and filter on the rest of the prefix.
        let (dir, _) = prefix.rsplit_once('/').unwrap_or(("", prefix));
        let base = match (&self.prefix, dir.is_empty()) {
            (Some(p), true) => Some(OsPath::from(p.as_str())),
            (Some(p), false) => Some(OsPath::from(format!("{p}/{dir}"))),
            (None, true) => None,
            (None, false) => Some(OsPath::from(dir)),
        };
        let mut stream = self.store.list(base.as_ref());
        let mut out = Vec::new();
        while let Some(meta) = stream.next().await {
            let meta = meta.map_err(|e| self.error("list", base.as_ref(), e))?;
            if let Some(path) = self.ours(&meta.location) {
                if path.as_str().starts_with(prefix) {
                    out.push(Listed {
                        path,
                        size: meta.size,
                    });
                }
            }
        }
        Ok(out)
    }

    async fn signed_url(
        &self,
        method: SignedMethod,
        path: &ObjectPath,
        ttl: Duration,
    ) -> Result<Option<String>, StorageError> {
        if ttl > MAX_SIGNED_TTL || ttl.is_zero() {
            return Err(StorageError::backend(
                "a signed URL must live between one second and seven days\n\
                 help: give a ttl inside that window",
            ));
        }
        let full = self.full(path);
        let method = match method {
            SignedMethod::Get => http::Method::GET,
            SignedMethod::Put => http::Method::PUT,
        };
        let url = self
            .signer
            .signed_url(method, &full, ttl)
            .await
            .map_err(|e| {
                StorageError::backend(format!(
                    "[storage] cannot sign a URL for {} bucket {:?}: {}\n\
                     help: signing needs a key: `secret_access_key_file` for S3, a service-account \
                     `credentials_file` for GCS, or a role allowed to sign blobs",
                    self.service,
                    self.bucket,
                    scrub(&e.to_string(), Some(&full))
                ))
            })?;
        Ok(Some(url.to_string()))
    }

    async fn probe(&self) -> Result<(), StorageError> {
        let base = self.prefix.as_deref().map(OsPath::from);
        let mut stream = self.store.list(base.as_ref());
        match stream.next().await {
            None | Some(Ok(_)) => Ok(()),
            Some(Err(e)) => Err(match e {
                object_store::Error::NotFound { .. } => self.missing_bucket(),
                e => self.error("probe", base.as_ref(), e),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_survives_a_header() {
        for value in [
            "plain.png",
            "ünïcødé.pdf",
            "100% done.txt",
            "a%41b",
            "日本.jpg",
        ] {
            let escaped = escape(value);
            assert!(
                escaped.bytes().all(|b| (0x20..0x7f).contains(&b)),
                "{escaped}"
            );
            assert_eq!(unescape(&escaped), value);
        }
        assert_eq!(unescape("50%"), "50%");
        assert_eq!(unescape("%zz"), "%zz");
    }

    #[test]
    fn a_client_message_loses_the_path() {
        let path = OsPath::from("objects/ab/cd/abcd");
        assert_eq!(
            scrub(
                "Object at location objects/ab/cd/abcd not found",
                Some(&path)
            ),
            "Object at location <object> not found"
        );
        assert_eq!(
            scrub(
                "PUT http://h/b/objects%2Fab%2Fcd%2Fabcd failed",
                Some(&path)
            ),
            "PUT http://h/b/<object> failed"
        );
    }

    #[test]
    fn a_prefix_is_a_path_or_nothing() {
        assert_eq!(clean_prefix(None).unwrap(), None);
        assert_eq!(clean_prefix(Some("/")).unwrap(), None);
        assert_eq!(clean_prefix(Some("app/")).unwrap().as_deref(), Some("app"));
        assert!(clean_prefix(Some("../x")).is_err());
    }
}
