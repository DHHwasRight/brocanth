//! The receiver through the whole pipeline, beside the CSRF hook.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_body::Body;
use brocanth::plugin_context::{Base, NoContext};
use brocanth::plugin_hooks::{HookRegistrar, HookSource, OnAsyncRequest};
use brocanth::plugin_order::Order;
use brocanth::plugin_routes::{Handler as Route, RouteCx};
use brocanth::routing::{Router, Verb};
use brocanth_middleware::{Csrf, CsrfConfig};
use brocanth_test::{TestClient, TestRequest};
use brocanth_webhooks::sign::{decode_secret, new_secret, sign_standard};
use brocanth_webhooks::{Delivery, Handler, Incoming, Receiver, Rejection, Scheme};

#[derive(serde::Deserialize)]
struct Event {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Clone, Default)]
struct Count(Arc<AtomicUsize>);

impl Handler for Count {
    type Event = Event;

    async fn handle(&self, event: Event, _delivery: &Delivery) -> Result<(), Rejection> {
        assert_eq!(event.kind, "invoice.paid");
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// The plugin a project writes, cut down: one source, one hook.
struct Hooks {
    receiver: Receiver,
}

impl Plugin for Hooks {
    type Config = Receiver;
    type Provides = NoContext;
    const NAME: &'static str = "webhooks_in";
    const INSTALLS_HOOKS: bool = true;

    fn new(receiver: Receiver) -> Self {
        Self { receiver }
    }
}

impl HookSource for Hooks {
    type Families = OnAsyncRequest;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        registrar.request_async(
            "receive",
            Order::new().before_if_present("csrf.check"),
            self.receiver.clone(),
        );
        Ok(())
    }
}

fn table() -> Router<Route<Base>> {
    let page: Route<Base> = Box::new(|_cx: &mut RouteCx<'_, Base>| {
        Ok(http::Response::new(Body::from(b"page".to_vec())))
    });
    let mut table = Router::builder();
    table.route("/", Verb::Post, page).unwrap();
    table.build().unwrap()
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[test]
fn a_signed_delivery_passes_the_csrf_check_and_a_forged_one_stops_at_the_hook() {
    let secret = new_secret();
    let count = Count::default();
    let receiver = Incoming::new("/webhooks")
        .source("billing", Scheme::standard(&secret).unwrap(), count.clone())
        .unwrap()
        .build()
        .unwrap();
    // CSRF registered first, so only the declared order puts the receiver
    // ahead of it.
    let app = AppBuilder::new()
        .plugin_hooking(Csrf::new(CsrfConfig::default()))
        .plugin_hooking(Hooks::new(receiver))
        .build()
        .expect("an application");
    let client = TestClient::serving(app, table());

    let body = br#"{"type":"invoice.paid"}"#;
    let stamp = now();
    let key = decode_secret(&secret).unwrap();
    let signed = || {
        TestRequest::post("/webhooks/billing")
            .header("webhook-id", "msg_1")
            .header("webhook-timestamp", &stamp.to_string())
            .header(
                "webhook-signature",
                &sign_standard(&key, "msg_1", stamp, body),
            )
            .body("application/json", body.to_vec())
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let first = runtime.block_on(client.send_async(signed()));
    assert_eq!(first.status(), 200, "{}", first.text());
    let again = runtime.block_on(client.send_async(signed()));
    assert_eq!(again.text(), "already received");
    assert_eq!(count.0.load(Ordering::SeqCst), 1);

    let forged = runtime.block_on(
        client.send_async(
            TestRequest::post("/webhooks/billing")
                .header("webhook-id", "msg_2")
                .header("webhook-timestamp", &stamp.to_string())
                .header("webhook-signature", "v1,AAAA")
                .body("application/json", body.to_vec()),
        ),
    );
    assert_eq!(forged.status(), 401);
    assert_eq!(count.0.load(Ordering::SeqCst), 1);

    // A GET on a source path is answered, not passed to the router.
    let get = runtime.block_on(client.send_async(TestRequest::get("/webhooks/billing")));
    assert_eq!(get.status(), 405);
    // Any other POST still meets the CSRF check.
    let other = runtime.block_on(
        client.send_async(TestRequest::post("/").body("application/json", b"{}".to_vec())),
    );
    assert_eq!(other.status(), 403);
}

// ---------------------------------------------------------------------------
// Each scheme, through `Receiver::receive_at`.
// ---------------------------------------------------------------------------

use std::sync::Mutex;
use std::time::Duration;

use brocanth_webhooks::sign::{sign_hmac, sign_stripe};

const NOW: i64 = 1_700_000_000;

/// Records every delivery it runs; answers `fail` with the status in it.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<Delivery>>>);

impl Log {
    fn count(&self) -> usize {
        self.0.lock().unwrap().len()
    }
    fn last(&self) -> Delivery {
        self.0
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("a delivery ran")
    }
}

impl Handler for Log {
    type Event = serde_json::Value;

    async fn handle(&self, event: serde_json::Value, delivery: &Delivery) -> Result<(), Rejection> {
        if let Some(status) = event.get("fail").and_then(serde_json::Value::as_u64) {
            return Err(Rejection::new(status as u16, "not now"));
        }
        self.0.lock().unwrap().push(delivery.clone());
        Ok(())
    }
}

/// How a sender signs a body at a time, with an id: the header pairs.
type Sign = Box<dyn Fn(&[u8], i64, &str) -> Vec<(String, String)>>;

/// One scheme and how its sender signs a body at a time.
struct Sender {
    name: &'static str,
    scheme: Scheme,
    sign: Sign,
}

const STANDARD_SECRET: &str = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";

fn senders() -> Vec<Sender> {
    let key = decode_secret(STANDARD_SECRET).unwrap();
    vec![
        Sender {
            name: "standard",
            scheme: Scheme::standard(STANDARD_SECRET).unwrap(),
            sign: Box::new(move |body, at, id| {
                vec![
                    ("webhook-id".into(), id.to_string()),
                    ("webhook-timestamp".into(), at.to_string()),
                    (
                        "webhook-signature".into(),
                        sign_standard(&key, id, at, body),
                    ),
                ]
            }),
        },
        Sender {
            name: "stripe",
            scheme: Scheme::stripe("whsec_stripe").unwrap(),
            sign: Box::new(|body, at, _| {
                vec![(
                    "stripe-signature".into(),
                    sign_stripe("whsec_stripe", at, body),
                )]
            }),
        },
        Sender {
            name: "github",
            scheme: Scheme::github("gh-secret").unwrap(),
            sign: Box::new(|body, _, id| {
                vec![
                    (
                        "x-hub-signature-256".into(),
                        sign_hmac("gh-secret", "sha256=", body),
                    ),
                    ("x-github-delivery".into(), id.to_string()),
                ]
            }),
        },
        Sender {
            name: "plain",
            scheme: Scheme::hmac_sha256("X-Signature", "", "plain-secret").unwrap(),
            sign: Box::new(|body, _, _| {
                vec![("x-signature".into(), sign_hmac("plain-secret", "", body))]
            }),
        },
    ]
}

fn receiver_of(senders: &[Sender], log: &Log) -> Receiver {
    let mut incoming = Incoming::new("/webhooks");
    for sender in senders {
        incoming = incoming
            .source(sender.name, sender.scheme.clone(), log.clone())
            .unwrap();
    }
    incoming.build().unwrap()
}

fn find<'a>(headers: &'a [(String, String)]) -> impl Fn(&str) -> Option<&'a str> + 'a {
    move |name| {
        headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

#[tokio::test]
async fn each_scheme_takes_its_own_signature_and_refuses_another_secret() {
    let log = Log::default();
    let all = senders();
    let receiver = receiver_of(&all, &log);
    for (n, sender) in all.iter().enumerate() {
        let body = format!(r#"{{"id":"evt_ok_{n}"}}"#).into_bytes();
        let headers = (sender.sign)(&body, NOW, &format!("msg_ok_{n}"));
        let ok = receiver
            .receive_at(sender.name, find(&headers), &body, NOW)
            .await;
        assert_eq!(ok.status, 200, "{}: {ok:?}", sender.name);

        // Signed for another source: every other scheme refuses it.
        for other in all.iter().filter(|o| o.name != sender.name) {
            let body = format!(r#"{{"id":"evt_x_{n}_{}"}}"#, other.name).into_bytes();
            let headers = (sender.sign)(&body, NOW, "msg_x");
            let refused = receiver
                .receive_at(other.name, find(&headers), &body, NOW)
                .await;
            assert_eq!(
                refused.status, 401,
                "{} took {}'s signature",
                other.name, sender.name
            );
        }
    }
    assert_eq!(log.count(), all.len());
}

#[tokio::test]
async fn each_scheme_refuses_a_body_changed_by_one_byte() {
    let log = Log::default();
    let all = senders();
    let receiver = receiver_of(&all, &log);
    let body = br#"{"id":"evt_1","amount":1000}"#.to_vec();
    for sender in &all {
        let headers = (sender.sign)(&body, NOW, "msg_1");
        for at in [0, body.len() / 2, body.len() - 1] {
            let mut changed = body.clone();
            changed[at] ^= 0x01;
            let answer = receiver
                .receive_at(sender.name, find(&headers), &changed, NOW)
                .await;
            assert_eq!(answer.status, 401, "{} at byte {at}", sender.name);
        }
        // One byte more, and one byte less.
        let mut longer = body.clone();
        longer.push(b' ');
        let answer = receiver
            .receive_at(sender.name, find(&headers), &longer, NOW)
            .await;
        assert_eq!(answer.status, 401, "{} with a byte more", sender.name);
        let shorter = &body[..body.len() - 1];
        let answer = receiver
            .receive_at(sender.name, find(&headers), shorter, NOW)
            .await;
        assert_eq!(answer.status, 401, "{} with a byte less", sender.name);
    }
    assert_eq!(log.count(), 0);
}

#[tokio::test]
async fn a_timestamp_outside_the_window_is_refused_in_the_past_and_the_future() {
    let log = Log::default();
    let all = senders();
    let receiver = receiver_of(&all, &log);
    for sender in all
        .iter()
        .filter(|s| matches!(s.name, "standard" | "stripe"))
    {
        for (n, offset) in [-301_i64, 301, -86_400 * 7].into_iter().enumerate() {
            let body = format!(r#"{{"id":"evt_old_{n}"}}"#).into_bytes();
            let headers = (sender.sign)(&body, NOW + offset, &format!("msg_old_{n}"));
            let answer = receiver
                .receive_at(sender.name, find(&headers), &body, NOW)
                .await;
            assert_eq!(answer.status, 401, "{} signed {offset}s away", sender.name);
            assert!(answer.message.contains("replay window"), "{answer:?}");
        }
        for (n, offset) in [-300_i64, 300].into_iter().enumerate() {
            let body = format!(r#"{{"id":"evt_edge_{}_{n}"}}"#, sender.name).into_bytes();
            let headers = (sender.sign)(&body, NOW + offset, &format!("msg_edge_{n}"));
            let answer = receiver
                .receive_at(sender.name, find(&headers), &body, NOW)
                .await;
            assert_eq!(answer.status, 200, "{} at the edge {offset}s", sender.name);
        }
    }
    assert_eq!(log.count(), 4);
}

#[tokio::test]
async fn the_tolerance_setting_moves_the_window() {
    let log = Log::default();
    let receiver = Incoming::new("/webhooks")
        .tolerance(10)
        .source(
            "app",
            Scheme::standard(STANDARD_SECRET).unwrap(),
            log.clone(),
        )
        .unwrap()
        .build()
        .unwrap();
    let sign = &senders()[0].sign;
    let late = sign(b"{}", NOW - 11, "msg_late");
    assert_eq!(
        receiver
            .receive_at("app", find(&late), b"{}", NOW)
            .await
            .status,
        401
    );
    let fine = sign(b"{}", NOW - 10, "msg_fine");
    assert_eq!(
        receiver
            .receive_at("app", find(&fine), b"{}", NOW)
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn a_scheme_without_a_timestamp_has_no_window_but_still_drops_a_replay() {
    let log = Log::default();
    let all = senders();
    let receiver = receiver_of(&all, &log);
    for sender in all.iter().filter(|s| matches!(s.name, "github" | "plain")) {
        let body = format!(r#"{{"zen":"{}"}}"#, sender.name).into_bytes();
        let headers = (sender.sign)(&body, NOW, "d-1");
        let week_later = NOW + 7 * 86_400;
        let first = receiver
            .receive_at(sender.name, find(&headers), &body, week_later)
            .await;
        assert_eq!(first.status, 200, "{}: {first:?}", sender.name);
        let replay = receiver
            .receive_at(sender.name, find(&headers), &body, week_later + 1)
            .await;
        assert_eq!(replay.message, "already received", "{}", sender.name);
        assert_eq!(log.last().timestamp, None);
    }
    assert_eq!(log.count(), 2);
}

#[tokio::test]
async fn a_replayed_delivery_runs_the_handler_once_in_every_scheme() {
    let log = Log::default();
    let all = senders();
    let receiver = receiver_of(&all, &log);
    for sender in &all {
        let body = format!(r#"{{"id":"evt_{}"}}"#, sender.name).into_bytes();
        let headers = (sender.sign)(&body, NOW, "msg_same");
        let first = receiver
            .receive_at(sender.name, find(&headers), &body, NOW)
            .await;
        assert_eq!(first.message, "ok", "{}", sender.name);
        // The sender's retry, signed a minute later.
        let retry = (sender.sign)(&body, NOW + 60, "msg_same");
        let again = receiver
            .receive_at(sender.name, find(&retry), &body, NOW + 60)
            .await;
        assert_eq!(
            (again.status, again.message.as_str()),
            (200, "already received"),
            "{}",
            sender.name
        );
    }
    assert_eq!(log.count(), all.len());
}

/// `x-github-delivery` is not signed. Whoever captured a delivery can send it
/// again with a new one; the key is the body's, so it still runs once.
#[tokio::test]
async fn a_replay_with_a_new_unsigned_delivery_header_is_still_a_duplicate() {
    let log = Log::default();
    let all = senders();
    let receiver = receiver_of(&all, &log);
    let github = &all[2];
    let body = br#"{"action":"opened","number":7}"#;
    let captured = (github.sign)(body, NOW, "guid-1");
    assert_eq!(
        receiver
            .receive_at("github", find(&captured), body, NOW)
            .await
            .message,
        "ok"
    );
    let mut replay = captured.clone();
    replay[1].1 = "guid-forged".to_string();
    let answer = receiver
        .receive_at("github", find(&replay), body, NOW)
        .await;
    assert_eq!(answer.message, "already received");
    // Without the header at all, the same.
    let bare = vec![captured[0].clone()];
    let answer = receiver.receive_at("github", find(&bare), body, NOW).await;
    assert_eq!(answer.message, "already received");
    assert_eq!(log.count(), 1);
}

#[tokio::test]
async fn a_standard_signature_covers_the_id() {
    let log = Log::default();
    let all = senders();
    let receiver = receiver_of(&all, &log);
    let body = br#"{"a":1}"#;
    let mut headers = (all[0].sign)(body, NOW, "msg_a");
    // The id swapped: a replay that tries for a fresh idempotency key.
    headers[0].1 = "msg_b".to_string();
    assert_eq!(
        receiver
            .receive_at("standard", find(&headers), body, NOW)
            .await
            .status,
        401
    );
    // The timestamp swapped within the window: the MAC covers it too.
    let mut headers = (all[0].sign)(body, NOW, "msg_c");
    headers[1].1 = (NOW + 1).to_string();
    assert_eq!(
        receiver
            .receive_at("standard", find(&headers), body, NOW)
            .await
            .status,
        401
    );
    assert_eq!(log.count(), 0);
}

#[tokio::test]
async fn standard_takes_one_valid_signature_among_several_and_only_v1() {
    let log = Log::default();
    let all = senders();
    let receiver = receiver_of(&all, &log);
    let body = br#"{"rotated":true}"#;
    let mut headers = (all[0].sign)(body, NOW, "msg_rot");
    let valid = headers[2].1.clone();
    // A rotated secret: the old signature first, then the new one.
    headers[2].1 = format!("v1,AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA= v1a,xyz {valid}");
    assert_eq!(
        receiver
            .receive_at("standard", find(&headers), body, NOW)
            .await
            .status,
        200
    );

    let mut other = (all[0].sign)(body, NOW, "msg_v2");
    let b64 = other[2].1.strip_prefix("v1,").unwrap().to_string();
    other[2].1 = format!("v2,{b64}");
    assert_eq!(
        receiver
            .receive_at("standard", find(&other), body, NOW)
            .await
            .status,
        401
    );
    other[2].1 = String::new();
    assert_eq!(
        receiver
            .receive_at("standard", find(&other), body, NOW)
            .await
            .status,
        401
    );
}

#[tokio::test]
async fn a_missing_or_unreadable_header_is_a_401_that_names_it() {
    let log = Log::default();
    let all = senders();
    let receiver = receiver_of(&all, &log);
    let body = br#"{"id":"evt_h"}"#;
    let full = (all[0].sign)(body, NOW, "msg_h");
    for (drop, name) in [
        (0, "webhook-id"),
        (1, "webhook-timestamp"),
        (2, "webhook-signature"),
    ] {
        let mut headers = full.clone();
        headers.remove(drop);
        let answer = receiver
            .receive_at("standard", find(&headers), body, NOW)
            .await;
        assert_eq!(answer.status, 401);
        assert!(answer.message.contains(name), "{answer:?}");
    }
    let mut headers = full.clone();
    headers[1].1 = "soon".to_string();
    let answer = receiver
        .receive_at("standard", find(&headers), body, NOW)
        .await;
    assert_eq!(answer.status, 401);
    assert!(answer.message.contains("cannot be read"), "{answer:?}");

    for value in ["v1=abc", "t=soon,v1=00", ""] {
        let headers = vec![("stripe-signature".to_string(), value.to_string())];
        let answer = receiver
            .receive_at("stripe", find(&headers), body, NOW)
            .await;
        assert_eq!(answer.status, 401, "{value:?}");
    }
    for value in ["sha256=zz", "sha256=abc", "md5=00", ""] {
        let headers = vec![("x-hub-signature-256".to_string(), value.to_string())];
        let answer = receiver
            .receive_at("github", find(&headers), body, NOW)
            .await;
        assert_eq!(answer.status, 401, "{value:?}");
    }
    let none: Vec<(String, String)> = Vec::new();
    for sender in &all {
        let answer = receiver
            .receive_at(sender.name, find(&none), body, NOW)
            .await;
        assert_eq!(answer.status, 401, "{} with no header", sender.name);
    }
    assert_eq!(log.count(), 0);
}

#[tokio::test]
async fn a_body_past_max_bytes_is_a_413_and_the_limit_itself_is_taken() {
    let log = Log::default();
    let receiver = Incoming::new("/webhooks")
        .max_bytes(32)
        .source(
            "app",
            Scheme::standard(STANDARD_SECRET).unwrap(),
            log.clone(),
        )
        .unwrap()
        .build()
        .unwrap();
    let sign = &senders()[0].sign;
    let fits = format!(r#"{{"pad":"{}"}}"#, "x".repeat(32 - 10)).into_bytes();
    assert_eq!(fits.len(), 32);
    let headers = sign(&fits, NOW, "msg_fit");
    assert_eq!(
        receiver
            .receive_at("app", find(&headers), &fits, NOW)
            .await
            .status,
        200
    );
    let over = format!(r#"{{"pad":"{}"}}"#, "x".repeat(32 - 9)).into_bytes();
    let headers = sign(&over, NOW, "msg_over");
    assert_eq!(
        receiver
            .receive_at("app", find(&headers), &over, NOW)
            .await
            .status,
        413
    );
    // Unsigned and too large: the size is checked first, so no MAC is spent.
    let none: Vec<(String, String)> = Vec::new();
    assert_eq!(
        receiver
            .receive_at("app", find(&none), &over, NOW)
            .await
            .status,
        413
    );
}

#[tokio::test]
async fn the_delivery_names_its_id_from_the_scheme_the_body_or_the_digest() {
    let log = Log::default();
    let all = senders();
    let receiver = receiver_of(&all, &log);

    let body = br#"{"id":"evt_body"}"#;
    let headers = (all[0].sign)(body, NOW, "msg_header");
    receiver
        .receive_at("standard", find(&headers), body, NOW)
        .await;
    let delivery = log.last();
    assert_eq!(
        (
            delivery.source.as_str(),
            delivery.id.as_str(),
            delivery.timestamp
        ),
        ("standard", "msg_header", Some(NOW))
    );

    let headers = (all[1].sign)(body, NOW, "");
    receiver
        .receive_at("stripe", find(&headers), body, NOW)
        .await;
    assert_eq!(log.last().id, "evt_body");

    let numbered = br#"{"id":42}"#;
    let headers = (all[3].sign)(numbered, NOW, "");
    receiver
        .receive_at("plain", find(&headers), numbered, NOW)
        .await;
    assert_eq!(log.last().id, "42");

    let anonymous = br#"{"zen":"keep it simple"}"#;
    let headers = (all[3].sign)(anonymous, NOW, "");
    receiver
        .receive_at("plain", find(&headers), anonymous, NOW)
        .await;
    let id = log.last().id;
    assert_eq!(id, brocanth_webhooks::sign::digest_hex(anonymous));
    assert_eq!(id.len(), 64);
}

#[tokio::test]
async fn a_rejection_answers_its_status_and_the_retry_runs_again() {
    let log = Log::default();
    let all = senders();
    let receiver = receiver_of(&all, &log);
    let body = br#"{"fail":422}"#;
    let headers = (all[0].sign)(body, NOW, "msg_422");
    let first = receiver
        .receive_at("standard", find(&headers), body, NOW)
        .await;
    assert_eq!((first.status, first.message.as_str()), (422, "not now"));
    let retry = receiver
        .receive_at("standard", find(&headers), body, NOW)
        .await;
    assert_eq!(retry.status, 422, "the key was given back: {retry:?}");

    // A status a rejection cannot answer becomes a 500, so the sender retries.
    for status in [200, 302, 99, 600] {
        let body = format!(r#"{{"fail":{status}}}"#).into_bytes();
        let headers = (all[0].sign)(&body, NOW, &format!("msg_{status}"));
        let answer = receiver
            .receive_at("standard", find(&headers), &body, NOW)
            .await;
        assert_eq!(answer.status, 500, "{status}");
    }
    assert_eq!(log.count(), 0);
}

#[tokio::test]
async fn the_keep_setting_is_how_long_a_key_stops_a_copy() {
    let log = Log::default();
    let receiver = Incoming::new("/webhooks")
        .keep(Duration::ZERO)
        .source(
            "app",
            Scheme::standard(STANDARD_SECRET).unwrap(),
            log.clone(),
        )
        .unwrap()
        .build()
        .unwrap();
    let headers = (senders()[0].sign)(b"{}", NOW, "msg_k");
    receiver.receive_at("app", find(&headers), b"{}", NOW).await;
    let again = receiver.receive_at("app", find(&headers), b"{}", NOW).await;
    assert_eq!(again.message, "ok", "the key was kept for no time");
    assert_eq!(log.count(), 2);
}

#[test]
fn a_path_or_a_source_that_cannot_work_is_refused_with_help() {
    for path in ["webhooks", "/", ""] {
        let err = Incoming::new(path).build().err().expect(path);
        assert!(err.to_string().contains("help:"), "{err}");
    }
    let receiver = Incoming::new("/hooks/").build().unwrap();
    assert_eq!(receiver.path(), "/hooks");

    for name in ["", "Stripe", "a/b", "a.b", "..", "a b", "é"] {
        let err = Incoming::new("/webhooks")
            .source(name, Scheme::stripe("s").unwrap(), Log::default())
            .err()
            .unwrap_or_else(|| panic!("{name:?} was taken"));
        assert!(err.to_string().contains("help:"), "{err}");
    }
    let err = Incoming::new("/webhooks")
        .source("stripe", Scheme::stripe("s").unwrap(), Log::default())
        .unwrap()
        .source("stripe", Scheme::stripe("t").unwrap(), Log::default())
        .err()
        .expect("twice");
    assert!(err.to_string().contains("help:"), "{err}");
    let receiver = Incoming::new("/webhooks")
        .source("b", Scheme::stripe("s").unwrap(), Log::default())
        .unwrap()
        .source("a", Scheme::stripe("s").unwrap(), Log::default())
        .unwrap()
        .build()
        .unwrap();
    assert_eq!(receiver.sources(), vec!["a", "b"]);
}

#[test]
fn the_defaults_are_what_the_module_doc_says() {
    assert_eq!(brocanth_webhooks::sign::DEFAULT_TOLERANCE, 300);
    assert_eq!(brocanth_webhooks::incoming::DEFAULT_MAX_BYTES, 1_048_576);
}

// ---------------------------------------------------------------------------
// Through the pipeline, beside the CSRF check.
// ---------------------------------------------------------------------------

/// The client over CSRF and the receiver. A macro, because the client's
/// type names the application's plugin list.
macro_rules! pipeline {
    ($receiver:expr) => {
        TestClient::serving(
            AppBuilder::new()
                .plugin_hooking(Csrf::new(CsrfConfig::default()))
                .plugin_hooking(Hooks::new($receiver))
                .build()
                .expect("an application"),
            table(),
        )
    };
}

#[tokio::test]
async fn only_a_known_source_is_answered_and_everything_else_meets_the_csrf_check() {
    let log = Log::default();
    let client = pipeline!(receiver_of(&senders(), &log));

    let unknown = client
        .send_async(TestRequest::post("/webhooks/nobody").body("application/json", b"{}".to_vec()))
        .await;
    assert_eq!(unknown.status(), 404);
    assert!(unknown
        .header("content-type")
        .unwrap()
        .starts_with("text/plain"));

    // Not one segment under the path: the hook lets it pass to the check.
    for path in [
        "/webhooks",
        "/webhooks/",
        "/webhooks/standard/x",
        "/webhooksx/standard",
    ] {
        let answer = client
            .send_async(TestRequest::post(path).body("application/json", b"{}".to_vec()))
            .await;
        assert_eq!(answer.status(), 403, "{path}: {}", answer.text());
    }

    let put = client
        .send_async(TestRequest::put("/webhooks/standard").body("application/json", b"{}".to_vec()))
        .await;
    assert_eq!(put.status(), 405);
    assert_eq!(put.header("allow"), Some("POST"));

    // A name that is markup is answered as text, never as a page.
    let markup = client
        .send_async(
            TestRequest::post("/webhooks/%3Cscript%3E").body("application/json", b"{}".to_vec()),
        )
        .await;
    assert_eq!(markup.status(), 404);
    assert!(markup
        .header("content-type")
        .unwrap()
        .starts_with("text/plain"));
    assert_eq!(log.count(), 0);
}

#[tokio::test]
async fn a_stripe_delivery_through_the_pipeline_runs_once() {
    let log = Log::default();
    let client = pipeline!(receiver_of(&senders(), &log));
    let body = br#"{"id":"evt_http","type":"invoice.paid"}"#;
    let signed = |at: i64| {
        TestRequest::post("/webhooks/stripe")
            .header("stripe-signature", &sign_stripe("whsec_stripe", at, body))
            .body("application/json", body.to_vec())
    };
    let stamp = now();
    assert_eq!(client.send_async(signed(stamp)).await.status(), 200);
    assert_eq!(
        client.send_async(signed(stamp + 1)).await.text(),
        "already received"
    );
    assert_eq!(client.send_async(signed(stamp - 3600)).await.status(), 401);
    assert_eq!(log.count(), 1);
}
