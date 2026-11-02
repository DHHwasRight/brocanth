# brocanth

The core of the brocanth framework. It is a router that hands a request and a
context to a handler, plus the plugin seams around that router: the
application lifecycle, request and response hooks, error hooks, model events,
readiness, typed context values and middleware offers. Use it in every
brocanth application, and in every plugin crate, because each plugin
implements its traits. Do not look here for a socket, a template engine, a
config parser or a database driver. The core has three dependencies (`wayfind`,
`http`, `thiserror`), and `tests/no_hidden_dependencies.rs` fails the build if
a fourth kind arrives. The socket is `brocanth-server`. Each other capability
is a plugin crate that the application names in `main`.

## Getting started

```toml
[dependencies]
brocanth = { git = "https://github.com/DHHwasRight/brocanth.git" }
```

The crate has no features. It is not a module: `brocanth create` always adds
it, so there is no `--with` key for it. A generated project serves through
`brocanth-server` with the `bridge` feature, and reads its pages with
`brocanth-pages`.

## Examples

### A route table, with 404 and 405

A `Router<H>` holds any handler type `H`. A path that exists with the wrong
verb gives `MethodNotAllowed` and the `Allow` set, never `NotFound`.

```rust
use brocanth::routing::{Dispatch, Router, Verb};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let mut builder = Router::builder();
builder.route("/users/{id}", Verb::Get, "show")?;
builder.route("/users/{id}", Verb::Delete, "destroy")?;
let router = builder.build()?;

match router.dispatch(Verb::Get, "/users/42") {
    Dispatch::Found { handler, matched } => {
        assert_eq!(*handler, "show");
        assert_eq!(matched.params().get("id"), Some("42"));
    }
    other => panic!("expected a match, got {other:?}"),
}

match router.dispatch(Verb::Post, "/users/42") {
    Dispatch::MethodNotAllowed { allow, .. } => {
        assert_eq!(allow.to_string(), "GET, HEAD, DELETE");
    }
    other => panic!("expected 405, got {other:?}"),
}
assert!(matches!(router.dispatch(Verb::Get, "/nowhere"), Dispatch::NotFound));
# Ok(())
# }
```

### A plugin with a request hook

A plugin that hooks implements `HookSource` and is registered with
`plugin_hooking`, not `plugin`. The application type records the installed
hook families, so an application with no hooks pays nothing for them.

```rust
use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_body::Body;
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{
    Flow, HookRegistrar, HookSource, OnRequest, Request, RequestHook, RequestState, Response,
};
use brocanth::plugin_order::Order;

/// Answers 403 for every path under `/private`.
struct Guard;

impl Plugin for Guard {
    type Config = ();
    type Provides = NoContext;
    const NAME: &'static str = "guard";
    const INSTALLS_HOOKS: bool = true;

    fn new((): ()) -> Self {
        Guard
    }
}

impl HookSource for Guard {
    type Families = OnRequest;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        registrar.request("deny", Order::new(), Deny);
        Ok(())
    }
}

struct Deny;

impl RequestHook for Deny {
    fn before_dispatch(
        &self,
        request: &mut Request,
        _state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        if request.uri().path().starts_with("/private") {
            let mut denied = Response::new(Body::from("forbidden"));
            *denied.status_mut() = http::StatusCode::FORBIDDEN;
            return Ok(Flow::respond(denied));
        }
        Ok(Flow::Continue)
    }
}

let app = AppBuilder::new()
    .plugin_hooking(Guard::new(()))
    .build()
    .expect("the application builds");

let mut request = Request::new(Vec::new());
*request.uri_mut() = "/private/report".parse().unwrap();
let response = app.run(&mut request, &mut RequestState::new(), |_, _| {
    unreachable!("the hook answers first")
});
assert_eq!(response.status(), 403);
assert_eq!(response.body().buffered(), Some(&b"forbidden"[..]));
```

`app.run` takes the handler as its last argument. In an application,
`brocanth-server`'s bridge calls `run` and passes the router's dispatch.

### Typed context values

A plugin publishes a value onto the per-request context. The set of installed
providers is part of the context's type, so a handler that asks for a value
nobody provides does not compile.

```rust
use brocanth::plugin_context::{Base, ContextBuilder, ContextItem, With};

struct RequestId(String);
impl ContextItem for RequestId {}

let mut builder = ContextBuilder::new();
builder.insert(RequestId("abc".into()));
let cx = builder.build::<With<RequestId, Base>>();

let id: &RequestId = cx.extract();
assert_eq!(id.0, "abc");
```

In an application, a plugin names `ProvidesContext<RequestId>` as its
`Plugin::Provides` and fills the value in `Plugin::provide`. The crate docs
(`plugin_context`) show the `compile_fail` case.

## Testing

```sh
cargo test -p brocanth
```

No environment variable is needed. Two tests shell out to `cargo`:
`tests/hooks_cost_nothing.rs` reads the LLVM IR of the core, and
`tests/no_hidden_dependencies.rs` reads `cargo tree`. The first run of each
takes a few seconds.

To test an application built on this crate, use `brocanth-test`.
`TestClient::serving(app, router)` sends a request through the hooks, the
router and the handler, and `TestResponse::html()` gives assertions on
elements, links and forms. Assert on behaviour, not on CSS class names.

## Read more

- The crate docs below this README: the seven seams, hook order and its cost.
- `PLAN.md`: the design. §13 holds the ADRs (ADR 1 is the hook cost, ADR 2
  is readiness).
- `docs/modules.md`: how build-time modules and run-time plugins attach.
- `docs/module-authoring.md`: how to write a plugin against these seams.
- `docs/async-handlers.md`: async request hooks and routes that wait.
- `docs/middleware.md`: middleware offers and mounts.
