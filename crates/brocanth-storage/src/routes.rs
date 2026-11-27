//! The HTTP edge of the store: an upload route, a download route, and
//! thumbnails, as request hooks. Behind the `routes` feature, which is off.
//!
//! # What answers where
//!
//! | Request | Answer |
//! |---|---|
//! | `POST {path}`, `multipart/form-data` | the guard, then the `file` part stored; `303` to `return_to`, or `201` with the address |
//! | `GET {path}/{address}?token=…` | the object, when the token opens that address; with `downloads = redirect`, a `307` to a signed URL of the bucket |
//! | `GET {path}/{address}?token=…&thumb=300x200t` | a thumbnail of it, from the cache or rendered |
//! | `GET {path}/{address}?token=…&thumb=300x200t&fmt=webp` | the thumbnail in a format [`RoutesConfig::formats`] lists |
//! | `GET {path}/{address}` | `403`, unless the routes are `public` |
//! | another verb on `{path}/{address}` | `405` |
//!
//! Anything else passes to the router untouched.
//!
//! # Why hooks and not pages
//!
//! A page answers with a `String` body, and a file is bytes. A hook answers
//! with a whole response, so an image goes out as it is stored, with its
//! entity tag and a `304` for a cached copy.
//!
//! # The decisions, each with its reason
//!
//! * **Nobody may upload until the application says who.** The
//!   [`UploadGuard`] is the application's: whether this request may store a
//!   file is a policy question, and a default "yes" is a free disk for every
//!   stranger. The generated guard answers no and says where to change it,
//!   the way the admin starts locked.
//! * **A download needs a token, unless the routes are `public`.** The token
//!   is minted by the application after its own policy said yes
//!   ([`FileTokens::issue`]); the route checks only that the token opens
//!   this address. A forged, expired and wrong token are one `403`.
//! * **The upload passes the CSRF check, and a form needs no script for it.**
//!   `middleware-csrf` reads its token from a urlencoded body or a header,
//!   never from a multipart body. The [`CsrfLift`] hook runs before the
//!   check, reads the form's token field out of an upload to `{path}`, and
//!   puts it in the header the check reads. The check itself is unchanged.
//! * **What is served inline is what the bytes are.** The type comes from
//!   the store, which sniffed it. Anything that is not a recognised image
//!   is an attachment of type `application/octet-stream`, with `nosniff`
//!   and a sandbox policy beside it. The type the client declared is not
//!   repeated: it is the client's claim, and `text/html` is a page.
//! * **A thumbnail is rendered at most once per size, and only in a size
//!   the application lists.** [`RoutesConfig::thumb_sizes`] is the list;
//!   any other `?thumb=` is a `400` before anything is read or decoded, so a
//!   token holder cannot fill the disk with one rendering per size. The
//!   first request renders and caches it; the next ones read the cache.
//!   Without the `variants` feature a thumbnail is served from the cache
//!   only.
//! * **A format is converted only on request, and only to a listed one.**
//!   `&fmt=` names it, beside `&thumb=`; [`RoutesConfig::formats`] lists
//!   the ones allowed, and it is empty unless the `image` module is chosen.
//!   The URL names the format rather than the `Accept` header, so one URL
//!   is one image and a cache in front needs no `Vary`. A source that
//!   declares more than [`RoutesConfig::max_pixels`] is a `413` before it is
//!   decoded.
//! * **Every upload gets an address of its own, unless the config dedupes.**
//!   With [`RoutesConfig::dedupe`] off (the default), an upload is stored at
//!   a random address, so one holder's `delete` never removes the file of
//!   another holder who uploaded the same bytes. Turn it on only when the
//!   application counts references before it deletes.
//! * **A name outside ASCII survives.** `Content-Disposition` carries an
//!   ASCII `filename=` and the real name in `filename*=` (RFC 6266).
//! * **A download may redirect to the bucket, when the project opts in.**
//!   With [`Downloads::Redirect`], a `GET` with a valid token answers
//!   `307` to a URL the store signs, and the bucket sends the bytes. The
//!   URL carries the same rule the proxy follows, inside its signature: a
//!   sniffed image inline as its type, anything else as an attachment of
//!   type `application/octet-stream` ([`SignedGet::for_object`]). `HEAD`,
//!   `?thumb=` and `&fmt=` keep proxying. The redirect holds
//!   `Cache-Control: private` for at most half the URL's life, and
//!   `Referrer-Policy: no-referrer`, so neither a cache nor a referrer
//!   keeps the bearer URL. A store that cannot sign (a disk, Postgres)
//!   refuses `Redirect` when the routes are built
//!   (`docs/object-storage.md` §9.3, D28 to D34).
//!
//! [`SignedGet::for_object`]: crate::SignedGet::for_object

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use brocanth::plugin_api::PluginError;
use brocanth::plugin_body::Body;
use brocanth::plugin_hooks::{AsyncRequestHook, Flow, Request, RequestState, Response};
use brocanth_input::multipart::{self, Limits, Multipart};

