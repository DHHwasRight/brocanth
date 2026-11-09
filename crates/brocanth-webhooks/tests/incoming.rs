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
