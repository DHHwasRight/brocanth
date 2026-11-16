//! The HTTP edge of the store: an upload route, a download route, and
//! thumbnails, as request hooks. Behind the `routes` feature, which is off.
//!
//! # What answers where
//!
//! | Request | Answer |
//! |---|---|
//! | `POST {path}`, `multipart/form-data` | the guard, then the `file` part stored; `303` to `return_to`, or `201` with the address |
//! | `GET {path}/{address}?token=…` | the object, when the token opens that address |
//! | `GET {path}/{address}?token=…&thumb=300x200t` | a thumbnail of it, from the cache or rendered |
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
//! * **Every upload gets an address of its own, unless the config dedupes.**
//!   With [`RoutesConfig::dedupe`] off (the default), an upload is stored at
//!   a random address, so one holder's `delete` never removes the file of
//!   another holder who uploaded the same bytes. Turn it on only when the
//!   application counts references before it deletes.
//! * **A name outside ASCII survives.** `Content-Disposition` carries an
//!   ASCII `filename=` and the real name in `filename*=` (RFC 6266).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use brocanth::plugin_api::PluginError;
use brocanth::plugin_body::Body;
use brocanth::plugin_hooks::{AsyncRequestHook, Flow, Request, RequestState, Response};
use brocanth_input::multipart::{self, Limits, Multipart};

use crate::error::StorageError;
use crate::key::ObjectKey;
use crate::meta::{ContentType, ObjectInfo};
use crate::spec::VariantSpec;
use crate::storage::Storage;
use crate::token::FileTokens;

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
    /// Store identical uploads once, at their content address. Off, each
    /// upload is stored at a random address of its own. See the module
    /// header for why off is the default.
    pub dedupe: bool,
    /// The form field that carries the CSRF token: `[csrf] field`.
    pub csrf_field: String,
    /// The header the CSRF check reads: `[csrf] header`.
    pub csrf_header: String,
}

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
            dedupe: false,
            csrf_field: "_csrf_token".to_string(),
            csrf_header: "x-csrf-token".to_string(),
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
        Ok(Self {
            inner: Arc::new(Inner {
                store,
                tokens,
                config,
                guard: Box::new(guard),
            }),
        })
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
        match token {
            Some(token) => match self.inner.tokens.verify(token) {
                Ok(opens) if opens == key => {}
                _ => {
                    return plain(
                        403,
                        "the link expired or is not valid; ask the page for a new one",
                    )
                }
            },
            None if config.public => {}
            None => return plain(403, "this file needs a link from the page that shows it"),
        }
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
                (
                    bytes,
                    meta.content_type,
                    meta.filename.map(|f| f.as_str().to_string()),
                )
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
            let name = filename.unwrap_or_else(|| "download".to_string());
            set(
                &mut answer,
                http::header::CONTENT_DISPOSITION,
                &content_disposition(&name),
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

    async fn variant(
        &self,
        key: &ObjectKey,
        spec: VariantSpec,
    ) -> Result<(Vec<u8>, ContentType, Option<String>), Refusal> {
        #[cfg(feature = "variants")]
        {
            match crate::variants::derive_variant(self.inner.store.as_ref(), key, spec).await {
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

/// The sizes, as a reader writes them: `150x150t, 300x200`.
fn sizes(specs: &[VariantSpec]) -> String {
    specs
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// `attachment` with the name twice (RFC 6266): `filename=` in ASCII, every
/// other character as `_`, for an old client, and `filename*=` with the
/// whole name, percent-encoded in UTF-8 (RFC 5987). The name is a
/// [`Filename`](crate::Filename): no quote, no backslash, no control.
fn content_disposition(name: &str) -> String {
    let ascii: String = name
        .chars()
        .map(|c| if c.is_ascii() { c } else { '_' })
        .collect();
    if ascii == name {
        return format!("attachment; filename=\"{name}\"");
    }
    let mut encoded = String::with_capacity(name.len() * 3);
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) {
            encoded.push(char::from(b));
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
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
            content_disposition("report.pdf"),
            "attachment; filename=\"report.pdf\""
        );
        let header = content_disposition("résumé 2.pdf");
        assert_eq!(
            header,
            "attachment; filename=\"r_sum_ 2.pdf\"; filename*=UTF-8''r%C3%A9sum%C3%A9%202.pdf"
        );
        assert!(http::HeaderValue::from_str(&header).is_ok());
    }

    #[test]
    fn a_parameter_is_read_by_its_whole_name() {
        assert_eq!(param("token=a.b&thumb=10x10", "thumb"), Some("10x10"));
        assert_eq!(param("xtoken=a&token=b", "token"), Some("b"));
        assert_eq!(param("token=", "token"), None);
        assert_eq!(param("", "token"), None);
    }
}