use crate::error::StorageError;
use crate::key::ObjectKey;
use crate::meta::{attachment, ContentType, Filename, ObjectInfo};
use crate::spec::{VariantFormat, VariantSpec};
use crate::storage::Storage;
use crate::token::{now, FileToken, FileTokens, UnixTime};

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// How the routes answer.
#[derive(Clone, Debug)]
pub struct RoutesConfig {
    /// Where the routes answer: uploads at `{path}`, files at
    /// `{path}/{address}`.
    pub path: String,
    /// The largest file accepted, in bytes.
    pub max_bytes: usize,
    /// The server's request body cap, `[server] max_body_bytes`. A
    /// `max_bytes` above it is refused at start: the server would cut the
    /// upload off with a 413 first. `None` skips the check.
    pub server_max_body: Option<usize>,
    /// The multipart field that holds the file.
    pub field: String,
    /// Serve a file to a request with no token.
    pub public: bool,
    /// Answer `?thumb=WxH`.
    pub thumbnails: bool,
    /// The only sizes `?thumb=` may ask for. Any other is a `400`.
    pub thumb_sizes: Vec<VariantSpec>,
    /// The formats `&fmt=` may convert a thumbnail to. Empty, the default,
    /// refuses every `&fmt=` with a `400`.
    pub formats: Vec<VariantFormat>,
    /// The most pixels a source may declare before a thumbnail of it is
    /// rendered: a `413` past it. At most
    /// [`MAX_SOURCE_PIXELS`](crate::MAX_SOURCE_PIXELS), the default.
    pub max_pixels: u64,
    /// Store identical uploads once, at their content address. Off, each
    /// upload is stored at a random address of its own. See the module
    /// header for why off is the default.
    pub dedupe: bool,
    /// The form field that carries the CSRF token: `[csrf] field`.
    pub csrf_field: String,
    /// The header the CSRF check reads: `[csrf] header`.
    pub csrf_header: String,
    /// How a download is answered: the bytes from this process (the
    /// default), or a redirect to a signed URL of the bucket.
    pub downloads: Downloads,
    /// How long a signed URL of a redirect lives. From one second to seven
    /// days; [`DEFAULT_SIGNED_TTL`] by default. It is a bearer credential
    /// nobody can revoke, so this is the leak window.
    pub signed_ttl: Duration,
    /// The `Content-Security-Policy` the application enforces, if any.
    /// With [`Downloads::Redirect`], the start check refuses a policy whose
    /// `img-src` does not allow the bucket's origin
    /// ([`FileRoutes::check_signing`]).
    pub csp: Option<String>,
}

/// How the download route answers a read (`docs/object-storage.md` D28).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Downloads {
    /// Read the object and send it, with `nosniff` and, for a non-image,
    /// `Content-Disposition: attachment` and `Content-Security-Policy:
    /// sandbox`. Needs nothing from the store. The default.
    #[default]
    Proxy,
    /// Answer `307` to a URL the store signs. The bytes do not pass through
    /// this process, and the bucket answers a `Range`. Needs a store that
    /// signs (S3, GCS), a private bucket, and an `img-src` that allows the
    /// bucket's origin.
    Redirect,
}

impl Downloads {
    /// `proxy` or `redirect`.
    ///
    /// # Errors
    /// Any other word, with a `help:` line.
    pub fn parse(text: &str) -> Result<Self, String> {
        match text.trim() {
            "proxy" => Ok(Self::Proxy),
            "redirect" => Ok(Self::Redirect),
            other => Err(format!(
                "[storage_routes] downloads is {other:?}, and it takes \"proxy\" or \"redirect\"\n\
                 help: set `downloads` under [storage_routes] to \"proxy\" (the bytes pass through \
                 the application) or \"redirect\" (a 307 to a signed URL of the bucket)"
            )),
        }
    }

    /// The setting's word.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proxy => "proxy",
            Self::Redirect => "redirect",
        }
    }
}

/// How long a signed URL of a redirect lives by default: five minutes.
pub const DEFAULT_SIGNED_TTL: Duration = Duration::from_secs(300);

/// The longest a signed URL may live: seven days, the limit of S3 and GCS.
pub const MAX_SIGNED_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

/// The most signed URLs one set of routes keeps. Past it, the expired ones
/// go; when none has expired, all go. A URL is about a kilobyte.
const SIGNED_CACHE_ENTRIES: usize = 4096;

/// The largest file by default: 2 MiB, well under the server's default
/// request body cap of 16 MiB.
pub const DEFAULT_MAX_BYTES: usize = 2 * 1024 * 1024;

pub use crate::spec::DEFAULT_THUMB_SIZES;

impl Default for RoutesConfig {
    fn default() -> Self {
        Self {
            path: "/files".to_string(),
            max_bytes: DEFAULT_MAX_BYTES,
            server_max_body: None,
            field: "file".to_string(),
            public: false,
            thumbnails: true,
            thumb_sizes: DEFAULT_THUMB_SIZES
                .iter()
                .map(|text| VariantSpec::parse(text).expect("a valid default size"))
                .collect(),
            formats: Vec::new(),
            max_pixels: crate::MAX_SOURCE_PIXELS,
            dedupe: false,
            csrf_field: "_csrf_token".to_string(),
            csrf_header: "x-csrf-token".to_string(),
            downloads: Downloads::Proxy,
            signed_ttl: DEFAULT_SIGNED_TTL,
            csp: None,
        }
    }
}

