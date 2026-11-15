//! Incoming webhooks: a request hook that verifies, deduplicates and hands
//! a typed event to the handler the application wrote.
//!
//! # Why a hook and not a page
//!
//! A webhook is a POST from a server, with no cookie and no CSRF token. A
//! page would sit behind `middleware-csrf`, which refuses it, and the fix
//! would be an exempt path that turns the check off for every POST under
//! it, signed or not. The [`Receiver`] is a request hook ordered before
//! `csrf.check`, and it answers every request it matches: a delivery that
//! fails the signature is refused here and never reaches the CSRF hook,
//! the router or a page. Nothing is exempted.
//!
//! # What a delivery goes through
//!
//! 1. The path is `{path}/{source}` and the verb POST, or the hook lets the
//!    request pass. Another verb on that path is a 405.
//! 2. The source is one the application named, or 404.
//! 3. The body is at most `max_bytes`, or 413.
//! 4. The signature is valid and, for a scheme with a timestamp, inside
//!    the replay window, or 401. See [`crate::sign`].
//! 5. The body decodes into the handler's event type, or 400.
//! 6. The idempotency key is claimed for a short lease ([`LEASE`]). A copy
//!    of a delivery that ran is answered 200 with nothing run; a copy that
//!    arrives while the first still runs is answered 409, so the sender
//!    retries it later. See [`crate::seen`].
//! 7. The handler runs. `Ok` is 200 and keeps the key for the whole window;
//!    `Err` gives the key back and answers the rejection's status, 500 by
//!    default, so the sender retries. A handler that panics, or a request
//!    that is dropped mid-way, gives the key back too.
//!
//! # What the sender is told
//!
//! A fixed, short body per status. Why a signature was refused, and what a
//! store or a handler's `?` failed with, goes to the log through `tracing`,
//! never to the sender: the reason helps a forger, and an error's text can
//! name a table or a host. [`Answer::detail`] holds it for the log and for
//! tests.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use brocanth::plugin_api::PluginError;
use brocanth::plugin_body::Body;
use brocanth::plugin_hooks::{AsyncRequestHook, Flow, Request, RequestState};
use serde::de::DeserializeOwned;

use crate::seen::{Claim, MemorySeen, Seen, DEFAULT_KEEP, LEASE, MAX_KEEP};
use crate::sign::{digest_hex, Scheme, DEFAULT_TOLERANCE};

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The largest body a delivery may have by default: one MiB. Stripe's
/// largest events are well under it.
pub const DEFAULT_MAX_BYTES: usize = 1024 * 1024;

/// What the application writes for one source: a typed event and what to
/// do with it.
///
/// ```
/// use brocanth_webhooks::{Delivery, Handler, Rejection};
///
/// #[derive(serde::Deserialize)]
/// struct Paid {
///     id: String,
///     amount: i64,
/// }
///
/// struct Payments;
///
/// impl Handler for Payments {
///     type Event = Paid;
///
///     async fn handle(&self, event: Paid, _delivery: &Delivery) -> Result<(), Rejection> {
///         if event.amount < 0 {
///             return Err(Rejection::new(422, "a payment is never negative"));
///         }
///         let _ = event.id;
///         Ok(())
///     }
/// }
/// ```
pub trait Handler: Send + Sync + 'static {
    /// The event, decoded from the JSON body. `serde_json::Value` takes
    /// anything.
    type Event: DeserializeOwned + Send + 'static;

    /// Handles one verified, first-seen delivery.
    ///
    /// Do the work, or record it, before returning `Ok`: the sender stops
    /// retrying at the 200. Return `Err` to have it retried.
    fn handle(
        &self,
        event: Self::Event,
        delivery: &Delivery,
    ) -> impl Future<Output = Result<(), Rejection>> + Send;
}

/// What a handler knows about the delivery beside its event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delivery {
    /// The source name, the last path segment.
    pub source: String,
    /// The idempotency key: the scheme's id, the body's `id` field, or the
    /// SHA-256 of the body, in that order.
    pub id: String,
    /// When the sender signed it, for a scheme with a timestamp.
    pub timestamp: Option<i64>,
}

