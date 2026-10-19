//! A built [`App`], driven through a request.
//!
//! # The same pipeline the server runs
//!
//! `brocanth-server`'s `bridge::Bridge::blocking` is thirty lines: build the
//! context, make a [`RequestState`], run the request hooks, ask the router,
//! call the handler, run the response hooks. [`TestClient::once`] is those
//! same thirty lines with the socket and the thread pool taken out, and the
//! statuses are owned in the same place -- the 404, the 405 and the 501 are
//! built *inside* the dispatch closure, so a response hook and
//! `after_dispatch` see them exactly as they would in production. A client
//! that answered 404 before the hooks ran would be a different application
//! from the one that ships.
//!
//! What is deliberately *not* here is the socket: no bytes are framed, no
//! `content-length` is computed and no connection is reused. A test that
//! needs those is a test of `brocanth-server`, and that crate has them.
//!
//! # Which table is served
//!
//! [`TestClient::new`] builds the route table out of
//! [`App::route_plan`] -- every route the application mounted from a plugin,
//! at the prefix the application chose. That is the one thing every test in
//! this repository currently fakes: `brocanth-resource/tests/routes.rs`
//! reaches into the plan by route *name* and builds its own `Params`, so no
//! test has ever exercised the path matching, the 404 or the 405.
//!
//! [`TestClient::serving`] takes a [`Router`] the application built itself,
//! which is how a scaffolded application's own pages are tested: the table
//! `main` serves, served.
//!
//! # Who is asking
//!
//! A screen behind a policy needs an identity, and the core's way of carrying
//! one is the [`RequestState`] that an authentication hook writes into.
//! [`TestClient::seeding`] is Django's `force_login`: it fills a fresh
//! `RequestState` at the point a request hook would have, before
//! `before_dispatch` runs. It is a seam into the production path and not a
//! way round it -- the policy, the principal closure and the screen all run.

use std::sync::Arc;

use brocanth::plugin_app::App;
use brocanth::plugin_hooks::{Begin, HandlerError, HookFamilies, Request, RequestState, Response};
use brocanth::plugin_routes::{AnyHandler, Handler, RouteCx, RoutePlan};
use brocanth::routing::{Allow, Dispatch, Router, Verb};

use crate::request::TestRequest;
use crate::response::TestResponse;
use crate::rt::block_on;

/// A future this crate drives on the thread that made it.
///
/// Not [`brocanth::plugin_routes::BoxFuture`], which is `Send`, because
/// nothing here crosses a task boundary and requiring `Send` would make a
/// perfectly good test handler fail to compile for a reason the test cannot
/// act on.
pub type TestFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + 'a>>;

/// What [`TestClient::seeding`] holds: the application's authentication hook,
/// stood in for.
type Seed = Arc<dyn Fn(&mut RequestState) + Send + Sync>;

// ---------------------------------------------------------------------------
// What a route table may hold
// ---------------------------------------------------------------------------

/// What this client needs to know about whatever the route table holds.
///
/// The same shape as `brocanth-server`'s `RouteKind`, with one difference:
/// the plan is passed in, so a table may hold an *index* into the
/// application's own [`RoutePlan`] rather than a handler. That is what lets
/// [`TestClient::new`] build a table from a plan it only borrows -- a
/// `Box<dyn Fn>` cannot be taken out of an `App` that is still running.
pub trait TestRoute<E>: Send + Sync + 'static {
    /// Runs the route on this thread, waiting here if it waits.
    fn run(&self, plan: &RoutePlan<E>, cx: &mut RouteCx<'_, E>) -> Result<Response, HandlerError>;

    /// Runs the route as a future, for a caller that has a runtime.
    fn run_async<'a>(
        &'a self,
        plan: &'a RoutePlan<E>,
        cx: &'a mut RouteCx<'_, E>,
    ) -> TestFuture<'a, Result<Response, HandlerError>>;
}

/// One route of the application's own [`RoutePlan`], by position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Planned(usize);