/// Whether a request may store a file. The application writes it.
///
/// Async, so it can ask a policy that loads a row (`brocanth-policy`). A
/// closure that answers at once is one too:
///
/// ```
/// use brocanth_storage::routes::UploadGuard;
///
/// /// What the application's session plugin puts in the state.
/// struct SignedIn;
///
/// let signed_in = |_: &http::Request<Vec<u8>>, state: &brocanth::plugin_hooks::RequestState| {
///     state.get::<SignedIn>().is_some()
/// };
/// fn takes(_: impl UploadGuard) {}
/// takes(signed_in);
/// ```
///
/// A guard that awaits implements the trait:
///
/// ```
/// use brocanth::plugin_hooks::{Request, RequestState};
/// use brocanth_storage::routes::{GuardFuture, UploadGuard};
///
/// struct Policy;
///
/// impl UploadGuard for Policy {
///     fn may_upload<'a>(&'a self, request: &'a Request, state: &'a RequestState) -> GuardFuture<'a> {
///         Box::pin(async move {
///             // Load the account from `state`, then ask the policy.
///             let _ = (request, state);
///             false
///         })
///     }
/// }
/// ```
///
/// Never treat any cookie as a sign-in: a stranger's browser sends cookies
/// too. Read what the session plugin put in `state`.
pub trait UploadGuard: Send + Sync + 'static {
    /// `true` lets the upload through. The request has passed every hook
    /// ordered before the routes, the session and the CSRF check among them.
    fn may_upload<'a>(&'a self, request: &'a Request, state: &'a RequestState) -> GuardFuture<'a>;
}

/// What [`UploadGuard::may_upload`] answers.
pub type GuardFuture<'a> = BoxFuture<'a, bool>;

impl<F> UploadGuard for F
where
    F: Fn(&Request, &RequestState) -> bool + Send + Sync + 'static,
{
    fn may_upload<'a>(&'a self, request: &'a Request, state: &'a RequestState) -> GuardFuture<'a> {
        Box::pin(std::future::ready(self(request, state)))
    }
}

struct Inner {
    store: Arc<dyn Storage>,
    tokens: FileTokens,
    config: RoutesConfig,
    guard: Box<dyn UploadGuard>,
    /// Signed URLs by address, with when each was signed. One address has
    /// one disposition, because the first upload owns the metadata, so the
    /// address is the whole key. A hit gives the browser the same URL, so
    /// its cache works across page views, and saves the GCS `signBlob`
    /// call of a store with no key.
    signed: Mutex<HashMap<ObjectKey, (String, Instant)>>,
    /// The start check passed once. `/readyz` runs a check on each probe,
    /// and a GCS store with no key would make a `signBlob` call each time.
    signing_checked: AtomicBool,
}

/// The upload and download routes, as one request hook. Cheap to clone.
#[derive(Clone)]
pub struct FileRoutes {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for FileRoutes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileRoutes")
            .field("config", &self.inner.config)
            .finish_non_exhaustive()
    }
}

impl FileRoutes {
    /// The routes over `store`, checking tokens with `tokens`, letting an
    /// upload through when `guard` says so.
    ///
    /// # Errors
    /// The path does not start with `/`, a limit is zero, or `max_bytes` is
    /// above `server_max_body`.
    pub fn new(
        store: Arc<dyn Storage>,
        tokens: FileTokens,
        mut config: RoutesConfig,
        guard: impl UploadGuard,
    ) -> Result<Self, PluginError> {
        config.path = config.path.trim_end_matches('/').to_string();
        if !config.path.starts_with('/') || config.path.len() < 2 {
            return Err(PluginError::new(format!(
                "[storage_routes] path `{}` must start with `/` and name something\n\
                 help: use `/files`",
                config.path
            )));
        }
        if config.max_bytes == 0 {
            return Err(PluginError::new(
                "[storage_routes] max_bytes is 0, so no upload is ever accepted\n\
                 help: set it to the largest file you take, in bytes",
            ));
        }
        if let Some(cap) = config.server_max_body.filter(|cap| config.max_bytes > *cap) {
            return Err(PluginError::new(format!(
                "[storage_routes] max_bytes is {}, above the server's request body cap of {cap} bytes, so a file that large gets 413 before it reaches the routes\n\
                 help: lower `max_bytes` under [storage_routes] to at most {cap}, or raise `max_body_bytes` under [server]",
                config.max_bytes
            )));
        }
        if config.thumbnails && config.thumb_sizes.is_empty() {
            return Err(PluginError::new(
                "[storage_routes] thumbnails are on and thumb_sizes is empty, so every thumbnail is refused\n\
                 help: list the sizes the pages use, such as [\"150x150t\", \"300x200\"], or turn thumbnails off",
            ));
        }
        if config.max_pixels == 0 || config.max_pixels > crate::MAX_SOURCE_PIXELS {
            return Err(PluginError::new(format!(
                "[storage_routes] max_pixels is {}, outside 1 to {}, the most the decoder is sized for\n\
                 help: set `max_pixels` under [storage_routes] to at most {}",
                config.max_pixels,
                crate::MAX_SOURCE_PIXELS,
                crate::MAX_SOURCE_PIXELS
            )));
        }
        if !config.formats.is_empty() && !config.thumbnails {
            return Err(PluginError::new(
                "[storage_routes] formats lists a format and thumbnails are off, so `&fmt=` has no thumbnail to convert\n\
                 help: empty `formats`, or turn thumbnails on",
            ));
        }
        if config.signed_ttl < Duration::from_secs(1) || config.signed_ttl > MAX_SIGNED_TTL {
            return Err(PluginError::new(format!(
                "[storage_routes] signed_ttl is {} seconds, outside 1 to {}, the life S3 and GCS allow a signed URL\n\
                 help: set `signed_ttl` under [storage_routes] to a number of seconds in that window, such as 300",
                config.signed_ttl.as_secs(),
                MAX_SIGNED_TTL.as_secs()
            )));
        }
        if config.downloads == Downloads::Redirect && !store.can_sign() {
            return Err(PluginError::new(
                "[storage_routes] downloads is \"redirect\", and the file store cannot sign a URL: a disk or Postgres store has no URL to redirect to\n\
                 help: set `downloads = \"proxy\"` under [storage_routes], or move the files to a bucket with `brocanth add storage-s3` (or `storage-gcs`)",
            ));
        }
        Ok(Self {
            inner: Arc::new(Inner {
                store,
                tokens,
                config,
                guard: Box::new(guard),
                signed: Mutex::new(HashMap::new()),
                signing_checked: AtomicBool::new(false),
            }),
        })
    }

