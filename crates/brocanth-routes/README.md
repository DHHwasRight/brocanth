# brocanth-routes

`routes!`: the whole route table of a project, written in one file, in the
shape of Rails' `config/routes.rb`. The macro checks the table while the crate
compiles. A missing controller action is a compile error on the `resources`
line, and every path goes through the core's `RouteTemplate`. Use it for an
API that has no pages, or when a team wants every route on one screen. For an
HTML application, the first answer is a file per route under `src/pages`
(`brocanth-pages`). Both fill the same `RouterBuilder`, so a project can use
both, and a path that both declare is one `BuildError` at startup.

## Getting started

```toml
[dependencies]
brocanth = { git = "https://github.com/DHHwasRight/brocanth.git" }
brocanth-routes = { git = "https://github.com/DHHwasRight/brocanth.git" }
```

The crate has no features and is not a module: no `brocanth create --with`
key writes a routes file. You add it by hand (see "In a generated project"
below).

The macro emits two items where you call it:

- `pub fn mount(routes: &mut RouterBuilder<Handler>, prefix: &str) -> Result<(), BuildError>`;
- `pub mod routes`, with one `path()` helper per route that takes exactly
  that route's parameters.

`Handler` and `page` must be in scope where you call the macro. A generated
project defines both in `src/pages/mod.rs`. The macro names nothing else of
the project, so the project keeps its own `Ctx` and `Error`.

## The grammar

| Line | What it adds |
|---|---|
| `root => h;` | `GET /` |
| `get "/about" => h;` | `GET /about`. Also `head`, `post`, `put`, `patch`, `delete`, `options` |
| `resources "/posts" => ctrl;` | seven actions on four paths (below) |
| `resource "/profile" => ctrl;` | the same, without `index` and without `{id}` |
| `scope "/admin" { .. }` | a URL prefix, and helper modules to match |
| `namespace admin { .. }` | the same, written as one identifier |
| `member { .. }` / `collection { .. }` | inside a resource: under `/posts/{id}` or under `/posts` |

`resources "/posts" => ctrl` routes `ctrl::index` (GET `/posts`), `ctrl::new`
(GET `/posts/new`), `ctrl::create` (POST `/posts`), `ctrl::show` (GET
`/posts/{id}`), `ctrl::edit` (GET `/posts/{id}/edit`), `ctrl::update` (PATCH
and PUT `/posts/{id}`) and `ctrl::destroy` (DELETE `/posts/{id}`).

Options on a resource: `only: [..]`, `except: [..]`, `param: slug`,
`shallow: true`. On any route or group: `as: name`, the helper module name.

## Examples

### A table, mounted and dispatched

The hidden lines define the three shapes a generated project writes in
`src/pages/mod.rs`: `Ctx`, `Handler` and `page`.

```rust
# pub struct Ctx { pub id: String }
# pub struct Response(pub String);
# pub struct Error;
# pub trait IntoResponse { fn into_response(self) -> Response; }
# impl IntoResponse for &'static str { fn into_response(self) -> Response { Response(self.into()) } }
# impl IntoResponse for String { fn into_response(self) -> Response { Response(self) } }
# pub type Handler = Box<dyn Fn(&mut Ctx) -> Result<Response, Error> + Send + Sync>;
# pub fn page<H, T>(handler: H) -> Handler
# where
#     H: Fn(&mut Ctx) -> Result<T, Error> + Send + Sync + 'static,
#     T: IntoResponse,
# {
#     Box::new(move |ctx| handler(ctx).map(IntoResponse::into_response))
# }
use brocanth::routing::{Dispatch, RouterBuilder, Verb};

pub mod pages {
    pub fn home(_ctx: &mut crate::Ctx) -> Result<&'static str, crate::Error> {
        Ok("home")
    }
}

pub mod posts {
    use crate::{Ctx, Error};

    pub fn index(_ctx: &mut Ctx) -> Result<&'static str, Error> {
        Ok("all posts")
    }
    pub fn show(ctx: &mut Ctx) -> Result<String, Error> {
        Ok(format!("post {}", ctx.id))
    }
}

mod table {
    use super::{page, Handler};

    brocanth_routes::routes! {
        root => crate::pages::home;
        resources "/posts" => crate::posts, only: [index, show];
    }
}

# fn main() {
let mut builder = RouterBuilder::new();
table::mount(&mut builder, "").expect("the table mounts");
let router = builder.build().expect("the table builds");

// The helpers take exactly the route's parameters.
assert_eq!(table::routes::posts::id::path(42), "/posts/42");

match router.dispatch(Verb::Get, "/posts/42") {
    Dispatch::Found { handler, matched } => {
        let mut ctx = Ctx { id: matched.params().get("id").unwrap().to_owned() };
        assert_eq!(handler(&mut ctx).ok().unwrap().0, "post 42");
    }
    _ => panic!("GET /posts/42 is routed"),
}

// `only:` dropped `destroy`, so DELETE is a 405, not a 404.
assert!(matches!(
    router.dispatch(Verb::Delete, "/posts/42"),
    Dispatch::MethodNotAllowed { .. }
));
# }
```

### A prefix, and a path that two sources claim

The application chooses the prefix when it calls `mount`. Two sources that
claim one path fail the build of the table, not the request.

```rust
# pub struct Ctx;
# pub struct Response;
# pub struct Error;
# pub type Handler = Box<dyn Fn(&mut Ctx) -> Result<Response, Error> + Send + Sync>;
# pub fn page<H>(handler: H) -> Handler
# where
#     H: Fn(&mut Ctx) -> Result<Response, Error> + Send + Sync + 'static,
# {
#     Box::new(handler)
# }
use brocanth::routing::{RouterBuilder, Verb};

pub fn dashboard(_ctx: &mut Ctx) -> Result<Response, Error> {
    Ok(Response)
}

mod admin {
    use super::{page, Handler};

    brocanth_routes::routes! {
        get "/" => crate::dashboard;
    }
}

# fn main() {
let mut builder = RouterBuilder::new();
admin::mount(&mut builder, "/admin").expect("the table mounts");
// The same table again, at the same prefix: one path, two claims.
admin::mount(&mut builder, "/admin").expect_err("a second claim on /admin is refused");
# let _ = Verb::Get;
# }
```

### In a generated project

Write `src/routes.rs` with the macro. Then ask `pages!` for the `mounts`
word, and call the routes file from the `mounted` function in
`src/pages/mod.rs`:

```rust,ignore
// src/pages/mod.rs
brocanth_pages::pages!("src/pages", mounts);

fn mounted(
    routes: &mut ::brocanth::routing::RouterBuilder<Handler>,
) -> Result<(), ::brocanth::routing::BuildError> {
    crate::routes::mount(routes, "")?;
    Ok(())
}
```

This block is `ignore`: it needs a generated project around it
(`src/pages`, `Ctx`, `Handler`).

## Testing

```sh
cargo test -p brocanth-routes
```

No environment variable is needed. `tests/mount.rs` builds real tables with
the macro and checks dispatch, `Allow` sets, prefixes, helpers and conflicts.
The README examples run as doctests of this crate.

To test an application that uses `routes!`, use `brocanth-test`:
`TestClient::serving(app, router)` sends a request by path, so the match, the
404 and the 405 are under test.

## Read more

- The crate docs: what a group nests, why a controller is a module, and
  what the macro does not do (segment constraints, subdomains).
- `docs/modules.md` §4.6: route bundles, and the one table that pages,
  modules and a routes file fill.
- `brocanth-pages`: the file-based half of the router.
