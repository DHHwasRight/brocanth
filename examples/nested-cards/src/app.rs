//! Nested Cards -- everything this binary contains, in the order it happens.
//!
//! A plugin that is not named in [`build`] is not registered, and one that is
//! not in `Cargo.toml` is not compiled at all. "The framework decided for you
//! and you cannot see where" is the thing this layout exists to make
//! impossible.
//!
//! `serve` and every other command read this, so there is one answer to what
//! this application is.

use std::sync::Arc;

use brocanth::plugin_app::AppBuilder;
use brocanth::routing::{Dispatch, MiddlewareStack, Router};

use crate::pages::{self, Ctx, Handler, IntoResponse, Response};

/// The application name. Every environment variable this binary reads is
/// `NESTED_CARDS_<SUFFIX>`: one convention, derived once, whatever spelling the
/// name arrived in.
pub const APP: &str = "nested-cards";

/// What a request needs, built once and shared by every connection.
pub struct App {
    pub router: Router<Handler>,
    pub stack: MiddlewareStack<Ctx, Response>,
    pub plugins: Vec<String>,
}

/// Builds it. The only place that decides what this binary is.
pub fn build() -> Result<App, String> {
    // 1. Configuration. No file lookup, no merge order, no TOML parser: the
    //    core takes values that are already built, so each plugin below is
    //    handed a literal the compiler checks. Add the config plugin when a
    //    file is wanted; nothing else changes.

    // 2. The application, and every plugin it contains. Each line is a line
    //    a reader can delete; nothing registers itself.
    let app = AppBuilder::new()
        .setup(|setup| {
            // A plugin *offers* middleware and cannot mount it: a
            // `MiddlewareCatalog` has no `mount`. Mounting takes a pattern, the
            // pattern is a decision, and the decision is the application's.
            // Nothing in this binary offers middleware, so nothing is mounted.
            let _ = setup.router();
            Ok(())
        })
        .build()
        .map_err(|e| e.to_string())?;

    // 3. The route table: the shape of src/pages and nothing else. A file is
    //    a route, and `pages::router` is the only place that says so.
    let router = pages::router().map_err(|e| e.to_string())?;

    // 4. The chain. Nothing offered middleware, so the stack is empty --
    //    and an empty stack costs nothing: `run` allocates only when a
    //    layer matches the path.
    let stack: MiddlewareStack<Ctx, Response> = MiddlewareStack::new();

    Ok(App {
        router,
        stack,
        plugins: app.instance_names().map(str::to_string).collect(),
    })
}

/// Turns a page's answer into the bytes the server writes.
///
/// `html` rather than a guess: every page returns markup, and a handler that
/// wants another type says so with a `content-type` header, which is applied
/// after and therefore wins.
fn sent(answered: Response) -> brocanth_server::Response {
    let mut out = brocanth_server::Response::html(answered.status, answered.body);
    for (name, value) in &answered.headers {
        out = out.with_header(name, value);
    }
    out
}

/// The file a request named, or `None` to let the router answer.
///
/// Checked before the route table, which is also the order of surprise: a
/// page at the same path as a file would be shadowed, and a path under
/// `/assets` or `/public` is not a route anybody writes by hand.
fn serve_asset(request: &brocanth_server::Request) -> Option<brocanth_server::Response> {
    if !matches!(
        request.verb,
        brocanth::routing::Verb::Get | brocanth::routing::Verb::Head
    ) {
        return None;
    }
    let asset = crate::assets::lookup(&request.path)?;

    // The ETag is the content hash, so it is strong and a match means the
    // bytes are identical. 304 is the whole point of serving `public/`
    // without a fingerprint in the name.
    if request.header("if-none-match") == Some(asset.etag) {
        return Some(
            brocanth_server::Response::new(304, asset.content_type, &[][..])
                .with_header("etag", asset.etag)
                .with_header("cache-control", asset.cache_control()),
        );
    }

    Some(
        brocanth_server::Response::new(200, asset.content_type, asset.body)
            .with_header("etag", asset.etag)
            .with_header("cache-control", asset.cache_control()),
    )
}

/// How many layout layers the client already has, from `DP-Partial`.
///
/// `html!` leaves out that many layouts, counting from the root, so a
/// navigation inside a section sends the section's contents and not the
/// document around them. dp-swap sends the number; the header is absent on
/// a first load, for a crawler, and whenever JavaScript is off, and then
/// this is zero and the whole document goes out.
///
/// `true` is dp-swap's word for "a fragment, I did not count", which is one
/// layer. Anything that does not parse is zero: a client that cannot say
/// what it holds gets everything, which is always correct and only slower.
fn holding(request: &brocanth_server::Request) -> u32 {
    match request.header("dp-partial") {
        None => 0,
        Some("true") => 1,
        Some(value) => value.parse().unwrap_or(0),
    }
}

/// Serves until the process is stopped.
///
/// Synchronous on the outside because the command that calls it is, and
/// the runtime is built here rather than wrapped around `main`: a command
/// that never serves should not start a reactor to run.
pub fn serve(addr: &str) -> Result<(), String> {
    let app = Arc::new(build()?);
    eprintln!("{APP}: plugins: [{}]", app.plugins.join(", "));

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    runtime.block_on(async move {
        let server = brocanth_server::Server::bind(addr)
            .await
            .map_err(|e| e.to_string())?;
        let bound = server.local_addr().map_err(|e| e.to_string())?;
        eprintln!("{APP}: listening on http://{bound}");

        server
            .run(move |request| {
                // Dispatch is synchronous and runs here, outside the
                // future, so what crosses the await is a finished
                // response and nothing borrowed.
                let out = serve_asset(&request).unwrap_or_else(|| sent(dispatch(&app, &request)));
                async move { out }
            })
            .await
            .map_err(|e| e.to_string())
    })
}

/// One request: the table, then the chain, then the handler.
///
/// The chain runs only for a route that matched. A 404 has no route and
/// therefore no context, which is also why the request hooks the plan
/// reserves for a logger are the core's job and not a layer's.
pub fn dispatch(app: &App, request: &brocanth_server::Request) -> Response {
    let (verb, path) = (request.verb, request.path.as_str());
    match app.router.dispatch(verb, path) {
        Dispatch::Found { handler, matched } => {
            let mut ctx = Ctx::new(verb, path, matched.params())
                .holding(holding(request))
                .sent(
                    &request.query,
                    request.body.to_vec(),
                    request
                        .headers()
                        .map(|(name, value)| (name.to_string(), value.to_string()))
                        .collect(),
                );
            // The one place a failed handler becomes a response --
            // what Echo calls an HTTPErrorHandler. It sits inside
            // the chain's terminal, so a layer still sees what the
            // error turned into and can rewrite it.
            app.stack.run(path, &mut ctx, |c| {
                handler(c).unwrap_or_else(IntoResponse::into_response)
            })
        }
        Dispatch::MethodNotAllowed { allow, .. } => Response {
            status: 405,
            headers: vec![("allow".to_string(), allow.to_string())],
            ..Default::default()
        },
        Dispatch::NotFound => Response::new(404, "Not Found"),
    }
}