    /// Whether a download redirects to a signed URL.
    pub fn redirects(&self) -> bool {
        self.inner.config.downloads == Downloads::Redirect
    }

    /// The start check of a redirecting route (`docs/object-storage.md`
    /// D34): the store signs one URL, and an enforced
    /// Content-Security-Policy allows the bucket's origin in `img-src`.
    /// Register it with `Lifecycle::ready_when`, so the start fails before
    /// the first image fails in a browser. With [`Downloads::Proxy`] it
    /// answers `Ok` and sends nothing. Once it passed, it answers `Ok` and
    /// signs nothing more.
    ///
    /// # Errors
    /// What failed, with a `help:` line. A missing `img-src` source names
    /// the value to add.
    pub async fn check_signing(&self) -> Result<(), String> {
        if !self.redirects() || self.inner.signing_checked.load(Ordering::Acquire) {
            return Ok(());
        }
        let origin = match self.inner.store.signing_origin().await {
            Ok(Some(origin)) => origin,
            Ok(None) => {
                return Err(
                    "downloads is \"redirect\", and the file store signs no URL\n\
                     help: set `downloads = \"proxy\"` under [storage_routes]"
                        .to_string(),
                )
            }
            Err(e) => {
                return Err(format!(
                    "downloads is \"redirect\", and the file store cannot sign a URL: {e}"
                ))
            }
        };
        if let Some(csp) = &self.inner.config.csp {
            if !img_src_allows(csp, &origin) {
                return Err(format!(
                    "downloads is \"redirect\", so images come from {origin}, and the Content-Security-Policy `img-src` does not allow it: every redirected image would fail in the browser\n\
                     help: add {origin:?} to `img-src`: under [security_headers.csp.allow], set img-src = [{origin:?}]"
                ));
            }
        }
        self.inner.signing_checked.store(true, Ordering::Release);
        Ok(())
    }

    /// The hook that lets a plain form's CSRF field reach the CSRF check.
    /// Order it before `csrf.check`.
    pub fn csrf_lift(&self) -> CsrfLift {
        CsrfLift {
            routes: self.clone(),
        }
    }

    /// The path the routes answer under, without a trailing slash.
    pub fn path(&self) -> &str {
        &self.inner.config.path
    }

    /// The URL a template puts in `src` or `href` for this address: the
    /// download route with a fresh token.
    pub fn url(&self, key: &ObjectKey) -> String {
        format!(
            "{}/{}?token={}",
            self.inner.config.path,
            key.to_hex(),
            self.inner.tokens.issue(key).as_str()
        )
    }

    /// The link to a thumbnail of `key`: [`thumb_url`] with this
    /// application's path and a new token.
    pub fn thumb_url(&self, key: &ObjectKey, size: VariantSpec) -> String {
        let token = self.inner.tokens.issue(key);
        thumb_url(&self.inner.config.path, key, &token, size)
    }