/// A handler's refusal. The key is given back and the sender retries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rejection {
    /// The status to answer.
    pub status: u16,
    /// Said to the sender in the body. Keep it free of anything private.
    pub message: String,
}

impl Rejection {
    /// A refusal with this status.
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    /// A 500: something failed here, try again later.
    pub fn retry(message: impl Into<String>) -> Self {
        Self::new(500, message)
    }
}

/// What a `?` on any error answers: a 500 with a fixed body. The error's
/// own text can name a table, a host or a file, so it goes to the log, not
/// to the sender. Build a [`Rejection`] by hand to say more.
impl<E: std::error::Error> From<E> for Rejection {
    fn from(error: E) -> Self {
        tracing::error!(error = %error, "webhook handler failed");
        Self::retry("the receiver failed; retry later")
    }
}

/// One answer to one delivery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    /// The HTTP status.
    pub status: u16,
    /// A short plain-text body: what the sender reads.
    pub message: String,
    /// Why, for the log: the check a signature failed, or the store's
    /// error. Never sent.
    pub detail: Option<String>,
}

impl Answer {
    fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            detail: None,
        }
    }

    /// The same, with a reason that goes to the log and not to the sender.
    fn logged(status: u16, message: impl Into<String>, detail: String) -> Self {
        tracing::warn!(status, detail = %detail, "webhook delivery refused");
        Self {
            status,
            message: message.into(),
            detail: Some(detail),
        }
    }
}

/// Gives a claimed key back when it is dropped armed: the handler panicked,
/// or the request's future was dropped before the handler returned. The
/// release is a spawned task, because `Drop` cannot wait. A release that
/// cannot run leaves the key to expire with its [`LEASE`].
struct Unclaim {
    seen: Arc<dyn Seen>,
    source: String,
    id: String,
    armed: bool,
}

impl Drop for Unclaim {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let (seen, source, id) = (
            Arc::clone(&self.seen),
            std::mem::take(&mut self.source),
            std::mem::take(&mut self.id),
        );
        runtime.spawn(async move {
            if let Err(e) = seen.release(&source, &id).await {
                tracing::warn!(%source, %id, error = %e, "webhook key not given back");
            }
        });
    }
}

trait Erased: Send + Sync {
    fn call<'a>(
        &'a self,
        body: &[u8],
        delivery: Delivery,
    ) -> Result<BoxFuture<'a, Result<(), Rejection>>, String>;
}

impl<H: Handler> Erased for H {
    fn call<'a>(
        &'a self,
        body: &[u8],
        delivery: Delivery,
    ) -> Result<BoxFuture<'a, Result<(), Rejection>>, String> {
        let event: H::Event = serde_json::from_slice(body).map_err(|e| e.to_string())?;
        Ok(Box::pin(async move { self.handle(event, &delivery).await }))
    }
}

struct Source {
    scheme: Scheme,
    handler: Box<dyn Erased>,
}

/// The receiver being built: a path, a window and the sources.
pub struct Incoming {
    path: String,
    tolerance: i64,
    max_bytes: usize,
    keep: Duration,
    sources: HashMap<String, Source>,
}

impl Incoming {
    /// A receiver under `path`: a source `stripe` answers at
    /// `{path}/stripe`.
    pub fn new(path: &str) -> Self {
        Self {
            path: path.trim_end_matches('/').to_string(),
            tolerance: DEFAULT_TOLERANCE,
            max_bytes: DEFAULT_MAX_BYTES,
            keep: DEFAULT_KEEP,
            sources: HashMap::new(),
        }
    }

    /// The replay window, in seconds.
    #[must_use]
    pub fn tolerance(mut self, seconds: i64) -> Self {
        self.tolerance = seconds;
        self
    }

    /// The largest body accepted, in bytes.
    #[must_use]
    pub fn max_bytes(mut self, bytes: usize) -> Self {
        self.max_bytes = bytes;
        self
    }

