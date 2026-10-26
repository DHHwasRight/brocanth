//! `routes!` against a real router.
//!
//! The macro emits the contract a generated project defines: a `Handler`, a
//! `page` that wraps a function into one, and a `mount` that fills a
//! [`RouterBuilder`]. None of that comes from a generated project here --
//! this file defines the same three shapes and builds real tables with them,
//! so the test tells the truth about what the macro produces without a
//! scaffold anywhere near it.

use brocanth::routing::{BuildError, Dispatch, Router, RouterBuilder, Verb};

// ---------------------------------------------------------------------------
// The contract, as `src/pages/mod.rs` writes it in a generated project.
// ---------------------------------------------------------------------------

/// Stands in for the request context. It carries what each test asserts on.
pub struct Ctx {
    pub id: String,
}

/// Stands in for the response. The body is enough to say which handler ran.
#[derive(Debug, PartialEq, Eq)]
pub struct Response(pub String);

/// Stands in for the project's error type.
#[derive(Debug)]
pub struct Error;

/// Stands in for the trait a handler's return type satisfies.
pub trait IntoResponse {
    fn into_response(self) -> Response;
}

impl IntoResponse for Response {
    fn into_response(self) -> Response {
        self
    }
}

impl IntoResponse for &'static str {
    fn into_response(self) -> Response {
        Response(self.to_owned())
    }
}

impl IntoResponse for String {
    fn into_response(self) -> Response {
        Response(self)
    }
}

/// Exactly the alias the contract names.
pub type Handler = Box<dyn Fn(&mut Ctx) -> Result<Response, Error> + Send + Sync>;

/// Exactly the function the contract names.
pub fn page<H, T>(handler: H) -> Handler
where
    H: Fn(&mut Ctx) -> Result<T, Error> + Send + Sync + 'static,
    T: IntoResponse,
{
    Box::new(move |ctx| handler(ctx).map(IntoResponse::into_response))
}

/// A project's own wrapper, to prove the escape hatch works: anything that is
/// not a bare path is taken as already being a `Handler`.
pub fn page_twice<H, T>(handler: H) -> Handler
where
    H: Fn(&mut Ctx) -> Result<T, Error> + Send + Sync + 'static,
    T: IntoResponse,
{
    Box::new(move |ctx| {
        let first = handler(ctx)?.into_response();
        Ok(Response(format!("{} {}", first.0, first.0)))
    })
}

// ---------------------------------------------------------------------------
// What a project writes: handlers, and controllers with the seven functions.
// ---------------------------------------------------------------------------

macro_rules! handler {
    ($name:ident) => {
        pub fn $name(_ctx: &mut crate::Ctx) -> Result<&'static str, crate::Error> {
            Ok(stringify!($name))
        }
    };
}

pub mod controllers {
    pub mod posts {
        handler!(index);
        handler!(new);
        handler!(create);
        handler!(edit);
        handler!(update);
        handler!(destroy);

        /// Echoes the captured id, so a test can prove the parameter arrived.
        pub fn show(ctx: &mut crate::Ctx) -> Result<String, crate::Error> {
            Ok(format!("post {}", ctx.id))
        }

        handler!(preview);
        handler!(search);
    }

    pub mod comments {
        handler!(index);
        handler!(new);
        handler!(create);
        handler!(show);
        handler!(edit);
        handler!(update);
        handler!(destroy);
    }

    pub mod tags {
        handler!(index);
        handler!(show);
    }

    pub mod profile {
        handler!(new);
        handler!(create);
        handler!(show);
        handler!(edit);
        handler!(update);
        handler!(destroy);
    }
}

pub mod pages {
    handler!(index);
    handler!(about);
    handler!(signup);
    handler!(dashboard);
}

// ---------------------------------------------------------------------------
// The tables.
// ---------------------------------------------------------------------------

/// The example from the crate documentation, as a project would write it.
mod site {
    use super::{page, page_twice, pages, Handler};

    brocanth_routes::routes! {
        root           => pages::index;
        get  "/about"  => pages::about;
        post "/signup" => page_twice(pages::signup);

        resources "/posts" => crate::controllers::posts {
            member     { get "/preview" => crate::controllers::posts::preview; }
            collection { get "/search"  => crate::controllers::posts::search; }
            resources "/comments" => crate::controllers::comments;
        }

        resources "/tags" => crate::controllers::tags, only: [index, show];
        resource  "/profile" => crate::controllers::profile;

        scope "/admin" {
            get "/" => pages::dashboard;
        }
    }
}

/// A second source, to prove that two tables share one builder and that the
/// second claim on a path is reported rather than applied.
mod other {
    use super::{page, pages, Handler};

    brocanth_routes::routes! {
        get "/about" => pages::about;
    }
}

/// Shallow nesting, which changes where a nested member route answers.
mod shallow {
    use super::{page, Handler};