    fn address_of<'r>(&self, path: &'r str) -> Option<&'r str> {
        let rest = path.strip_prefix(self.inner.config.path.as_str())?;
        let name = rest.strip_prefix('/')?;
        (!name.is_empty() && !name.contains('/')).then_some(name)
    }

    fn limits(&self) -> Limits {
        let max = self.inner.config.max_bytes;
        Limits {
            file_bytes: max,
            // The file, plus room for the text fields and the boundaries.
            total_bytes: max.saturating_add(64 * 1024),
            parts: 16,
            ..Limits::DEFAULT
        }
    }

    /// Answers one upload. The request is a `POST` to the upload path.
    pub async fn upload(&self, request: &Request, state: &RequestState) -> Response {
        let config = &self.inner.config;
        if !self.inner.guard.may_upload(request, state).await {
            return plain(
                403,
                "this application takes no upload from this request\n\
                 help: the application decides who may upload in its UploadGuard",
            );
        }
        let sent = match read_multipart(request, self.limits()) {
            Ok(sent) => sent,
            Err(answer) => return *answer,
        };
        let Some(file) = sent.file(&config.field) else {
            return plain(
                400,
                &format!("the form has no file in the `{}` field", config.field),
            );
        };
        if file.is_empty() {
            return plain(400, "the file is empty");
        }
        let mut about = ObjectInfo::new();
        if let Some(name) = file.filename() {
            about = about.with_filename(name);
        }
        if let Some(declared) = file.declared_type() {
            about = about.with_declared_type(declared);
        }
        let stored = if config.dedupe {
            self.inner.store.put(file.bytes(), &about).await
        } else {
            // An address of its own: see the module header.
            match ObjectKey::random() {
                Ok(key) => self
                    .inner
                    .store
                    .put_at(&key, file.bytes(), &about)
                    .await
                    .map(|()| key),
                Err(e) => Err(e),
            }
        };
        let key = match stored {
            Ok(key) => key,
            Err(e) => return failed(&e),
        };
        let token = self.inner.tokens.issue(&key);
        let hex = key.to_hex();
        let location = format!("{}/{hex}?token={}", config.path, token.as_str());
        match sent.field("return_to").filter(|to| is_local(to)) {
            Some(to) => {
                // The query goes before a fragment: after it, the browser
                // keeps it on the client and the page never reads it.
                let (to, fragment) = to.split_at(to.find('#').unwrap_or(to.len()));
                let joint = if to.contains('?') { '&' } else { '?' };
                let back = format!("{to}{joint}file={hex}&token={}{fragment}", token.as_str());
                let mut answer = plain(303, "");
                set(&mut answer, http::header::LOCATION, &back);
                answer
            }
            None => {
                let mut answer = plain(201, &hex);
                set(&mut answer, http::header::LOCATION, &location);
                answer
            }
        }
    }

    /// Answers one download of `address`, with the query of the request.
    pub async fn download(&self, request: &Request, address: &str) -> Response {
        let config = &self.inner.config;
        let Ok(key) = ObjectKey::parse(address) else {
            return plain(404, "no file is stored at that address");
        };
        let query = request.uri().query().unwrap_or("");
        let token = param(query, "token");
        let token_expires = match token {
            Some(token) => match self.inner.tokens.verify_until(token) {
                Ok((opens, expires)) if opens == key => Some(expires),
                _ => {
                    return plain(
                        403,
                        "the link expired or is not valid; ask the page for a new one",
                    )
                }
            },
            None if config.public => None,
            None => return plain(403, "this file needs a link from the page that shows it"),
        };
        let thumb = match param(query, "thumb") {
            None => None,
            Some(_) if !config.thumbnails => {
                return plain(404, "this application serves no thumbnail")
            }
            Some(text) => match VariantSpec::parse(text) {
                Ok(spec) if config.thumb_sizes.contains(&spec) => Some(spec),
                Ok(_) => {
                    return plain(
                        400,
                        &format!(
                            "this application serves no thumbnail of that size\n\
                             help: ask for one of {}",
                            sizes(&config.thumb_sizes)
                        ),
                    )
                }
                Err(e) => return plain(400, &e.to_string()),
            },
        };
        let thumb = match (thumb, param(query, "fmt")) {
            (thumb, None) => thumb,
            (None, Some(_)) => {
                return plain(
                    400,
                    "`fmt` converts a thumbnail, and this request names none\n\
                     help: add `&thumb=` with a size the application lists",
                )
            }
            (Some(spec), Some(text)) => match VariantFormat::parse(text) {
                Ok(format) if config.formats.contains(&format) => {
                    Some(spec.with_format(Some(format)))
                }
                Ok(_) if config.formats.is_empty() => {
                    return plain(
                        400,
                        "this application converts no thumbnail to another format",
                    )
                }
                Ok(_) => {
                    let listed: Vec<&str> = config.formats.iter().map(|f| f.as_str()).collect();
                    return plain(
                        400,
                        &format!(
                            "this application converts no thumbnail to that format\n\
                             help: ask for one of {}",
                            listed.join(", ")
                        ),
                    );
                }
                Err(e) => return plain(400, &e.to_string()),
            },
        };
        // A redirect for a whole object read with GET. HEAD, a thumbnail
        // and a conversion keep proxying (D30): a URL signed for GET
        // refuses HEAD, and a thumbnail is small and this process's own.
        if thumb.is_none() && self.redirects() && request.method() == http::Method::GET {
            return self.redirect(&key, token_expires).await;
        }
        let etag = match thumb {
            Some(spec) => format!("\"{}-{spec}\"", key.to_hex()),
            None => key.etag(),
        };
        let cache = if token.is_some() {
            // No longer than the token, which the cache would outlive.
            format!("private, max-age={}", self.inner.tokens.ttl())
        } else {
            // The address is the content: it never changes.
            "public, max-age=31536000, immutable".to_string()
        };
        if request
            .headers()
            .get(http::header::IF_NONE_MATCH)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|sent| sent.split(',').any(|t| t.trim() == etag))
        {
            let mut answer = plain(304, "");
            answer.headers_mut().remove(http::header::CONTENT_TYPE);
            set(&mut answer, http::header::ETAG, &etag);
            set(&mut answer, http::header::CACHE_CONTROL, &cache);
            return answer;
        }
        let (bytes, content_type, filename) = match thumb {
            Some(spec) => match self.variant(&key, spec).await {
                Ok(found) => found,
                Err(answer) => return *answer,
            },
            None => {
                let meta = match self.inner.store.meta(&key).await {
                    Ok(meta) => meta,
                    Err(e) => return failed(&e),
                };
                let bytes = match self.inner.store.get(&key).await {
                    Ok(bytes) => bytes,
                    Err(e) => return failed(&e),
                };
                (bytes, meta.content_type, meta.filename)
            }
        };
        // A type that is not a sniffed image is the client's claim, so it is
        // not what the answer says: `text/html` from an upload form would
        // be a page on this origin wherever the attachment header is lost.
        let content_type = if content_type.is_safe_to_inline() {
            content_type
        } else {
            ContentType::octet_stream()
        };
        let head = request.method() == http::Method::HEAD;
        let length = bytes.len();
        let mut answer = http::Response::new(Body::from(if head { Vec::new() } else { bytes }));
        set(
            &mut answer,
            http::header::CONTENT_TYPE,
            content_type.as_str(),
        );
        set(
            &mut answer,
            http::header::CONTENT_LENGTH,
            &length.to_string(),
        );
        set(&mut answer, http::header::ETAG, &etag);
        set(&mut answer, http::header::CACHE_CONTROL, &cache);
        set(&mut answer, http::header::X_CONTENT_TYPE_OPTIONS, "nosniff");
        if !content_type.is_safe_to_inline() {
            set(
                &mut answer,
                http::header::CONTENT_DISPOSITION,
                &attachment(filename.as_ref()),
            );
            // Opened directly, it still runs nothing.
            set(
                &mut answer,
                http::header::CONTENT_SECURITY_POLICY,
                "sandbox",
            );
        }
        answer
    }

    /// `307` to a signed URL of the object (D29, D32).
    async fn redirect(&self, key: &ObjectKey, token_expires: Option<UnixTime>) -> Response {
        let ttl = self.inner.config.signed_ttl;
        // A cached URL is at most half its life old, so a browser that
        // keeps the redirect for the other half never follows a dead URL.
        let half = ttl / 2;
        let url = match self.cached(key, half) {
            Some(url) => url,
            None => match self.inner.store.signed_url(key, ttl).await {
                Ok(Some(url)) => {
                    self.remember(key, &url);
                    url
                }
                // `new` refused a store that cannot sign.
                Ok(None) => return plain(500, "the file store signs no URL"),
                Err(e) => return failed(&e),
            },
        };
        let mut max_age = half.as_secs();
        if let Some(expires) = token_expires {
            max_age = max_age.min(u64::try_from(expires.saturating_sub(now())).unwrap_or(0));
        }
        let mut answer = plain(307, "");
        answer.headers_mut().remove(http::header::CONTENT_TYPE);
        set(&mut answer, http::header::LOCATION, &url);
        set(
            &mut answer,
            http::header::CACHE_CONTROL,
            &format!("private, max-age={max_age}"),
        );
        // The URL is a bearer credential: no page the bucket's answer
        // opens may learn it from a `Referer`.
        set(&mut answer, http::header::REFERRER_POLICY, "no-referrer");
        answer
    }

    fn cached(&self, key: &ObjectKey, fresh_for: Duration) -> Option<String> {
        let signed = self.inner.signed.lock().ok()?;
        let (url, at) = signed.get(key)?;
        (at.elapsed() < fresh_for).then(|| url.clone())
    }

    fn remember(&self, key: &ObjectKey, url: &str) {
        let Ok(mut signed) = self.inner.signed.lock() else {
            return;
        };
        if signed.len() >= SIGNED_CACHE_ENTRIES {
            let fresh_for = self.inner.config.signed_ttl / 2;
            signed.retain(|_, (_, at)| at.elapsed() < fresh_for);
            if signed.len() >= SIGNED_CACHE_ENTRIES {
                signed.clear();
            }
        }
        signed.insert(*key, (url.to_string(), Instant::now()));
    }

    async fn variant(
        &self,
        key: &ObjectKey,
        spec: VariantSpec,
    ) -> Result<(Vec<u8>, ContentType, Option<Filename>), Refusal> {
        #[cfg(feature = "variants")]
        {
            match crate::variants::derive_variant_with(
                self.inner.store.as_ref(),
                key,
                spec,
                self.inner.config.max_pixels,
            )
            .await
            {
                Ok(variant) => Ok((variant.bytes, variant.content_type, None)),
                Err(StorageError::NotAnImage) => Err(refuse(
                    415,
                    "the file is not an image, so it has no thumbnail",
                )),
                Err(StorageError::ImageTooLarge { .. }) => {
                    Err(refuse(413, "the image is too large to make a thumbnail of"))
                }
                Err(e) => Err(Box::new(failed(&e))),
            }
        }
        #[cfg(not(feature = "variants"))]
        {
            match self.inner.store.get_variant(key, spec).await {
                Ok(Some(bytes)) => {
                    let content_type =
                        ContentType::sniff(&bytes).unwrap_or_else(ContentType::octet_stream);
                    Ok((bytes, content_type, None))
                }
                Ok(None) => Err(refuse(
                    404,
                    "no thumbnail of that size is cached, and this build renders none\n\
                     help: turn on the `variants` feature of brocanth-storage",
                )),
                Err(e) => Err(Box::new(failed(&e))),
            }
        }
    }
}