    /// How long an idempotency key is kept, at most [`MAX_KEEP`].
    #[must_use]
    pub fn keep(mut self, keep: Duration) -> Self {
        self.keep = keep.min(MAX_KEEP);
        self
    }

    /// Adds a source: deliveries to `{path}/{name}`, signed with `scheme`,
    /// handled by `handler`.
    ///
    /// # Errors
    /// The name is not one lower-case path segment, or is taken.
    pub fn source<H: Handler>(
        mut self,
        name: &str,
        scheme: Scheme,
        handler: H,
    ) -> Result<Self, PluginError> {
        let valid = !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
        if !valid {
            return Err(PluginError::new(format!(
                "webhook source `{name}` is not one lower-case path segment\n\
                 help: use letters, digits, `-` and `_`, as in `stripe`"
            )));
        }
        if self.sources.contains_key(name) {
            return Err(PluginError::new(format!(
                "webhook source `{name}` is added twice\n\
                 help: give each source its own name"
            )));
        }
        self.sources.insert(
            name.to_string(),
            Source {
                scheme,
                handler: Box::new(handler),
            },
        );
        Ok(self)
    }

    /// The hook, with keys kept in memory until [`Receiver::set_seen`].
    ///
    /// A source with no replay window ([`Scheme::hmac_sha256`],
    /// [`Scheme::github`]) is safe from a replay only while its keys last.
    /// [`Receiver::set_seen`] with a lasting store warns about none; the
    /// memory store warns at build, through `tracing`.
    ///
    /// # Errors
    /// The path does not start with `/`.
    pub fn build(self) -> Result<Receiver, PluginError> {
        if !self.path.starts_with('/') || self.path.len() < 2 {
            return Err(PluginError::new(format!(
                "webhook path `{}` must start with `/` and name something\n\
                 help: use `/webhooks`",
                self.path
            )));
        }
        let receiver = Receiver {
            inner: Arc::new(Inner {
                incoming: self,
                seen: RwLock::new(Arc::new(MemorySeen::new())),
            }),
        };
        receiver.warn_unwindowed("in memory");
        Ok(receiver)
    }
}

struct Inner {
    incoming: Incoming,
    seen: RwLock<Arc<dyn Seen>>,
}

/// The request hook. Cheap to clone: the sources are shared.
#[derive(Clone)]
pub struct Receiver {
    inner: Arc<Inner>,
}

impl Receiver {
    /// Where the idempotency keys go from now on. The generated plugin
    /// calls it in `before_start`, with [`PgSeen`](crate::PgSeen) when the
    /// application has a database.
    pub fn set_seen(&self, seen: Arc<dyn Seen>) {
        *self.inner.seen.write().unwrap_or_else(|e| e.into_inner()) = seen;
    }

    /// The sources whose scheme has no replay window, sorted.
    pub fn unwindowed(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .inner
            .incoming
            .sources
            .iter()
            .filter(|(_, source)| !source.scheme.has_window())
            .map(|(name, _)| name.as_str())
            .collect();
        names.sort_unstable();
        names
    }

    fn warn_unwindowed(&self, store: &str) {
        for source in self.unwindowed() {
            tracing::warn!(
                source,
                "webhook source `{source}` has no replay window and its keys are {store}: \
                 a captured delivery is accepted again after a restart; \
                 help: give the receiver a lasting store with `Receiver::set_seen(PgSeen)`"
            );
        }
    }

    /// The path prefix, without a trailing slash.
    pub fn path(&self) -> &str {
        &self.inner.incoming.path
    }

    /// The source names, sorted.
    pub fn sources(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .inner
            .incoming
            .sources
            .keys()
            .map(String::as_str)
            .collect();
        names.sort_unstable();
        names
    }

