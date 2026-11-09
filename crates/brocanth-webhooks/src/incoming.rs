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
//! 6. The idempotency key is claimed, or the delivery is a duplicate and is
//!    answered 200 with nothing run. See [`crate::seen`].
//! 7. The handler runs. `Ok` is 200; `Err` gives the key back and answers
//!    the rejection's status, 500 by default, so the sender retries.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use brocanth::plugin_api::PluginError;
use brocanth::plugin_body::Body;
use brocanth::plugin_hooks::{AsyncRequestHook, Flow, Request, RequestState};
use serde::de::DeserializeOwned;

use crate::seen::{MemorySeen, Seen, DEFAULT_KEEP};
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

impl<E: std::error::Error> From<E> for Rejection {
    fn from(error: E) -> Self {
        Self::retry(error.to_string())
    }
}

/// One answer to one delivery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    /// The HTTP status.
    pub status: u16,
    /// A short plain-text body.
    pub message: String,
}

impl Answer {
    fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
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

    /// How long an idempotency key is kept.
    #[must_use]
    pub fn keep(mut self, keep: Duration) -> Self {
        self.keep = keep;
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
        Ok(Receiver {
            inner: Arc::new(Inner {
                incoming: self,
                seen: RwLock::new(Arc::new(MemorySeen::new())),
            }),
        })
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
            Err(refused) => return Answer::new(401, refused.to_string()),
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
        match seen.claim(source, &id, incoming.keep).await {
            Ok(true) => {}
            Ok(false) => return Answer::new(200, "already received"),
            Err(e) => return Answer::new(503, e.to_string()),
        }
        match run.await {
            Ok(()) => Answer::new(200, "ok"),
            Err(rejection) => {
                // Give the key back, so the sender's retry runs the handler
                // again. A failed release keeps the key: the retry is then
                // answered as a duplicate, which the store's error explains.
                let _ = seen.release(source, &id).await;
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
            assert!(replayed.message.contains("replay window"), "{replayed:?}");
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