impl AsyncRequestHook for FileRoutes {
    fn before_dispatch<'a>(
        &'a self,
        request: &'a mut Request,
        state: &'a mut RequestState,
    ) -> BoxFuture<'a, Result<Flow, PluginError>> {
        Box::pin(async move {
            let path = request.uri().path();
            if path.trim_end_matches('/') == self.inner.config.path {
                if request.method() == http::Method::POST {
                    return Ok(Flow::respond(self.upload(request, state).await));
                }
                // `GET {path}` is the application's: a page may list files.
                return Ok(Flow::Continue);
            }
            let Some(address) = self.address_of(path) else {
                return Ok(Flow::Continue);
            };
            let address = address.to_string();
            if request.method() != http::Method::GET && request.method() != http::Method::HEAD {
                let mut answer = plain(405, "a file is read with GET");
                set(&mut answer, http::header::ALLOW, "GET, HEAD");
                return Ok(Flow::respond(answer));
            }
            Ok(Flow::respond(self.download(request, &address).await))
        })
    }
}

/// Copies the CSRF field of an upload form into the header the CSRF check
/// reads. See the module header for why.
#[derive(Clone)]
pub struct CsrfLift {
    routes: FileRoutes,
}

impl CsrfLift {
    /// Does the copy on one request. Changes nothing when the request is not
    /// a multipart `POST` to the upload path, when the header is already
    /// there, or when the form has no such field.
    pub fn lift(&self, request: &mut Request) {
        let config = &self.routes.inner.config;
        if request.method() != http::Method::POST
            || request.uri().path().trim_end_matches('/') != config.path
            || request.headers().contains_key(config.csrf_header.as_str())
        {
            return;
        }
        // A file past `max_bytes` still lets its token through: the upload
        // hook then answers 413, which says more than the CSRF check's 403.
        let limits = self.routes.limits();
        let limits = Limits {
            file_bytes: limits.total_bytes,
            ..limits
        };
        let Ok(sent) = read_multipart_fields(request, limits) else {
            return;
        };
        let Some(token) = sent.field(&config.csrf_field) else {
            return;
        };
        if let (Ok(name), Ok(value)) = (
            http::HeaderName::from_bytes(config.csrf_header.as_bytes()),
            http::HeaderValue::from_str(token),
        ) {
            request.headers_mut().insert(name, value);
        }
    }
}