    /// Handles one delivery to `source`, at the current time.
    pub async fn receive<'h>(
        &self,
        source: &str,
        header: impl Fn(&str) -> Option<&'h str>,
        body: &[u8],
    ) -> Answer {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
        self.receive_at(source, header, body, now).await
    }

    /// The same, at `now` (Unix seconds), so a test can sign in the past.
    pub async fn receive_at<'h>(
        &self,
        source: &str,
        header: impl Fn(&str) -> Option<&'h str>,
        body: &[u8],
        now: i64,
    ) -> Answer {
        let incoming = &self.inner.incoming;
        let Some(found) = incoming.sources.get(source) else {
            return Answer::new(404, format!("no webhook source is named `{source}`"));
        };
        if body.len() > incoming.max_bytes {
            return Answer::new(413, "the delivery is larger than this receiver accepts");
        }
        let verified = match found.scheme.verify(&header, body, now, incoming.tolerance) {
            Ok(verified) => verified,
            Err(refused) => {
                return Answer::logged(401, "the signature is not valid", refused.to_string())
            }
        };
        let id = verified
            .id
            .or_else(|| body_id(body))
            .unwrap_or_else(|| digest_hex(body));
        let delivery = Delivery {
            source: source.to_string(),
            id: id.clone(),
            timestamp: verified.timestamp,
        };
        let run = match found.handler.call(body, delivery) {
            Ok(run) => run,
            Err(why) => return Answer::new(400, format!("the event cannot be read: {why}")),
        };
        let seen = self
            .inner
            .seen
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        // A short lease first: a handler that never returns -- a panic, a
        // dropped request -- holds the key for minutes, not for `keep`.
        match seen.claim(source, &id, LEASE.min(incoming.keep)).await {
            Ok(Claim::New) => {}
            Ok(Claim::Done) => return Answer::new(200, "already received"),
            Ok(Claim::Running) => {
                return Answer::new(409, "this delivery is being handled; retry later")
            }
            Err(e) => {
                return Answer::logged(
                    503,
                    "the receiver cannot check for duplicates now; retry later",
                    e.to_string(),
                )
            }
        }
        let mut unclaim = Unclaim {
            seen: Arc::clone(&seen),
            source: source.to_string(),
            id: id.clone(),
            armed: true,
        };
        let outcome = run.await;
        unclaim.armed = false;
        match outcome {
            Ok(()) => {
                // Now the key holds for the whole window. A failed confirm
                // leaves the lease, so a copy after it runs the handler
                // again: the log says so.
                if let Err(e) = seen.confirm(source, &id, incoming.keep).await {
                    tracing::warn!(%source, %id, error = %e, "webhook key kept only for its lease");
                }
                Answer::new(200, "ok")
            }
            Err(rejection) => {
                // Give the key back, so the sender's retry runs the handler
                // again. A failed release keeps the key for its lease.
                if let Err(e) = seen.release(source, &id).await {
                    tracing::warn!(%source, %id, error = %e, "webhook key not given back");
                }
                let status = if (400..600).contains(&rejection.status) {
                    rejection.status
                } else {
                    500
                };
                Answer::new(status, rejection.message)
            }
        }
    }

    /// The source a request is for, or `None` when it is not under the path.
    fn source_of<'r>(&self, path: &'r str) -> Option<&'r str> {
        let rest = path.strip_prefix(self.inner.incoming.path.as_str())?;
        let name = rest.strip_prefix('/')?;
        (!name.is_empty() && !name.contains('/')).then_some(name)
    }
}