impl<E: 'static> TestRoute<E> for Planned {
    fn run(&self, plan: &RoutePlan<E>, cx: &mut RouteCx<'_, E>) -> Result<Response, HandlerError> {
        let handler = &plan.routes()[self.0].handler;
        match handler.call_sync(cx) {
            Some(outcome) => outcome,
            // A contributed route that waits. The admin's four are all of
            // this kind, so this is the common arm rather than the odd one.
            None => block_on(handler.call(cx)),
        }
    }

    fn run_async<'a>(
        &'a self,
        plan: &'a RoutePlan<E>,
        cx: &'a mut RouteCx<'_, E>,
    ) -> TestFuture<'a, Result<Response, HandlerError>> {
        Box::pin(plan.routes()[self.0].handler.call(cx))
    }
}

impl<E: 'static> TestRoute<E> for Handler<E> {
    fn run(&self, _plan: &RoutePlan<E>, cx: &mut RouteCx<'_, E>) -> Result<Response, HandlerError> {
        self(cx)
    }

    fn run_async<'a>(
        &'a self,
        _plan: &'a RoutePlan<E>,
        cx: &'a mut RouteCx<'_, E>,
    ) -> TestFuture<'a, Result<Response, HandlerError>> {
        Box::pin(std::future::ready(self(cx)))
    }
}

impl<E: 'static> TestRoute<E> for AnyHandler<E> {
    fn run(&self, _plan: &RoutePlan<E>, cx: &mut RouteCx<'_, E>) -> Result<Response, HandlerError> {
        match self.call_sync(cx) {
            Some(outcome) => outcome,
            None => block_on(self.call(cx)),
        }
    }

    fn run_async<'a>(
        &'a self,
        _plan: &'a RoutePlan<E>,
        cx: &'a mut RouteCx<'_, E>,
    ) -> TestFuture<'a, Result<Response, HandlerError>> {
        Box::pin(self.call(cx))
    }
}

// ---------------------------------------------------------------------------
// The client
// ---------------------------------------------------------------------------

/// Django's `self.client`, for a brocanth application.
///
/// ```ignore
/// let client = TestClient::new(app).seeding(|state| {
///     state.insert(CurrentUser { staff: true });
/// });
///
/// let page = client.get("/admin/jobs?page=2");
/// page.assert_ok();
/// page.html()
///     .within("nav", "aria-label", "Sections")
///     .assert_link_to("/admin/post");
///
/// client.post("/admin/jobs", &[]).assert_redirects("/admin/jobs/retries");
/// ```
pub struct TestClient<E, H, R = Planned> {
    app: App<E, H>,
    router: Router<R>,
    headers: Vec<(String, String)>,
    seed: Option<Seed>,
    max_redirects: usize,
}

impl<E: 'static, H> TestClient<E, H, Planned> {
    /// A client over every route the application mounted.
    ///
    /// # Panics
    ///
    /// When the mounted routes do not make a table — two templates that
    /// cannot coexist, or a path the matcher refuses. The mount itself
    /// already rejected a duplicate path, so reaching this is a bug in the
    /// matcher rather than in the test.
    #[must_use]
    pub fn new(app: App<E, H>) -> Self {
        let mut builder = Router::builder();
        for (index, route) in app.route_plan().routes().iter().enumerate() {
            if let Err(e) = builder.route(&route.path, route.verb, Planned(index)) {
                panic!(
                    "`{}` contributes `{} {}`, which this table cannot hold: {e}",
                    route.source, route.verb, route.path
                );
            }
        }
        let router = builder
            .build()
            .unwrap_or_else(|e| panic!("the mounted routes do not make a table: {e}"));
        Self::serving(app, router)
    }
}

impl<E: 'static, H, R: TestRoute<E>> TestClient<E, H, R> {
    /// A client over a table the application built itself.
    ///
    /// The routes the application mounted from plugins are *not* added: this
    /// serves exactly what it is given, which is the point — it is for
    /// driving the table `main` serves.
    #[must_use]
    pub fn serving(app: App<E, H>, router: Router<R>) -> Self {
        TestClient {
            app,
            router,
            headers: Vec::new(),
            seed: None,
            max_redirects: 10,
        }
    }

    /// The application, for a test that wants to read it as well as drive it.
    #[must_use]
    pub fn app(&self) -> &App<E, H> {
        &self.app
    }

    /// A header on every request this client sends.
    #[must_use]
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// What to put in the [`RequestState`] of every request, before the
    /// request hooks run.
    ///
    /// Where an authentication hook would have written. See the module
    /// header for why this is a seam into the production path rather than a
    /// way round it.
    #[must_use]
    pub fn seeding<F>(mut self, f: F) -> Self
    where
        F: Fn(&mut RequestState) + Send + Sync + 'static,
    {
        self.seed = Some(Arc::new(f));
        self
    }

    /// How many redirects
    /// [`following`](crate::TestRequest::following) will follow before it
    /// calls the chain a loop. Ten by default.
    #[must_use]
    pub fn following_at_most(mut self, hops: usize) -> Self {
        self.max_redirects = hops;
        self
    }
}

