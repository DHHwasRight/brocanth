# brocanth-test

An HTTP test client for a brocanth application. `TestClient` takes a built
`App` and sends it a request. The request goes through the same pipeline
the server runs: the request hooks, the router, the handler, the response
hooks. `TestResponse` gives the status, the headers and an `Html` reader.
The reader answers questions about the page: "is there a link to
`/admin/post`", "does this form post to `/admin/user/3/delete`".

This is the testing guide for brocanth. The README of each other crate
points here for "how an application tests code that uses this crate".

Use it to test a screen, a route, a redirect, a 404 or a 405, and a page
behind a policy. Do not use it to test sockets, framing or
`content-length`: it opens no socket. Those are `brocanth-server` tests.

## The rule: assert on behaviour, not on CSS classes

Assert on what a user can do with the page: follow a link, submit a form,
fill a field, press a button, read a text. Do not assert on a class name,
and do not search the body for a substring.

- `body.contains("/admin/post")` passes when the path is in a comment, a
  `data-` attribute or a `<style>` block. `html.assert_link_to("/admin/post")`
  passes only when a link goes there.
- A class name changes when the design changes. The behaviour does not.
- Scope an assertion to its region with `within`: the link in
  `<nav aria-label="Sections">` answers "which section am I in"; a
  pagination link does not.

**Reason:** `docs/framework-gaps.md` §G6. Three rounds of tests in one
afternoon asserted on CSS class names, because no test level existed for
behaviour. This crate is that level.

## Getting started

```toml
[dev-dependencies]
brocanth-test = { git = "https://github.com/DHHwasRight/brocanth.git" }
```

Inside this workspace: `brocanth-test.workspace = true`.

The crate has no features and no `scaffold` module. `brocanth create` does
not add it. Add it by hand as a dev-dependency.

It takes two dependencies: `brocanth` and `http`. It takes no async runtime
and no HTML parser (see "Why no HTML parser" below).

## Examples

### Test the routes `main` serves

`TestClient::serving` takes a `Router` that the application built. This is
how a generated application tests its own pages.

```rust
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_body::Body;
use brocanth::plugin_context::Base;
use brocanth::plugin_routes::{Handler, RouteCx};
use brocanth::routing::{Router, Verb};
use brocanth_test::TestClient;

fn page() -> Handler<Base> {
    Box::new(|cx: &mut RouteCx<'_, Base>| {
        let slug = cx.params().get("slug").unwrap_or("none").to_string();
        let html = format!(r#"<h1>{slug}</h1><a href="/pages/{slug}/edit">Edit</a>"#);
        Ok(http::Response::builder()
            .header("content-type", "text/html; charset=utf-8")
            .body(Body::from(html.into_bytes()))
            .unwrap())
    })
}

let app = AppBuilder::new().build().expect("an application");
let mut routes = Router::builder();
routes.route("/pages/{slug}", Verb::Get, page()).unwrap();
let client = TestClient::serving(app, routes.build().unwrap());

let response = client.get("/pages/about");
response.assert_ok();
response.html().assert_text("about").assert_link_to("/pages/about/edit");

// The router is under test too: a path with no route is a 404,
// and a known path with an unknown verb is a 405.
client.get("/pages").assert_status(404);
```

`TestClient::new(app)` serves every route the application mounted from a
plugin (`App::route_plan`), at the prefix the application chose. Use it to
test a plugin's screens, such as the admin's.

### Ask the page, not the bytes

`Html` is a scanner. It finds tags, attributes, inner text, and a region
scoped by an attribute. Each assertion panics with a list of what is there,
so a failure says what to fix.

```rust
use brocanth_test::Html;

let page = Html::new(
    r#"<nav aria-label="Sections"><a href="/admin/post">Posts</a></nav>
       <form method="post" action="/admin/post/3/delete">
         <input name="reason" value="spam">
         <button>Delete</button>
       </form>"#,
);

page.within("nav", "aria-label", "Sections")
    .assert_link_to("/admin/post");
page.assert_form_posts_to("/admin/post/3/delete")
    .assert_field("reason")
    .assert_button("Delete")
    .assert_no_button("Publish");

let form = page.form_posting_to("/admin/post/3/delete").unwrap();
assert_eq!(form.attr("method"), Some("post"));
assert_eq!(page.field("reason").unwrap().attr("value"), Some("spam"));
```

