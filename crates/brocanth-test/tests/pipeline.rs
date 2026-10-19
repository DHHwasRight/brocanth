//! The claim that this client runs the application rather than a sketch of
//! it: the hooks fire, the router matches, the statuses are owned where
//! `brocanth-server` owns them, and a route that waits is driven either way.
//!
//! Each test here is a thing no existing test could express, because every
//! existing test reaches past the router and calls a handler by name.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_context::{Base, NoContext};
use brocanth::plugin_body::Body;
use brocanth::plugin_hooks::{
    Both, Flow, HookFamilies, HookRegistrar, HookSource, OnRequest, OnResponse, Request,
    RequestHook, RequestState, Response, ResponseHook,
};
use brocanth::plugin_order::Order;
use brocanth::plugin_routes::{
    async_handler, BoxFuture, Handler, RouteBundle, RouteCx, RouteSource,
};
use brocanth::routing::{Router, Verb};
use brocanth_test::{TestClient, TestRequest};

// ---------------------------------------------------------------------------
// One plugin: two hooks and three routes
// ---------------------------------------------------------------------------

/// What the hooks saw, so a test can say when they ran as well as whether.
#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<String>>>);

impl Seen {
    fn push(&self, line: impl Into<String>) {
        self.0.lock().expect("not poisoned").push(line.into());
    }

    fn take(&self) -> Vec<String> {
        self.0.lock().expect("not poisoned").clone()
    }
}

struct Probe {
    seen: Seen,
}

impl Plugin for Probe {
    type Config = Seen;
    type Provides = NoContext;
    const NAME: &'static str = "probe";
    const INSTALLS_HOOKS: bool = true;
    const CONTRIBUTES_ROUTES: bool = true;

    fn new(config: Self::Config) -> Self {
        Probe { seen: config }
    }
}

struct Hooks {
    seen: Seen,
}

impl RequestHook for Hooks {
    fn before_dispatch(
        &self,
        request: &mut Request,
        _state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        self.seen.push(format!("before {}", request.uri().path()));
        if request.uri().path() == "/stop" {
            let mut short = Response::new(Body::from(b"a hook answered".to_vec()));
            *short.status_mut() = http::StatusCode::ACCEPTED;
            return Ok(Flow::respond(short));
        }
        Ok(Flow::Continue)
    }

    fn after_dispatch(&self, _request: &Request, response: &Response, _state: &mut RequestState) {
        self.seen
            .push(format!("after {}", response.status().as_u16()));
    }
}

impl ResponseHook for Hooks {
    // The header half, which runs for every response including a
    // streaming one. Stamping a status is a header, so it belongs here.
    fn on_headers(
        &self,
        _request: &Request,
        head: &mut brocanth::plugin_hooks::ResponseHead,
        _body: brocanth::plugin_hooks::BodyShape<'_>,
        _state: &mut RequestState,
    ) -> Result<(), PluginError> {
        let status = head.status.as_u16();
        head.headers.insert(
            "x-seen",
            http::HeaderValue::from_str(&status.to_string()).expect("a number is a header value"),
        );
        Ok(())
    }
}

impl HookSource for Probe {
    type Families = Both<OnRequest, OnResponse>;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        registrar.request(
            "trace",
            Order::new(),
            Hooks {
                seen: self.seen.clone(),
            },
        );
        registrar.response(
            "stamp",
            Order::new(),
            Hooks {
                seen: self.seen.clone(),
            },
        );
        Ok(())
    }
}

impl<E: 'static> RouteSource<E> for Probe {
    const PREFIX: &'static str = "/";

    fn routes(&self, bundle: &mut RouteBundle<'_, E>) -> Result<(), PluginError> {
        bundle.get("screen", "/screen", screen());
        bundle.post("write", "/screen", redirect());
        bundle.get_async("greeting", "/hello/{who}", greeting());
        Ok(())
    }
}

/// A page with the two things a test asks about: where it links, and where
/// its form posts.
fn screen<E>() -> Handler<E> {
    Box::new(|_cx: &mut RouteCx<'_, E>| {
        Ok(html(
            200,
            r#"<p>Two waiting</p>
               <a href="/hello/ada">Ada</a>
               <form method="post" action="/screen">
                 <input name="reason" value="because">
                 <button>Empty</button>
               </form>"#,
        ))
    })
}

fn redirect<E>() -> Handler<E> {
    Box::new(|_cx: &mut RouteCx<'_, E>| {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = http::StatusCode::SEE_OTHER;
        response
            .headers_mut()
            .insert("location", http::HeaderValue::from_static("/screen"));
        Ok(response)
    })
}

/// A route that genuinely suspends, so driving it is not a sync call wearing
/// a future's clothes.
fn greeting<E: 'static>() -> brocanth::plugin_routes::AsyncHandler<E> {
    async_handler(|cx: &mut RouteCx<'_, E>| {
        let who = cx.params().get("who").unwrap_or("nobody").to_string();
        Box::pin(async move {
            yield_once().await;
            Ok(html(200, &format!("<h1>Hello {who}</h1>")))
        }) as BoxFuture<'_, _>
    })
}

