# brocanth-openapi

`brocanth-openapi` derives an OpenAPI 3.1 document from a brocanth route
table. The router knows every path, verb and path parameter. You add
summaries and body schemas where you want them. The crate builds the
document, validates it, serves it as JSON and YAML, and compares it with a
committed copy, so an out-of-date spec fails the build. Use it when a client
outside your code reads your HTTP API. Do not use it to generate clients or
to run a "try it" console: point an existing generator or viewer
(Swagger UI, Redoc, Scalar) at the document instead.

## Getting started

```toml
[dependencies]
brocanth-openapi = { git = "https://github.com/DHHwasRight/brocanth.git" }
# or, inside this workspace:
# brocanth-openapi = { path = "crates/brocanth-openapi" }
```

| Feature | Default | What it adds |
|---|---|---|
| `admin` | off | `admin::ApiPanel`, the document as an "API" tab in `brocanth-admin` |
| `zod` | off | `SpecBuilder::describing::<T>()` and `body_for::<T>()` for a struct that derives `zod::Entry` |
| `scaffold` | off | the `openapi` module for `brocanth create`. Only the CLI turns it on |

### In a generated project

```sh
brocanth create blog --with openapi
```

This writes `src/plugins/openapi/mod.rs` (the plugin that derives the
document from `crate::pages::router()`), and two pages:
`src/pages/openapi.json.rs` and `src/pages/openapi.yaml.rs`. Add `input` too
(`--with openapi,input`), and the request and response bodies come from the
handler signatures.

`brocanth add openapi` works in an existing project. The module changes
`src/pages/mod.rs`: if you have not edited that file, the command rewrites
it. If you have, it prints the change, writes nothing, and goes through
once you have made the change by hand.

The module declares two settings (see `docs/module-flags.md`):

| Key | Flag | Variable | Default |
|---|---|---|---|
| `title` | `--openapi.title` | `$<APP>_OPENAPI__TITLE` | the project title |
| `version` | `--openapi.version` | `$<APP>_OPENAPI__VERSION` | `0.1.0` |

With the `config` module the keys live in `[openapi]` in
`config/<env>.toml`:

```toml
[openapi]
title = "Blog"
version = "1.2.0"
```

With `config`, the binary refuses flags until you set
`flags_with_config: true` in `src/modules/brocanth.rs`. The environment variables always work.
`version` is the version of the contract, not of the crate: change it when
an operation changes.

## Examples

### Derive a document from a router

```rust
use brocanth::routing::{Router, Verb};
use brocanth_openapi::{Schema, SchemaSource, SpecBuilder};

/// A body type that describes itself.
struct Post;

impl SchemaSource for Post {
    fn schema_name() -> Option<&'static str> {
        Some("Post")
    }
    fn schema() -> Schema {
        Schema::object()
            .required("id", Schema::integer())
            .required("title", Schema::string().max_length(120))
            .property("body", Schema::string().nullable())
    }
}

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let mut routes = Router::builder();
routes.route("/posts", Verb::Get, "index")?;
routes.route("/posts/{id}", Verb::Get, "show")?;
let router = routes.build()?;

let document = SpecBuilder::new("Blog", "1.0.0")
    .component_of::<Post>()
    .routes_from_router(&router)
    .describe("/posts", Verb::Get, |op| {
        op.summary("List posts")
            .query_param("page", Schema::integer().minimum(1.0))
            .json_response(200, "Every post", Schema::array(Schema::reference("Post")))
    })
    // `/posts/{id}` has no description and is still a valid operation:
    // an operationId, its path parameter and a 200.
    .build()?;

assert_eq!(document.operation_count(), 2);
let show = document.path("/posts/{id}").unwrap().operation(Verb::Get).unwrap();
assert_eq!(show.operation_id, "get_posts_by_id");
# Ok(())
# }
```

`build` refuses a duplicate `operationId` and two paths that differ only in
a parameter name (`/users/{id}` and `/users/{user_id}`).

### Serve it

`OpenApiPlugin` serves the document at `/openapi.json` and `/openapi.yaml`.
Its `configure` hook validates the document, so an invalid document stops
the application at startup.

```rust
use brocanth::plugin_api::Plugin;
use brocanth_openapi::{OpenApiConfig, OpenApiPlugin, SpecBuilder};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let document = SpecBuilder::new("Blog", "1.0.0").route("/posts", brocanth::routing::Verb::Get).build()?;
let plugin = OpenApiPlugin::new(OpenApiConfig::new(document).yaml_route("/spec.yaml"));

let response = plugin.serve("/openapi.json").expect("our route");
assert_eq!(response.status(), 200);
assert_eq!(response.headers()["cache-control"], "no-cache");
assert!(plugin.serve("/spec.yaml").is_some());
assert!(plugin.serve("/openapi.yaml").is_none());
# Ok(())
# }
```