Attribute values and text come back decoded. Write the URL a browser
follows (`"?sort=views&dir=asc"`), not the markup (`&amp;`).

### Who is asking: `seeding`

A screen behind a policy needs an identity. `seeding` fills the
`RequestState` of each request before the request hooks run, where an
authentication hook writes. It is Django's `force_login`. The hooks, the
policy and the handler still run.

```rust
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_body::Body;
use brocanth::plugin_context::Base;
use brocanth::plugin_routes::{Handler, RouteCx};
use brocanth::routing::{Router, Verb};
use brocanth_test::TestClient;

struct CurrentUser {
    staff: bool,
}

fn dashboard() -> Handler<Base> {
    Box::new(|cx: &mut RouteCx<'_, Base>| {
        let staff = cx.state().get::<CurrentUser>().is_some_and(|u| u.staff);
        let status = if staff { 200 } else { 403 };
        Ok(http::Response::builder()
            .status(status)
            .body(Body::from(b"<p>Dashboard</p>".to_vec()))
            .unwrap())
    })
}

let routes = || {
    let mut b = Router::builder();
    b.route("/dashboard", Verb::Get, dashboard()).unwrap();
    b.build().unwrap()
};

let anonymous = TestClient::serving(AppBuilder::new().build().unwrap(), routes());
anonymous.get("/dashboard").assert_status(403);

let staff = TestClient::serving(AppBuilder::new().build().unwrap(), routes())
    .seeding(|state| {
        state.insert(CurrentUser { staff: true });
    });
staff.get("/dashboard").assert_ok().html().assert_text("Dashboard");
```

### Forms, redirects and other verbs

```rust,no_run
# use brocanth::plugin_app::AppBuilder;
# use brocanth::plugin_routes::Handler;
# use brocanth::plugin_context::Base;
# use brocanth::routing::Router;
use brocanth_test::{TestClient, TestRequest};
# let app = AppBuilder::new().build().unwrap();
# let router: Router<Handler<Base>> = Router::builder().build().unwrap();
# let client = TestClient::serving(app, router);

// A form post, as a browser sends it.
client
    .post("/posts", &[("title", "Hello")])
    .assert_redirects("/posts/1");

// Follow the redirect and keep the chain.
let landed = client.send(
    TestRequest::post("/posts").form(&[("title", "Hello")]).following(),
);
landed.assert_ok();
assert_eq!(landed.redirect_chain(), ["/posts/1"]);

// Any verb, header or body.
client
    .send(TestRequest::delete("/posts/1").header("accept", "text/html"))
    .assert_status(303);
```

`no_run`: the routes are not written out here. The first example shows the
setup.

## Sync or async

`client.get` and `client.send` run on the test thread. A route that waits
is driven by `brocanth_test::rt::block_on`, about thirty lines of `std`.
That is correct for a future that does not wait on a reactor: an in-memory
fixture, a formatter.

A route that waits on a Tokio timer or socket needs a runtime. Use
`get_async`, `post_async` or `send_async` inside `#[tokio::test]`. An
application with an `AsyncRequestHook` must use `send_async`: `send`
calls `App::run`, and the core refuses that at compile time.

## Testing this crate

```sh
cargo test -p brocanth-test
```

No environment variable is needed. `tests/pipeline.rs` checks the pipeline:
hooks, router, 404, 405, redirects, sync and async routes.
`tests/panels.rs` drives the real admin panels.

## Why no HTML parser

Every assertion these tests need is "find the tags called `a`, read their
`href`". That is a scanner, not a parser. A spec parser repairs the tree: it
adds the `<tbody>` the framework did not write and closes the `<a>` a
template left open. A test then passes on markup that a browser shows in a
different way. The scanner reads what was written. An element whose end
tag is missing has no inner text here. Comments, `<script>` and `<style>`
are skipped.

## Links

- `docs/framework-gaps.md` §G6: the gap this crate closes.
- `docs/llm-authoring.md`: the contract for writing brocanth with a model.
- The crate docs (`cargo doc -p brocanth-test --open`): the Django mapping
  and the reason for each design choice.