fn html(status: u16, body: &str) -> Response {
    http::Response::builder()
        .status(status)
        .header("content-type", "text/html; charset=utf-8")
        .body(Body::from(body.as_bytes().to_vec()))
        .expect("a response")
}

fn client(seen: &Seen) -> TestClient<Base, impl HookFamilies> {
    let mut app = AppBuilder::new()
        .plugin_hooking(Probe::new(seen.clone()))
        .build()
        .expect("an application");
    app.mount_routes_default::<Probe>()
        .expect("the routes mount");
    TestClient::new(app)
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

#[test]
fn the_router_runs_and_the_handler_gets_the_parameter_out_of_the_path() {
    // What every test in this repository currently fakes: the existing ones
    // look a route up by name and hand it a `Params` they built themselves,
    // so nothing has ever checked that this path reaches this handler.
    let page = client(&Seen::default()).get("/hello/ada");
    page.assert_ok();
    page.html().assert_text("Hello ada");
}

#[test]
fn a_response_hook_sees_the_404_the_router_built() {
    // The statuses are owned inside the dispatch region, exactly as in
    // `brocanth-server`. A client that answered 404 before the hooks ran
    // would be a different application from the one that ships.
    let seen = Seen::default();
    let page = client(&seen).get("/nowhere");

    page.assert_status(404).assert_header("x-seen", "404");
    assert_eq!(seen.take(), ["before /nowhere", "after 404"]);
}

#[test]
fn a_request_hook_may_answer_before_any_route_does() {
    let seen = Seen::default();
    let page = client(&seen).get("/stop");

    page.assert_status(202);
    page.assert_contains("a hook answered");
    // The response hook still ran: a short circuit is an answer, not an
    // escape from the pipeline.
    page.assert_header("x-seen", "202");
    assert_eq!(seen.take(), ["before /stop", "after 202"]);
}

#[test]
fn a_path_that_exists_with_a_verb_that_does_not_is_a_405_with_an_allow() {
    client(&Seen::default())
        .send(TestRequest::delete("/screen"))
        .assert_status(405)
        .assert_header("allow", "GET, HEAD, POST");
}

#[test]
fn a_route_that_waits_answers_the_same_whether_it_is_blocked_on_or_awaited() {
    let client = client(&Seen::default());

    let blocked = client.get("/hello/grace");
    let awaited = brocanth_test::rt::block_on(client.get_async("/hello/grace"));

    assert_eq!(blocked.status(), awaited.status());
    assert_eq!(blocked.body(), awaited.body());
    awaited.html().assert_text("Hello grace");
}

#[test]
fn a_redirect_is_followed_and_the_chain_is_kept() {
    let client = client(&Seen::default());

    client
        .post("/screen", &[("reason", "because")])
        .assert_redirects("/screen");

    let landed = client.send(TestRequest::post("/screen").form(&[]).following());
    landed.assert_ok();
    assert_eq!(landed.redirect_chain(), ["/screen"]);
    landed.html().assert_text("Two waiting");
}

#[test]
fn a_page_is_asked_where_it_goes_rather_than_what_it_spells() {
    let page = client(&Seen::default()).get("/screen");
    let html = page.html();

    html.assert_link_to("/hello/ada");
    html.assert_form_posts_to("/screen");
    html.assert_field("reason");
    html.assert_button("Empty");
}

#[test]
fn a_table_the_application_built_itself_can_be_served_as_it_is() {
    // The other half of `TestClient::serving`: a scaffolded application's
    // pages are a `Router<Handler<E>>` that `main` builds, and a test of
    // those pages wants that table rather than a plan.
    let app = AppBuilder::new().build().expect("an application");
    let mut builder = Router::builder();
    builder
        .route("/pages/{slug}", Verb::Get, page_handler())
        .expect("a template");
    let client = TestClient::serving(app, builder.build().expect("a table"));

    client
        .get("/pages/about")
        .assert_ok()
        .html()
        .assert_link_to("/pages/about/edit");
    client.get("/pages").assert_status(404);
}

fn page_handler() -> Handler<Base> {
    Box::new(|cx: &mut RouteCx<'_, Base>| {
        let slug = cx.params().get("slug").unwrap_or("none").to_string();
        Ok(html(
            200,
            &format!(r#"<a href="/pages/{slug}/edit">Edit</a>"#),
        ))
    })
}

// ---------------------------------------------------------------------------
// A future that is `Pending` exactly once
// ---------------------------------------------------------------------------

fn yield_once() -> impl Future<Output = ()> {
    struct YieldOnce(bool);

    impl Future for YieldOnce {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            if self.0 {
                return Poll::Ready(());
            }
            self.0 = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }

    YieldOnce(false)
}