    brocanth_routes::routes! {
        resources "/posts" => crate::controllers::posts, shallow: true {
            resources "/comments" => crate::controllers::comments;
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers for the assertions.
// ---------------------------------------------------------------------------

fn router_of(
    mount: fn(&mut RouterBuilder<Handler>, &str) -> Result<(), BuildError>,
    prefix: &str,
) -> Router<Handler> {
    let mut builder = RouterBuilder::new();
    mount(&mut builder, prefix).expect("the table builds");
    builder.build().expect("the table is a router")
}

/// Run whatever answers `verb path`, and give back the body.
fn call(router: &Router<Handler>, verb: Verb, path: &str) -> String {
    match router.dispatch(verb, path) {
        Dispatch::Found { handler, matched } => {
            let mut ctx = Ctx {
                id: matched.params().get("id").unwrap_or_default().to_owned(),
            };
            handler(&mut ctx).expect("the handler answers").0
        }
        Dispatch::MethodNotAllowed { allow, .. } => {
            panic!("{verb} {path} is not allowed; the route answers {allow}")
        }
        Dispatch::NotFound => panic!("nothing matches {verb} {path}"),
    }
}

fn allow(router: &Router<Handler>, path: &str) -> String {
    router
        .allow_for(path)
        .unwrap_or_else(|| panic!("no route matches {path}"))
        .to_string()
}

// ---------------------------------------------------------------------------
// The tests.
// ---------------------------------------------------------------------------

#[test]
fn a_resource_is_seven_actions_on_four_paths() {
    let router = router_of(site::mount, "");
    assert_eq!(call(&router, Verb::Get, "/posts"), "index");
    assert_eq!(call(&router, Verb::Post, "/posts"), "create");
    assert_eq!(call(&router, Verb::Get, "/posts/new"), "new");
    assert_eq!(call(&router, Verb::Get, "/posts/42"), "post 42");
    assert_eq!(call(&router, Verb::Get, "/posts/42/edit"), "edit");
    assert_eq!(call(&router, Verb::Patch, "/posts/42"), "update");
    assert_eq!(call(&router, Verb::Put, "/posts/42"), "update");
    assert_eq!(call(&router, Verb::Delete, "/posts/42"), "destroy");
}

#[test]
fn the_collection_and_the_member_each_answer_their_own_verbs() {
    // One path, one `Allow` set: what makes a 405 useful instead of a 404.
    let router = router_of(site::mount, "");
    assert_eq!(allow(&router, "/posts"), "GET, HEAD, POST");
    assert_eq!(allow(&router, "/posts/42"), "GET, HEAD, PUT, PATCH, DELETE");
    assert!(matches!(
        router.dispatch(Verb::Delete, "/posts"),
        Dispatch::MethodNotAllowed { .. }
    ));
}

#[test]
fn only_keeps_the_actions_it_names() {
    let router = router_of(site::mount, "");
    assert_eq!(call(&router, Verb::Get, "/tags"), "index");
    assert_eq!(call(&router, Verb::Get, "/tags/rust"), "show");
    assert!(matches!(
        router.dispatch(Verb::Delete, "/tags/rust"),
        Dispatch::MethodNotAllowed { .. }
    ));
    assert!(matches!(
        router.dispatch(Verb::Get, "/tags/new"),
        // `new` was not asked for, so `/tags/new` is the show route.
        Dispatch::Found { .. }
    ));
}

#[test]
fn a_singular_resource_has_no_index_and_no_parameter() {
    let router = router_of(site::mount, "");
    assert_eq!(call(&router, Verb::Get, "/profile"), "show");
    assert_eq!(call(&router, Verb::Get, "/profile/edit"), "edit");
    assert_eq!(
        allow(&router, "/profile"),
        "GET, HEAD, POST, PUT, PATCH, DELETE"
    );
    assert!(matches!(
        router.dispatch(Verb::Get, "/profile/1"),
        Dispatch::NotFound
    ));
}

#[test]
fn member_and_collection_hang_off_the_right_path() {
    let router = router_of(site::mount, "");
    assert_eq!(call(&router, Verb::Get, "/posts/42/preview"), "preview");
    assert_eq!(call(&router, Verb::Get, "/posts/search"), "search");
}

#[test]
fn a_nested_resource_keeps_the_parent_in_the_path() {
    // `{id}` and `{post_id}` sit in the same position in two templates. The
    // matcher takes them as two routes, which is what makes `/posts/42/edit`
    // and `/posts/42/comments` both reachable.
    let router = router_of(site::mount, "");
    assert_eq!(call(&router, Verb::Get, "/posts/42/comments"), "index");
    assert_eq!(call(&router, Verb::Get, "/posts/42/comments/7"), "show");
    assert_eq!(call(&router, Verb::Get, "/posts/42/edit"), "edit");
}

#[test]
fn shallow_moves_the_nested_member_routes_to_the_root() {
    let router = router_of(shallow::mount, "");
    assert_eq!(call(&router, Verb::Get, "/posts/42/comments"), "index");
    assert_eq!(call(&router, Verb::Post, "/posts/42/comments"), "create");
    assert_eq!(call(&router, Verb::Get, "/comments/7"), "show");
    assert_eq!(call(&router, Verb::Get, "/comments/7/edit"), "edit");
    assert!(matches!(
        router.dispatch(Verb::Get, "/posts/42/comments/7"),
        Dispatch::NotFound
    ));
}

#[test]
fn a_scope_moves_everything_under_it() {
    let router = router_of(site::mount, "");
    assert_eq!(call(&router, Verb::Get, "/admin"), "dashboard");
}

#[test]
fn root_is_the_slash() {
    let router = router_of(site::mount, "");
    assert_eq!(call(&router, Verb::Get, "/"), "index");
}

#[test]
fn a_handler_that_is_already_a_handler_is_not_wrapped_twice() {
    let router = router_of(site::mount, "");
    assert_eq!(call(&router, Verb::Post, "/signup"), "signup signup");
}

#[test]
fn a_prefix_moves_every_route_in_the_table() {
    let router = router_of(site::mount, "/api/v1");
    // The table's own root answers at the prefix itself, with no trailing
    // slash to strip later.
    assert_eq!(call(&router, Verb::Get, "/api/v1"), "index");
    assert_eq!(call(&router, Verb::Get, "/api/v1/posts/42"), "post 42");
    assert_eq!(call(&router, Verb::Get, "/api/v1/admin"), "dashboard");
    assert!(matches!(
        router.dispatch(Verb::Get, "/posts"),
        Dispatch::NotFound
    ));
}

#[test]
fn a_trailing_slash_on_the_prefix_does_not_double() {
    let router = router_of(site::mount, "/api/");
    assert_eq!(call(&router, Verb::Get, "/api/posts"), "index");
}

#[test]
fn two_sources_fill_one_table() {
    // The point of the contract: the page tree, this file, and whatever a
    // module mounts all end up in one builder.
    let mut builder = RouterBuilder::new();
    shallow::mount(&mut builder, "").expect("the first table builds");
    other::mount(&mut builder, "").expect("the second table builds");
    let router = builder.build().expect("the table is a router");
    assert_eq!(call(&router, Verb::Get, "/about"), "about");
    assert_eq!(call(&router, Verb::Get, "/posts"), "index");
}

#[test]
fn a_path_claimed_twice_is_reported_rather_than_replaced() {
    // `site` already answers `GET /about`. A second source claiming it is the
    // failure one shared table exists to catch: silently overwriting would
    // make the live handler depend on the order the sources were mounted.
    let mut builder = RouterBuilder::new();
    site::mount(&mut builder, "").expect("the first table builds");
    let error = other::mount(&mut builder, "").expect_err("the second claim is refused");
    assert_eq!(
        error,
        BuildError::DuplicateHandler {
            template: "/about".to_owned(),
            verb: Verb::Get,
        }
    );
}

#[test]
fn every_route_has_a_path_function_taking_its_parameters() {
    assert_eq!(site::routes::root::path(), "/");
    assert_eq!(site::routes::about::path(), "/about");
    assert_eq!(site::routes::posts::path(), "/posts");
    assert_eq!(site::routes::posts::new::path(), "/posts/new");
    assert_eq!(site::routes::posts::id::path(42), "/posts/42");
    assert_eq!(site::routes::posts::id::edit::path(42), "/posts/42/edit");
    assert_eq!(
        site::routes::posts::post_id::comments::path(42),
        "/posts/42/comments"
    );
    assert_eq!(
        site::routes::posts::post_id::comments::id::path(42, 7),
        "/posts/42/comments/7"
    );
    assert_eq!(site::routes::admin::path(), "/admin");
}

#[test]
fn a_path_function_encodes_what_it_is_given() {
    // The value cannot escape the segment the route gave it.
    assert_eq!(site::routes::posts::id::path("a/b"), "/posts/a%2Fb");
}

#[test]
fn a_helper_knows_the_verbs_its_route_answers() {
    assert_eq!(site::routes::posts::VERBS, &["GET", "POST"]);
    assert_eq!(
        site::routes::posts::id::VERBS,
        &["GET", "PUT", "PATCH", "DELETE"]
    );
    assert_eq!(site::routes::posts::id::TEMPLATE, "/posts/{id}");
}

#[test]
fn a_url_hangs_the_path_off_a_base() {
    // The paths are relative to the mount prefix, so a table mounted under
    // one passes it here.
    assert_eq!(
        site::routes::posts::id::url("https://example.com/api", 42),
        "https://example.com/api/posts/42"
    );
}