impl AsyncRequestHook for CsrfLift {
    fn before_dispatch<'a>(
        &'a self,
        request: &'a mut Request,
        _state: &'a mut RequestState,
    ) -> BoxFuture<'a, Result<Flow, PluginError>> {
        self.lift(request);
        Box::pin(async { Ok(Flow::Continue) })
    }
}

fn boundary_of(request: &Request) -> Result<String, Refusal> {
    let content_type = request
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let essence = content_type.split(';').next().unwrap_or("").trim();
    if !essence.eq_ignore_ascii_case("multipart/form-data") {
        return Err(refuse(
            415,
            "an upload is a multipart/form-data body\nhelp: add enctype=\"multipart/form-data\" to the form",
        ));
    }
    multipart::boundary(content_type)
        .map(str::to_string)
        .ok_or_else(|| refuse(400, "the content type names no boundary"))
}

fn read_multipart(request: &Request, limits: Limits) -> Result<Multipart, Refusal> {
    let boundary = boundary_of(request)?;
    Multipart::parse_with(request.body(), &boundary, limits)
        .map_err(|fault| refuse(fault.status(), &fault.to_string()))
}

fn read_multipart_fields(request: &Request, limits: Limits) -> Result<Multipart, Refusal> {
    let boundary = boundary_of(request)?;
    Multipart::fields_only(request.body(), &boundary, limits)
        .map_err(|fault| refuse(fault.status(), &fault.to_string()))
}

/// A refusal, boxed: a `Response` is large, and most results are `Ok`.
type Refusal = Box<Response>;

fn refuse(status: u16, message: &str) -> Refusal {
    Box::new(plain(status, message))
}

/// A path on this site: one leading `/`, not `//`, no backslash. Anything
/// else is an open redirect.
fn is_local(to: &str) -> bool {
    to.starts_with('/')
        && !to.starts_with("//")
        && !to.contains('\\')
        && !to.chars().any(char::is_control)
}

/// One query parameter, undecoded: a token and a size are URL-safe.
fn param<'q>(query: &'q str, name: &str) -> Option<&'q str> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name && !v.is_empty()).then_some(v)
    })
}

fn failed(error: &StorageError) -> Response {
    match error {
        StorageError::NotFound | StorageError::InvalidKey => {
            plain(404, "no file is stored at that address")
        }
        StorageError::TooLarge { .. } => plain(413, &error.to_string()),
        _ => plain(500, "the file store failed; try again"),
    }
}

/// The link to a thumbnail: `{path}/{address}?token=…&thumb=300x200t`, and
/// `&fmt=webp` when `size` names a format
/// ([`VariantSpec::with_format`]).
///
/// The size must be one the routes list, and the format one
/// [`RoutesConfig::formats`] lists, or the link answers `400`.
pub fn thumb_url(path: &str, key: &ObjectKey, token: &FileToken, size: VariantSpec) -> String {
    let bare = size.with_format(None);
    let mut url = format!(
        "{path}/{}?token={}&thumb={bare}",
        key.to_hex(),
        token.as_str()
    );
    if let Some(format) = size.format() {
        url.push_str("&fmt=");
        url.push_str(format.as_str());
    }
    url
}