`OpenApiPlugin::mount` adds both routes to a `RouterBuilder`. Another plugin
reads the document with `brocanth_openapi::document_of(services)`.

### Fail the build when the committed spec drifts

```rust
use brocanth::routing::Verb;
use brocanth_openapi::{check, render, DriftError, Format, SpecBuilder};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let old = SpecBuilder::new("Blog", "1.0.0").route("/posts", Verb::Get).build()?;
let committed = render(&old, Format::Json);

// Same document: no drift.
check(&old, &committed, Format::Json)?;

// A new route the committed file does not have: drift.
let new = SpecBuilder::new("Blog", "1.0.0")
    .route("/posts", Verb::Get)
    .route("/posts", Verb::Post)
    .build()?;
assert!(matches!(check(&new, &committed, Format::Json), Err(DriftError::Drifted(_))));
# Ok(())
# }
```

In CI, call `check_file(&document, "openapi.json")` from a test. It picks the
format from the extension. `write(&document, "openapi.json")` refreshes the
file. `examples/blog.rs` shows both:

```sh
cargo run -p brocanth-openapi --example blog -- --write openapi.json
cargo run -p brocanth-openapi --example blog -- --check openapi.json
```

### The API tab in the admin (`admin` feature)

`ApiPanel` lists every operation, and shows one screen per operation with
its parameters, body and responses. It is read only and sends no requests.

```rust
# #[cfg(feature = "admin")]
# fn main() -> Result<(), Box<dyn std::error::Error>> {
use std::sync::Arc;

use brocanth::routing::Verb;
use brocanth_admin::policy::{policy_fn, Principal, Request};
use brocanth_admin::{Admin, AdminConfig, Fixture, Submission};
use brocanth_openapi::admin::ApiPanel;
use brocanth_openapi::SpecBuilder;

let document = Arc::new(SpecBuilder::new("Blog", "1.0.0").route("/posts", Verb::Get).build()?);

let admin = Admin::new(
    Arc::new(Fixture::new()),
    AdminConfig::new()
        .panel(ApiPanel::new(document))
        .with_policy(policy_fn(|_: &Principal<'_>, _: Request<'_>| true)),
);

struct Operator;
let page = brocanth_admin::rt::block_on(admin.dispatch(
    Principal::new(&Operator),
    "GET",
    "/admin/api",
    "",
    &Submission::new(),
))?;
assert_eq!(page.status(), 200);
# Ok(())
# }
# #[cfg(not(feature = "admin"))]
# fn main() {}
```

With `OpenApiPlugin`, pass `plugin.shared()` to `ApiPanel::new`. Use
`.at("docs")` when you already have a model with the key `api`.

### Bodies from `zod` (`zod` feature)

A struct that derives `zod::Entry` registers itself, and every struct it
holds, under `components/schemas`.

```rust
# #[cfg(feature = "zod")]
# fn main() -> Result<(), Box<dyn std::error::Error>> {
use brocanth_openapi::SpecBuilder;
use zod::{Entry, Int, Text};

#[derive(Entry)]
struct Signup {
    #[text(min = 3, max = 16, slug)]
    handle: String,
    #[int(min = 13, max = 130)]
    age: i64,
}

let document = SpecBuilder::new("Accounts", "1.0.0")
    .describing::<Signup>()
    .build()?;
let json = document.to_json().to_json();
assert!(json.contains("\"Signup\""));
# Ok(())
# }
# #[cfg(not(feature = "zod"))]
# fn main() {}
```

`brocanth-schema` is the other source: `adapter::schema_for` turns a
`SchemaNode` into a `Schema`. A `SchemaNode` has no name, so you name the
component yourself.

## Testing

```sh
cargo test -p brocanth-openapi
cargo test -p brocanth-openapi --all-features   # the panel and zod tests too
```

No database and no environment variables. `tests/generated_document.rs`
checks a derived document. `tests/panel.rs` (feature `admin`) renders the
API tab through a real `Admin` and reads it with `brocanth_test::Html`.

In an application, test two things:

- The committed spec is current: a test that calls `check_file` on the
  document your plugin builds.
- The route answers: with `brocanth-test`'s `TestClient`, a `GET
  /openapi.json` returns 200 and `application/json`.

The panel's stylesheet `src/admin.css` is built from `style/`. See
`style/README.md`. You need it only when you change the panel's templates.

## More

- `docs/module-flags.md`: how a declared setting becomes a key, a variable
  and a flag.
- `docs/backoffice.md`: the API tab among the other admin panels.
- `docs/surface.md`: why there are no client generators, and the one
  exception (`brocanth-client`).
- The crate documentation below: the design argument and the quiet
  failures each test guards.