impl<E: 'static, H: HookFamilies, R: TestRoute<E>> TestClient<E, H, R> {
    /// `GET`, with the query in the target: `get("/admin/jobs?page=2")`.
    ///
    /// # Panics
    ///
    /// As [`TestClient::send`].
    pub fn get(&self, target: &str) -> TestResponse {
        self.send(TestRequest::get(target))
    }

    /// `POST`, with a form body, which is how a browser submits one.
    ///
    /// # Panics
    ///
    /// As [`TestClient::send`].
    pub fn post(&self, target: &str, form: &[(&str, &str)]) -> TestResponse {
        self.send(TestRequest::post(target).form(form))
    }

    /// One request, on this thread.
    ///
    /// A route that waits is driven by [`crate::rt::block_on`], which parks
    /// the calling thread. That is right for a handler whose futures are
    /// ready — an in-memory fixture, a formatter — and wrong for one waiting
    /// on a Tokio timer or socket, which has no driver to wake it here. Use
    /// [`TestClient::send_async`] inside a `#[tokio::test]` for that.
    ///
    /// An application that installed an
    /// [`AsyncRequestHook`](brocanth::plugin_hooks::AsyncRequestHook) cannot
    /// use this at all, and the core says so at compile time: this calls
    /// [`App::run`], whose `const` assertion names
    /// [`App::begin_async`] as the replacement. [`TestClient::send_async`] is
    /// the client's version of that advice.
    ///
    /// # Panics
    ///
    /// When a followed redirect carries no `location`, or when the chain is
    /// longer than [`TestClient::following_at_most`].
    pub fn send(&self, request: TestRequest) -> TestResponse {
        let mut chain = Vec::new();
        let mut current = request;
        loop {
            let response = self.once(&current);
            match self.next_hop(&current, &response, &chain) {
                None => return response.with_chain(chain),
                Some(next) => {
                    chain.push(next.target().to_string());
                    current = next;
                }
            }
        }
    }

    /// [`TestClient::get`], for a caller that has a runtime.
    ///
    /// # Panics
    ///
    /// As [`TestClient::send`].
    pub async fn get_async(&self, target: &str) -> TestResponse {
        self.send_async(TestRequest::get(target)).await
    }

    /// [`TestClient::post`], for a caller that has a runtime.
    ///
    /// # Panics
    ///
    /// As [`TestClient::send`].
    pub async fn post_async(&self, target: &str, form: &[(&str, &str)]) -> TestResponse {
        self.send_async(TestRequest::post(target).form(form)).await
    }

    /// One request, awaited rather than blocked on.
    ///
    /// The entry point for an application whose routes or request hooks wait
    /// on something real. It runs [`App::begin_async`], so an async request
    /// hook is run rather than walked past.
    ///
    /// # Panics
    ///
    /// As [`TestClient::send`].
    pub async fn send_async(&self, request: TestRequest) -> TestResponse {
        let mut chain = Vec::new();
        let mut current = request;
        loop {
            let response = self.once_async(&current).await;
            match self.next_hop(&current, &response, &chain) {
                None => return response.with_chain(chain),
                Some(next) => {
                    chain.push(next.target().to_string());
                    current = next;
                }
            }
        }
    }

    /// One request, synchronously: hooks, router, handler, hooks.
    ///
    /// `brocanth-server`'s `Bridge::blocking`, without the socket.
    fn once(&self, request: &TestRequest) -> TestResponse {
        let mut core = request.build(&self.headers);
        let context = self.app.context();
        let mut state = self.state();
        let plan = self.app.route_plan();

        let response = self.app.run(&mut core, &mut state, |core, state| {
            // `core` is `&mut` for the hooks that rewrite it; dispatch only
            // reads, and `RouteCx` borrows it shared.
            let read: &Request = core;
            let Some(verb) = Verb::from_http(read.method()) else {
                return Ok(plain(501, "Not Implemented"));
            };
            match self.router.dispatch(verb, read.uri().path()) {
                Dispatch::Found { handler, matched } => {
                    let params = matched.params();
                    let mut cx = RouteCx::new(read, &params, &context, state);
                    handler.run(plan, &mut cx)
                }
                // Built here and not returned early, so the response hooks
                // and `after_dispatch` see them like any other response.
                Dispatch::MethodNotAllowed { allow, .. } => Ok(method_not_allowed(allow)),
                Dispatch::NotFound => Ok(plain(404, "Not Found")),
            }
        });

        TestResponse::from_core(response)
    }

    /// One request, with the `.await` where the bridge puts it.
    ///
    /// `brocanth-server`'s `Bridge::inline`, without the socket: the same
    /// pipeline split into [`App::begin_async`], the handler, and
    /// [`App::finish`], so the core never holds a future.
    async fn once_async(&self, request: &TestRequest) -> TestResponse {
        let mut core = request.build(&self.headers);
        let context = self.app.context();
        let mut state = self.state();
        let plan = self.app.route_plan();

        let outcome = match self.app.begin_async(&mut core, &mut state).await {
            Begin::Answered(outcome) => outcome,
            Begin::Dispatch => {
                let read: &Request = &core;
                match Verb::from_http(read.method()) {
                    None => Ok(plain(501, "Not Implemented")),
                    Some(verb) => match self.router.dispatch(verb, read.uri().path()) {
                        Dispatch::Found { handler, matched } => {
                            let params = matched.params();
                            let mut cx = RouteCx::new(read, &params, &context, &mut state);
                            handler.run_async(plan, &mut cx).await
                        }
                        Dispatch::MethodNotAllowed { allow, .. } => Ok(method_not_allowed(allow)),
                        Dispatch::NotFound => Ok(plain(404, "Not Found")),
                    },
                }
            }
        };

        TestResponse::from_core(self.app.finish(&mut core, &mut state, outcome))
    }

    /// A fresh request state, with whatever
    /// [`TestClient::seeding`] puts in one.
    fn state(&self) -> RequestState {
        let mut state = RequestState::new();
        if let Some(seed) = &self.seed {
            seed(&mut state);
        }
        state
    }

    /// Where a followed request goes next, or nothing when it has arrived.
    fn next_hop(
        &self,
        request: &TestRequest,
        response: &TestResponse,
        chain: &[String],
    ) -> Option<TestRequest> {
        if !request.follows() || !response.is_redirect() {
            return None;
        }
        let location = response.location().unwrap_or_else(|| {
            panic!(
                "`{} {}` answered {} with no `location`, so there is nowhere to follow",
                request.verb(),
                request.target(),
                response.status()
            )
        });
        assert!(
            chain.len() < self.max_redirects,
            "followed {} redirects and the chain has not ended: {} -> {location}",
            chain.len(),
            chain.join(" -> ")
        );
        // 307 and 308 keep the verb and the body; every other redirect a
        // browser follows with a GET.
        Some(request.redirected_to(location, matches!(response.status(), 307 | 308)))
    }
}

/// A response with a status, a plain-text body and nothing else.
fn plain(status: u16, body: &'static str) -> Response {
    http::Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .body(brocanth::plugin_body::Body::from(body.as_bytes().to_vec()))
        .expect("a response this crate wrote")
}

/// 405, carrying the verbs the path does answer.
///
/// The same answer `brocanth-server` gives, so a test that asserts on a 405
/// is asserting on what a browser would get.
fn method_not_allowed(allow: Allow) -> Response {
    http::Response::builder()
        .status(405)
        .header("content-type", "text/plain; charset=utf-8")
        .header("allow", allow.header_value())
        .body(brocanth::plugin_body::Body::from(b"Method Not Allowed".to_vec()))
        .expect("a response this crate wrote")
}