/// The top-level `id` string of a JSON body: Stripe's event id.
fn body_id(body: &[u8]) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct WithId {
        id: Option<serde_json::Value>,
    }
    match serde_json::from_slice::<WithId>(body).ok()?.id? {
        serde_json::Value::String(id) if !id.is_empty() => Some(id),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn plain(status: u16, message: String) -> brocanth::plugin_hooks::Response {
    let mut response = http::Response::new(Body::from(message));
    *response.status_mut() = http::StatusCode::from_u16(status).unwrap_or(http::StatusCode::OK);
    response.headers_mut().insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

impl AsyncRequestHook for Receiver {
    fn before_dispatch<'a>(
        &'a self,
        request: &'a mut Request,
        _state: &'a mut RequestState,
    ) -> BoxFuture<'a, Result<Flow, PluginError>> {
        Box::pin(async move {
            let Some(source) = self.source_of(request.uri().path()) else {
                return Ok(Flow::Continue);
            };
            if request.method() != http::Method::POST {
                let mut answer = plain(405, "a webhook is delivered with POST".to_string());
                answer
                    .headers_mut()
                    .insert(http::header::ALLOW, http::HeaderValue::from_static("POST"));
                return Ok(Flow::respond(answer));
            }
            let headers = request.headers();
            let answer = self
                .receive(
                    source,
                    |name| headers.get(name).and_then(|v| v.to_str().ok()),
                    request.body(),
                )
                .await;
            Ok(Flow::respond(plain(answer.status, answer.message)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sign::{new_secret, sign_standard, sign_stripe};
    use std::sync::atomic::{AtomicUsize, Ordering};

    const NOW: i64 = 1_700_000_000;

    #[derive(serde::Deserialize)]
    struct Event {
        kind: String,
    }

    #[derive(Clone, Default)]
    struct Count(Arc<AtomicUsize>);

    impl Handler for Count {
        type Event = Event;

        async fn handle(&self, event: Event, _delivery: &Delivery) -> Result<(), Rejection> {
            if event.kind == "fail" {
                return Err(Rejection::retry("not now"));
            }
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn run<F: Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future)
    }

    fn standard(secret: &str, id: &str, body: &[u8]) -> Vec<(&'static str, String)> {
        let key = crate::sign::decode_secret(secret).unwrap();
        vec![
            ("webhook-id", id.to_string()),
            ("webhook-timestamp", NOW.to_string()),
            ("webhook-signature", sign_standard(&key, id, NOW, body)),
        ]
    }

    fn lookup<'a>(list: &'a [(&'static str, String)]) -> impl Fn(&str) -> Option<&'a str> + 'a {
        move |name| {
            list.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.as_str())
        }
    }

    fn receiver(secret: &str, count: &Count) -> Receiver {
        Incoming::new("/webhooks")
            .source("app", Scheme::standard(secret).unwrap(), count.clone())
            .unwrap()
            .source("stripe", Scheme::stripe("whsec_s").unwrap(), count.clone())
            .unwrap()
            .build()
            .unwrap()
    }

    #[test]
    fn a_signed_delivery_runs_once_and_its_copy_is_a_duplicate() {
        let secret = new_secret();
        let count = Count::default();
        let receiver = receiver(&secret, &count);
        let body = br#"{"kind":"paid"}"#;
        let headers = standard(&secret, "msg_1", body);
        run(async {
            let first = receiver
                .receive_at("app", lookup(&headers), body, NOW)
                .await;
            assert_eq!(first, Answer::new(200, "ok"));
            let again = receiver
                .receive_at("app", lookup(&headers), body, NOW)
                .await;
            assert_eq!(again, Answer::new(200, "already received"));
        });
        assert_eq!(count.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_failed_handler_gives_the_key_back_for_the_retry() {
        let secret = new_secret();
        let count = Count::default();
        let receiver = receiver(&secret, &count);
        let body = br#"{"kind":"fail"}"#;
        let headers = standard(&secret, "msg_2", body);
        run(async {
            let first = receiver
                .receive_at("app", lookup(&headers), body, NOW)
                .await;
            assert_eq!(first.status, 500);
            let retry = receiver
                .receive_at("app", lookup(&headers), body, NOW)
                .await;
            assert_eq!(retry.status, 500, "the handler ran again: {retry:?}");
        });
    }

    #[test]
    fn a_refusal_names_the_check_that_failed() {
        let secret = new_secret();
        let count = Count::default();
        let receiver = receiver(&secret, &count);
        let body = br#"{"kind":"paid"}"#;
        let headers = standard(&secret, "msg_3", body);
        run(async {
            let forged = receiver
                .receive_at("app", lookup(&headers), br#"{"kind":"evil"}"#, NOW)
                .await;
            assert_eq!(forged.status, 401);
            let replayed = receiver
                .receive_at("app", lookup(&headers), body, NOW + 3600)
                .await;
            assert_eq!(replayed.status, 401);
            assert!(!replayed.message.contains("replay window"), "{replayed:?}");
            let why = replayed.detail.clone().unwrap_or_default();
            assert!(why.contains("replay window"), "{replayed:?}");
            let unknown = receiver
                .receive_at("nope", lookup(&headers), body, NOW)
                .await;
            assert_eq!(unknown.status, 404);
            let garbage = standard(&secret, "msg_4", b"[1]");
            let unreadable = receiver
                .receive_at("app", lookup(&garbage), b"[1]", NOW)
                .await;
            assert_eq!(unreadable.status, 400);
        });
        assert_eq!(count.0.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn stripe_is_deduplicated_by_the_event_id_across_two_signatures() {
        let count = Count::default();
        let receiver = receiver(&new_secret(), &count);
        let body = br#"{"id":"evt_9","kind":"paid"}"#;
        let first = [("stripe-signature", sign_stripe("whsec_s", NOW, body))];
        let retry = [("stripe-signature", sign_stripe("whsec_s", NOW + 60, body))];
        run(async {
            let a = receiver
                .receive_at("stripe", lookup(&first), body, NOW)
                .await;
            let b = receiver
                .receive_at("stripe", lookup(&retry), body, NOW + 60)
                .await;
            assert_eq!((a.status, b.message.as_str()), (200, "already received"));
        });
        assert_eq!(count.0.load(Ordering::SeqCst), 1);
    }

    /// A handler that waits on a gate, to hold a delivery mid-run.
    #[derive(Clone)]
    struct Gate(Arc<tokio::sync::Notify>, Count);

    impl Handler for Gate {
        type Event = Event;

        async fn handle(&self, event: Event, delivery: &Delivery) -> Result<(), Rejection> {
            if event.kind == "panic" {
                panic!("the handler failed hard");
            }
            self.0.notified().await;
            self.1.handle(event, delivery).await
        }
    }

    fn gated(secret: &str, gate: &Gate) -> Receiver {
        Incoming::new("/webhooks")
            .source("app", Scheme::standard(secret).unwrap(), gate.clone())
            .unwrap()
            .build()
            .unwrap()
    }

    #[test]
    fn a_copy_that_arrives_while_the_first_runs_is_told_to_retry() {
        let secret = new_secret();
        let gate = Gate(Arc::default(), Count::default());
        let receiver = gated(&secret, &gate);
        let body = br#"{"kind":"paid"}"#;
        let headers = standard(&secret, "msg_busy", body);
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_time()
            .build()
            .unwrap()
            .block_on(async {
                let first = {
                    let receiver = receiver.clone();
                    let headers = headers.clone();
                    tokio::spawn(async move {
                        receiver
                            .receive_at("app", lookup(&headers), body, NOW)
                            .await
                    })
                };
                // Let the first claim the key and wait at the gate.
                tokio::time::sleep(Duration::from_millis(50)).await;
                let copy = receiver
                    .receive_at("app", lookup(&headers), body, NOW)
                    .await;
                assert_eq!(copy.status, 409, "{copy:?}");
                gate.0.notify_one();
                assert_eq!(first.await.unwrap(), Answer::new(200, "ok"));
                let after = receiver
                    .receive_at("app", lookup(&headers), body, NOW)
                    .await;
                assert_eq!(after, Answer::new(200, "already received"));
            });
        assert_eq!(gate.1 .0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_dropped_or_panicked_run_gives_the_key_back() {
        let secret = new_secret();
        let gate = Gate(Arc::default(), Count::default());
        let receiver = gated(&secret, &gate);
        let body = br#"{"kind":"paid"}"#;
        let headers = standard(&secret, "msg_drop", body);
        let boom = br#"{"kind":"panic"}"#;
        let boom_headers = standard(&secret, "msg_panic", boom);
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_time()
            .build()
            .unwrap()
            .block_on(async {
                // The request is dropped while the handler waits.
                let dropped = tokio::time::timeout(
                    Duration::from_millis(50),
                    receiver.receive_at("app", lookup(&headers), body, NOW),
                )
                .await;
                assert!(dropped.is_err(), "the run was cut");
                let panicked = {
                    let receiver = receiver.clone();
                    let headers = boom_headers.clone();
                    tokio::spawn(async move {
                        receiver
                            .receive_at("app", lookup(&headers), boom, NOW)
                            .await
                    })
                };
                assert!(panicked.await.is_err(), "the handler panicked");
                // The release is a spawned task: give it a moment.
                tokio::time::sleep(Duration::from_millis(50)).await;
                let retry = {
                    let receiver = receiver.clone();
                    let headers = headers.clone();
                    tokio::spawn(async move {
                        receiver
                            .receive_at("app", lookup(&headers), body, NOW)
                            .await
                    })
                };
                tokio::time::sleep(Duration::from_millis(50)).await;
                gate.0.notify_one();
                assert_eq!(retry.await.unwrap(), Answer::new(200, "ok"));
                // The panicked key was given back: the copy runs, and
                // panics again, rather than being a duplicate.
                let again = {
                    let receiver = receiver.clone();
                    tokio::spawn(async move {
                        receiver
                            .receive_at("app", lookup(&boom_headers), boom, NOW)
                            .await
                    })
                };
                assert!(again.await.is_err(), "the copy ran the handler again");
            });
        let _ = gate;
    }

    #[test]
    fn a_handler_error_and_a_refusal_say_nothing_private_to_the_sender() {
        #[derive(Clone)]
        struct Leaky;
        impl Handler for Leaky {
            type Event = Event;
            async fn handle(&self, _: Event, _: &Delivery) -> Result<(), Rejection> {
                let failed: Result<(), std::io::Error> = Err(std::io::Error::other(
                    "connection to db.internal:5432 refused",
                ));
                failed?;
                Ok(())
            }
        }
        let secret = new_secret();
        let receiver = Incoming::new("/webhooks")
            .source("app", Scheme::standard(&secret).unwrap(), Leaky)
            .unwrap()
            .build()
            .unwrap();
        let body = br#"{"kind":"paid"}"#;
        let headers = standard(&secret, "msg_leak", body);
        run(async {
            let answer = receiver
                .receive_at("app", lookup(&headers), body, NOW)
                .await;
            assert_eq!(answer.status, 500);
            assert!(!answer.message.contains("db.internal"), "{answer:?}");
            let mut forged = headers.clone();
            forged.remove(0);
            let refused = receiver.receive_at("app", lookup(&forged), body, NOW).await;
            assert_eq!(refused.status, 401);
            assert!(!refused.message.contains("webhook-id"), "{refused:?}");
            assert!(refused.detail.unwrap_or_default().contains("webhook-id"));
        });
    }

    #[test]
    fn a_source_without_a_window_is_named() {
        let receiver = Incoming::new("/webhooks")
            .source(
                "app",
                Scheme::standard(&new_secret()).unwrap(),
                Count::default(),
            )
            .unwrap()
            .source("gh", Scheme::github("s3cret").unwrap(), Count::default())
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(receiver.unwindowed(), vec!["gh"]);
    }

    #[test]
    fn a_huge_keep_is_clamped() {
        let incoming = Incoming::new("/webhooks").keep(Duration::MAX);
        assert_eq!(incoming.keep, MAX_KEEP);
    }

    #[test]
    fn a_source_name_must_be_a_path_segment() {
        let err = Incoming::new("/webhooks")
            .source("Has/Slash", Scheme::stripe("x").unwrap(), Count::default())
            .err()
            .expect("refused");
        assert!(err.to_string().contains("help:"), "{err}");
    }

    #[test]
    fn only_one_segment_under_the_path_is_a_source() {
        let receiver = receiver(&new_secret(), &Count::default());
        assert_eq!(receiver.source_of("/webhooks/app"), Some("app"));
        assert_eq!(receiver.source_of("/webhooks"), None);
        assert_eq!(receiver.source_of("/webhooks/app/x"), None);
        assert_eq!(receiver.source_of("/webhooksapp"), None);
    }
}
