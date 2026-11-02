# brocanth-server

The HTTP entrypoint of a brocanth application, on Tokio and hyper. It owns the
socket, one task per connection, HTTP/1.1 and HTTP/2 framing, streamed
response bodies and a graceful stop. It takes one async function, `Request` in
and `Response` out, and knows nothing about routes, pages or middleware. Use it
to serve an application. Turn on the `bridge` feature to serve a brocanth
`App` and `Router` without writing the dispatch yourself. Do not use it for
routing or for hooks: those live in the `brocanth` core, on the far side of
the function. hyper's types do not appear in the public API.

## Getting started

```toml
[dependencies]
brocanth-server = { git = "https://github.com/DHHwasRight/brocanth.git", features = ["bridge"] }
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

| Feature | Default | What it adds |
|---|---|---|
| `bridge` | off | the `bridge` module: `Bridge`, `bridge::serve`, and the conversions between this crate's `Request`/`Response` and the core's |

With `bridge` off, `src/bridge.rs` is not compiled and none of its API
exists. The feature removes code, not a crate: `brocanth` and `hyper` depend
on `http` either way.

The crate is not a module. `brocanth create` always adds it, with `bridge`
on, because every generated project serves.

## Examples

### Serve one function

The standalone shape: a socket, and a function the application writes.

```rust,no_run
use brocanth_server::{Response, Server};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    Server::bind("127.0.0.1:3000")
        .await?
        .run(|request| async move {
            Response::text(200, format!("you asked for {}", request.path))
        })
        .await?;
    Ok(())
}
```

`bind` fails before `run` starts, so the application can report a taken port
before it says it started.

### Stop the server

`Server::stopper` gives a handle that stops the accept loop. `run` returns
when the connections in flight have drained. `drain_grace` limits that wait.

```rust
use std::time::Duration;

use brocanth_server::{Response, Server};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Port 0: the system picks a free port.
    let server = Server::bind("127.0.0.1:0")
        .await?
        .drain_grace(Duration::from_secs(5));
    let stopper = server.stopper();

    let running = tokio::spawn(server.run(|_request| async { Response::text(200, "ok") }));
    stopper.stop();
    running.await??;
    Ok(())
}
```

### Test a handler without a socket

`Request::new` builds a request in process. A test calls the handler with it
and reads the response.

```rust
use brocanth::routing::Verb;
use brocanth_server::{Request, Response};

async fn handle(request: Request) -> Response {
    match request.header("accept") {
        Some("application/json") => Response::json(200, r#"{"ok":true}"#),
        _ => Response::html(200, "<p>ok</p>"),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let request = Request::new(Verb::Get, "/status").with_header("accept", "application/json");
    let response = handle(request).await;
    assert_eq!(response.status, 200);
    assert_eq!(response.header("content-type"), Some("application/json"));
    assert_eq!(response.body.bytes().map(|b| &b[..]), Some(&b"{\"ok\":true}"[..]));
}
```

### Serve a brocanth application (`bridge`)

`Bridge` pairs an `App` with its `Router`. `Bridge::handle` runs one request
through the hooks, the router and the handler, and answers 404 or 405 itself.
`bridge::serve(addr, app, router)` binds and serves in one call;
`Bridge::serve(server)` serves on a `Server` you built, so you keep its
stopper.

```rust
# #[cfg(feature = "bridge")]
# mod with_bridge {
use std::sync::Arc;

use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_body::Body;
use brocanth::plugin_context::Base;
use brocanth::plugin_hooks::Response as CoreResponse;
use brocanth::plugin_routes::Handler;
use brocanth::routing::{Router, Verb};
use brocanth_server::bridge::Bridge;
use brocanth_server::Request;

pub async fn run() {
    let app = AppBuilder::new().build().expect("the application builds");

    let mut routes = Router::<Handler<Base>>::builder();
    let index: Handler<Base> = Box::new(|_cx| Ok(CoreResponse::new(Body::from("home"))));
    routes.route("/", Verb::Get, index).expect("a valid route");
    let router = routes.build().expect("the table builds");

    let bridge = Arc::new(Bridge::new(app, router));
    assert_eq!(bridge.handle(Request::new(Verb::Get, "/")).await.status, 200);
    assert_eq!(bridge.handle(Request::new(Verb::Get, "/nope")).await.status, 404);
    assert_eq!(bridge.handle(Request::new(Verb::Post, "/")).await.status, 405);

    // In `main`: brocanth_server::bridge::serve("127.0.0.1:3000", app, router).await?;
}
# }
# #[tokio::main]
# async fn main() {
#     #[cfg(feature = "bridge")]
#     with_bridge::run().await;
# }
```

A sync route runs on `tokio::task::spawn_blocking`, so a slow handler does not
block a Tokio worker. The cost is one OS thread per concurrent sync request. A
route that can wait (`AnyHandler::Async`) runs inline and costs one task. The
`bridge` module docs give the measurements and the panic handling.

## Testing

```sh
cargo test -p brocanth-server
cargo test -p brocanth-server --features bridge
```

No environment variable is needed. The tests bind ephemeral ports on
`127.0.0.1`. `tests/bridges.rs` runs only with the `bridge` feature.

To test an application, do not start a server. Use `brocanth-test`:
`TestClient::serving(app, router)` drives the same pipeline the bridge drives,
and `TestResponse` reads the status, the headers and the HTML.

## Read more

- The crate docs: the M:N model, and where the sync core still stops.
- The `bridge` module docs: `spawn_blocking`, async routes, panics.
- `docs/async-handlers.md`: routes and request hooks that wait.
- `docs/observability.md`: readiness and the probes a server answers.
- `PLAN.md` §8 (features remove code) and ADR 2 (readiness before shutdown).