/// A `srcset` value: one [`thumb_url`] per size, each with its width, as
/// `<img srcset>` reads it: `/files/a?…&thumb=150x150t 150w, …`.
///
/// A size with no fixed width (`0xH`) has no width descriptor to give, so
/// it is left out.
pub fn srcset(path: &str, key: &ObjectKey, token: &FileToken, sizes: &[VariantSpec]) -> String {
    sizes
        .iter()
        .filter(|size| size.width() > 0)
        .map(|size| format!("{} {}w", thumb_url(path, key, token, *size), size.width()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The sizes, as a reader writes them: `150x150t, 300x200`.
fn sizes(specs: &[VariantSpec]) -> String {
    specs
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether a Content-Security-Policy lets an `<img>` load from `origin`
/// (`scheme://host[:port]`): its `img-src`, or `default-src` without one,
/// or no such directive at all. After a redirect a source's path is not
/// compared (CSP 3), so only the scheme, the host and the port count.
fn img_src_allows(policy: &str, origin: &str) -> bool {
    let mut img = None;
    let mut default = None;
    for directive in policy.split(';') {
        let mut words = directive.split_whitespace();
        let Some(name) = words.next() else { continue };
        let sources: Vec<&str> = words.collect();
        if name.eq_ignore_ascii_case("img-src") && img.is_none() {
            img = Some(sources);
        } else if name.eq_ignore_ascii_case("default-src") && default.is_none() {
            default = Some(sources);
        }
    }
    let Some(sources) = img.or(default) else {
        return true;
    };
    sources.iter().any(|source| source_allows(source, origin))
}

/// One CSP source against an origin. `'self'` never matches: the bucket is
/// another origin.
fn source_allows(source: &str, origin: &str) -> bool {
    let Some((scheme, authority)) = origin.split_once("://") else {
        return false;
    };
    let web = scheme == "http" || scheme == "https";
    let source = source.to_ascii_lowercase();
    if source == "*" {
        return web;
    }
    if source.starts_with('\'') {
        return false;
    }
    // A scheme matches itself, and `http` lets `https` through.
    let scheme_ok = |s: &str| s == scheme || (s == "http" && scheme == "https");
    if let Some(s) = source.strip_suffix(':') {
        if !s.contains(['/', '.']) {
            return scheme_ok(s);
        }
    }
    let (source_scheme, rest) = match source.split_once("://") {
        Some((s, rest)) => (Some(s), rest),
        None => (None, source.as_str()),
    };
    match source_scheme {
        Some(s) if !scheme_ok(s) => return false,
        None if !web => return false,
        _ => {}
    }
    let host_port = rest.split('/').next().unwrap_or("");
    let (source_host, source_port) = split_port(host_port);
    let (host, port) = split_port(authority);
    let host_ok = match source_host.strip_prefix("*.") {
        Some(parent) => host.len() > parent.len() + 1 && host.ends_with(&format!(".{parent}")),
        None => source_host == host,
    };
    let default_port = if scheme == "https" { "443" } else { "80" };
    let port = port.unwrap_or(default_port);
    let port_ok = match source_port {
        Some("*") => true,
        Some(p) => p == port,
        None => port == default_port,
    };
    host_ok && port_ok
}

/// `host:port` as its parts, when what follows the last `:` is a port.
fn split_port(text: &str) -> (&str, Option<&str>) {
    match text.rsplit_once(':') {
        Some((host, port))
            if port == "*" || (!port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())) =>
        {
            (host, Some(port))
        }
        _ => (text, None),
    }
}

fn set(answer: &mut Response, name: http::HeaderName, value: &str) {
    if let Ok(value) = http::HeaderValue::from_str(value) {
        answer.headers_mut().insert(name, value);
    }
}

fn plain(status: u16, message: &str) -> Response {
    let mut answer = http::Response::new(Body::from(message.to_string()));
    *answer.status_mut() =
        http::StatusCode::from_u16(status).unwrap_or(http::StatusCode::INTERNAL_SERVER_ERROR);
    set(
        &mut answer,
        http::header::CONTENT_TYPE,
        "text/plain; charset=utf-8",
    );
    answer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_path_on_this_site_is_a_return_address() {
        assert!(is_local("/posts/new"));
        assert!(is_local("/posts?draft=1"));
        assert!(!is_local("//evil.example"));
        assert!(!is_local("https://evil.example"));
        assert!(!is_local("/\\evil.example"));
        assert!(!is_local("/a\r\nset-cookie: x"));
    }

    #[test]
    fn a_name_outside_ascii_goes_out_twice() {
        assert_eq!(
            attachment(Filename::sanitise("report.pdf").as_ref()),
            "attachment; filename=\"report.pdf\""
        );
        let header = attachment(Filename::sanitise("résumé 2.pdf").as_ref());
        assert_eq!(
            header,
            "attachment; filename=\"r_sum_ 2.pdf\"; filename*=UTF-8''r%C3%A9sum%C3%A9%202.pdf"
        );
        assert!(http::HeaderValue::from_str(&header).is_ok());
        assert_eq!(attachment(None), "attachment; filename=\"download\"");
    }

    #[test]
    fn img_src_is_read_as_a_browser_reads_it() {
        let bucket = "https://b.s3.eu-west-1.amazonaws.com";
        // The default policy of `middleware-security-headers`.
        let base = "default-src 'self'; img-src 'self' data:; object-src 'none'";
        assert!(!img_src_allows(base, bucket));
        assert!(img_src_allows(
            &format!("default-src 'self'; img-src 'self' data: {bucket}"),
            bucket
        ));
        // No img-src: default-src decides; neither: anything loads.
        assert!(!img_src_allows("default-src 'self'", bucket));
        assert!(img_src_allows("default-src https:", bucket));
        assert!(img_src_allows("object-src 'none'", bucket));
        // Wildcards, schemes, ports and paths.
        assert!(img_src_allows("img-src *", bucket));
        assert!(img_src_allows("img-src https://*.amazonaws.com", bucket));
        assert!(!img_src_allows(
            "img-src https://*.b.s3.eu-west-1.amazonaws.com",
            bucket
        ));
        assert!(img_src_allows(
            "img-src b.s3.eu-west-1.amazonaws.com",
            bucket
        ));
        assert!(img_src_allows(
            "img-src https://b.s3.eu-west-1.amazonaws.com/objects/",
            bucket
        ));
        assert!(img_src_allows(
            "img-src https://b.s3.eu-west-1.amazonaws.com:443",
            bucket
        ));
        assert!(!img_src_allows(
            "img-src https://b.s3.eu-west-1.amazonaws.com:8443",
            bucket
        ));
        assert!(!img_src_allows("img-src http://other.example", bucket));
        let minio = "http://localhost:9000";
        assert!(img_src_allows("img-src http://localhost:9000", minio));
        assert!(!img_src_allows("img-src http://localhost", minio));
        assert!(img_src_allows("img-src http://localhost:*", minio));
        assert!(!img_src_allows("img-src https://localhost:9000", minio));
        assert!(!img_src_allows("img-src 'none'", minio));
    }

    #[test]
    fn a_parameter_is_read_by_its_whole_name() {
        assert_eq!(param("token=a.b&thumb=10x10", "thumb"), Some("10x10"));
        assert_eq!(param("xtoken=a&token=b", "token"), Some("b"));
        assert_eq!(param("token=", "token"), None);
        assert_eq!(param("", "token"), None);
    }
}
