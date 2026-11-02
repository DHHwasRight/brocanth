# brocanth-pages

The page router of a brocanth project: two proc macros that read `src/pages`
while the crate compiles. `pages!` turns the directory tree into the route
table. A route is a file, and the verb is the handler's name. `html!` wraps a
page in the layouts above it in the tree, and renders only the layers the
client does not hold yet. Use it in every generated project; `brocanth create`
always adds it. A proc macro puts no bytes in the binary. Do not use it for a
route table you want on one screen, or for an API with no pages: that is
`brocanth-routes`. Both fill one `RouterBuilder`. The macros do not render a
template. A renderer (`rsx` for HTML, `jt` for JSON) compiles the template
files, and the project's own `render` functions turn them into a response.

## Getting started

```toml
[dependencies]
brocanth = { git = "https://github.com/DHHwasRight/brocanth.git" }
brocanth-pages = { git = "https://github.com/DHHwasRight/brocanth.git" }
```

The crate has no features. It is not a module that you choose: every
generated project gets it. The renderers are modules:

```sh
brocanth create blog --with rsx      # .rsx templates and layouts, for HTML
brocanth create api --with jt        # .jt templates, for JSON
brocanth add rsx-htmx                # partial navigation over the layouts
```

None of these declares a config setting, so none has a `--<module>.<key>`
flag or a `$<APP>_<MODULE>__<KEY>` variable.

## The page router

`src/pages/mod.rs` holds one line that matters:

```rust,ignore
brocanth_pages::pages!("src/pages");
```

It emits a `pub mod` per file, a `router()`, a `routes` module of path
helpers, and a `handlers()` table. The path on disk is the URL:

| File | Route |
|---|---|
| `src/pages/index.rs` | `/` |
| `src/pages/about.rs` | `/about` |
| `src/pages/users/[id].rs` | `/users/{id}` |
| `src/pages/docs/[...rest].rs` | `/docs/{*rest}` |

The verb comes off the function name: `handle` is GET, `handle_post` is
POST, and so on for `head`, `put`, `patch`, `delete`, `options`. A
`pub fn handle_<x>` that is not a verb is a compile error, and so is a page
with no handler.

Two words after the path turn on more output:

- `describe`: a `described()` table of what each route takes and gives,
  for `brocanth-openapi` and `brocanth-client`. It names `brocanth_input` and
  `zod`.
- `mounts`: `router()` also calls `crate::pages::mounted(&mut routes)`, so
  routes from a module or from a `routes!` file go into the same table.

`pages!` and `html!` name these items of the project, which a generated
`src/pages/mod.rs` defines: `Handler`, `page`, `page_with_body` (for a
handler that takes a body), `render`, `render_with`, `render_oob`, `prefix`,
and `Ctx::partial_depth`.

## Layouts

A layout is a file called `layout.rsx`. It wraps every page in its directory
and below, and layouts stack from the root down. A page never names its
layout:

```rust,ignore
// src/pages/users/[id].rs
use crate::pages::{html, views, Ctx, Error, IntoResponse};

pub fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let id: u64 = ctx.require("id")?.parse()?;
    html!(ctx, views::users::_id_::User { id })
}
```

With `src/pages/layout.rsx` and `src/pages/users/layout.rsx`, this renders the
root layout around the `users` layout around the page. Delete
`users/layout.rsx` and the same line renders one layer.

`html!` fills `children`. It also fills `layout` (the URL prefix the layout
covers, such as `/users`) and `path` (the request path) when the layout
declares them, and nothing else. A layout that defines an `Oob` component
sends it out of band in a partial, to update its own chrome. When the client
holds `n` layers (`ctx.partial_depth()`), `html!` leaves out the outer `n`.

Only `.rsx` owns layouts (`LAYOUT_EXTENSIONS`). Two layouts in one directory
are a compile error. After you create a new page or layout, touch any source
file: cargo cannot see a file that nothing references yet.

## Renderers

A renderer compiles template files beside the pages into a module of
`src/pages/mod.rs`. `pages!` reads only `.rs` files, so a template beside a
page needs nothing from this crate.

| Renderer | Module | In `src/pages/mod.rs` | A page calls |
|---|---|---|---|
| rsx (HTML) | `rsx` | `pub mod views { rsx::load_dir!("src/pages"); }` | `html!(ctx, views::about::About { .. })` |
| jt (JSON) | `jt` | `pub mod payload { jt::load_dir!("src/pages"); }` | `render_json(payload::status::Status(..))` |

Both can live in one project: two modules over one tree. `jt` has no layouts,
because JSON has no document to wrap. Both renderers and this crate turn
`[id]` into `_id_` by the same rule, so a page and its template share a
module path.

## Examples

The examples above are `ignore` in this file: the macros read a directory
while the crate compiles, and a doctest has no page tree beside it.
`tests/readme.rs` runs them against a tree laid out as a project lays out
`src/pages` (`tests/readme/src/pages`). It shows:

- the route table from the files, and the verbs from the function names;
- `routes::root::path()` returning `/` and `routes::users::id::path(7)` returning `/users/7`;
- two layouts stacking around `/users/7`;
- a partial at depth 1 and 2 leaving out the outer layouts;
- `handle_post` in the same file as `handle`.

In that test, `views` is written by hand where a project has
`rsx::load_dir!`, so the test needs no template engine.

## Testing

```sh
cargo test -p brocanth-pages
```

No environment variable is needed. The unit tests write page trees under the
system temp directory and expand the macros against them.

A generated project tests its own table: `src/pages/mod.rs` asserts the
whole set of routes. To test a page, use `brocanth-test`:
`TestClient::serving(app, router)` sends a request by path, and
`TestResponse::html()` asserts on elements, links and forms. To test a
partial, send the `DP-Partial` header with `TestClient::header`.

## Read more

- `docs/modules.md` §4.1 and §5: the renderer seam, `Slot::PagesRenderers`,
  and how `jt` attached without a change here.
- `docs/static-export.md`: the partials per depth, written to files.
- `docs/surface.md`: what `describe` feeds.
- `brocanth-routes`: the declared half of the router.
