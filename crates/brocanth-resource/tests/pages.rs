//! The page-shaped surface, against the contract a generated project offers.
//!
//! No generated project is needed, and that is deliberate: the contract is
//! four items, so this file writes those four items and asserts against them.
//! A test that needed `brocanth create` to run first would test the generator
//! and not this crate.
//!
//! ```text
//! pub type Handler = Box<dyn Fn(&mut Ctx) -> Result<Response, Error> + Send + Sync>;
//! pub fn page<H, T>(handler: H) -> Handler
//! where H: Fn(&mut Ctx) -> Result<T, Error> + Send + Sync + 'static, T: IntoResponse;
//! ```
//!
//! The `Ctx`, `Response` and `Error` below are the generated ones, cut down to
//! what a records route reads and writes. The field names and the method names
//! are the generated ones too, so a change on either side shows up here.

#![cfg(feature = "pages")]

mod common;

use std::sync::Arc;

use brocanth::routing::{BuildError, Dispatch, Router, RouterBuilder, Verb};
use brocanth_resource::pages::{self, PageCtx, PageError, PageResponse, PageSubject};
use brocanth_resource::Failure;
use common::{seed_eight, Posts, Visitor};

// ---------------------------------------------------------------------------
// The contract, as `src/pages/mod.rs` writes it
// ---------------------------------------------------------------------------

/// A page's answer: a status, a body and the headers that body needs.
#[derive(Debug, Default)]
struct Response {
    status: u16,
    body: String,
    headers: Vec<(String, String)>,
}

impl Response {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(held, _)| held.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// What a handler may fail with.
#[derive(Debug)]
struct Error {
    status: u16,
    #[allow(dead_code)]
    message: String,
    #[allow(dead_code)]
    headers: Vec<(String, String)>,
}

/// What a handler may return instead of building a [`Response`] by hand.
trait IntoResponse {
    fn into_response(self) -> Response;
}

impl IntoResponse for Response {
    fn into_response(self) -> Response {
        self
    }
}

impl IntoResponse for &str {
    fn into_response(self) -> Response {
        Response {
            status: 200,
            body: self.to_owned(),
            ..Response::default()
        }
    }
}

/// What a handler is given.
struct Ctx {
    #[allow(dead_code)]
    verb: Verb,
    #[allow(dead_code)]
    path: String,
    query: String,
    body: Vec<u8>,
    params: Vec<(String, String)>,
    headers: Vec<(String, String)>,
}

impl Ctx {
    fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

type Handler = Box<dyn Fn(&mut Ctx) -> Result<Response, Error> + Send + Sync>;

/// Puts a handler in the table whatever it returns.
fn page<H, T>(handler: H) -> Handler
where
    H: Fn(&mut Ctx) -> Result<T, Error> + Send + Sync + 'static,
    T: IntoResponse,
{
    Box::new(move |ctx| handler(ctx).map(IntoResponse::into_response))
}

// ---------------------------------------------------------------------------
// The four lines a generated project writes
// ---------------------------------------------------------------------------

impl PageCtx for Ctx {
    fn query(&self) -> &str {
        &self.query
    }

    fn body(&self) -> &[u8] {
        &self.body
    }

    fn param(&self, name: &str) -> Option<&str> {
        Ctx::param(self, name)
    }

    fn header(&self, name: &str) -> Option<&str> {
        Ctx::header(self, name)
    }
}

impl PageResponse for Response {
    fn page_response(status: u16, body: String, headers: Vec<(String, String)>) -> Self {
        Response {
            status,
            body,
            headers,
        }
    }
}

impl PageError for Error {
    fn page_error(status: u16, message: String) -> Self {
        Error {
            status,
            message,
            headers: Vec::new(),
        }
    }
}

/// The subject comes out of a header here, as it does for the other surface.
/// In a project it comes out of whatever that project authenticates with,
/// which this crate never learns.
impl PageSubject<Ctx> for Posts {
    fn subject(&self, ctx: &Ctx) -> Result<Visitor, Failure> {
        match PageCtx::header(ctx, "x-user") {
            None => Ok(Visitor::anonymous()),
            Some("admin") => Ok(Visitor::admin()),
            Some("banned") => Ok(Visitor::banned()),
            Some(id) => id
                .parse()
                .map(Visitor::user)
                .map_err(|_| Failure::NoSubject),
        }
    }
}

// ---------------------------------------------------------------------------
// Driving the table
// ---------------------------------------------------------------------------

const PREFIX: &str = "/api/collections/posts/records";

fn mounted(posts: Posts) -> Router<Handler> {
    let mut routes = RouterBuilder::new();
    pages::mount(&mut routes, PREFIX, Arc::new(posts)).expect("the routes mount");
    routes.build().expect("the table builds")
}

/// One request, through the table a project would dispatch.
fn call(
    router: &Router<Handler>,
    verb: Verb,
    target: &str,
    who: Option<&str>,
    body: &str,
) -> Result<Response, Error> {
    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path, query),
        None => (target, ""),
    };
    let Dispatch::Found { handler, matched } = router.dispatch(verb, path) else {
        panic!("no route answers {} {path}", verb.as_str());
    };
    let params = matched
        .params()
        .iter()
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect();
    let mut ctx = Ctx {
        verb,
        path: path.to_owned(),
        query: query.to_owned(),
        body: body.as_bytes().to_vec(),
        params,
        headers: who
            .map(|who| vec![("x-user".to_owned(), who.to_owned())])
            .unwrap_or_default(),
    };
    handler(&mut ctx)
}

fn answered(
    router: &Router<Handler>,
    verb: Verb,
    target: &str,
    who: Option<&str>,
    body: &str,
) -> Response {
    call(router, verb, target, who, body).expect("the handler answers rather than failing")
}

fn at(suffix: &str) -> String {
    format!("{PREFIX}{suffix}")
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// Five REST routes, and the two a browser form can reach.
#[test]
fn seven_routes_appear_under_the_prefix_the_project_chose() {
    let router = mounted(Posts::new());

    let offered = [
        (Verb::Get, at("")),
        (Verb::Post, at("")),
        (Verb::Get, at("/1")),
        (Verb::Patch, at("/1")),
        (Verb::Delete, at("/1")),
        // An HTML form sends GET or POST and nothing else.
        (Verb::Post, at("/1")),
        (Verb::Post, at("/1/delete")),
    ];
    for (verb, path) in &offered {
        assert!(
            router.dispatch(*verb, path).is_found(),
            "{} {path} is not in the table",
            verb.as_str()
        );
    }

    // The templates, rather than the paths one request matched.
    let templates: Vec<&str> = router
        .routes()
        .iter()
        .map(|(template, _)| template.pattern())
        .collect();
    assert_eq!(
        templates,
        vec![at(""), at("/{id}"), at("/{id}/delete")],
        "three paths carry the seven routes"
    );
}

/// A verb the resource does not answer is a 405 and never a 404: the path is
/// there, and hiding that hides the only fact that makes the mistake findable.
#[test]
fn a_verb_the_resource_does_not_offer_is_a_405() {
    let router = mounted(Posts::new());

    assert!(matches!(
        router.dispatch(Verb::Put, &at("/1")),
        Dispatch::MethodNotAllowed { .. }
    ));
    assert!(matches!(
        router.dispatch(Verb::Delete, &at("")),
        Dispatch::MethodNotAllowed { .. }
    ));
}

/// The five, for a project that writes its own forms.
#[test]
fn mount_api_offers_the_five_rest_routes_and_no_form_route() {
    let mut routes: RouterBuilder<Handler> = RouterBuilder::new();
    pages::mount_api(&mut routes, PREFIX, Arc::new(Posts::new())).expect("the routes mount");
    let router = routes.build().expect("the table builds");

    for (verb, path) in [
        (Verb::Get, at("")),
        (Verb::Post, at("")),
        (Verb::Get, at("/1")),
        (Verb::Patch, at("/1")),
        (Verb::Delete, at("/1")),
    ] {
        assert!(router.dispatch(verb, &path).is_found());
    }
    // The path is there and the verb is not, so a form POST is a 405; the
    // delete form's path is not there at all. Both are the difference between
    // the two mounts.
    assert!(matches!(
        router.dispatch(Verb::Post, &at("/1")),
        Dispatch::MethodNotAllowed { .. }
    ));
    assert!(matches!(
        router.dispatch(Verb::Post, &at("/1/delete")),
        Dispatch::NotFound
    ));
    assert_eq!(router.len(), 2, "two paths carry the five routes");
}

// ---------------------------------------------------------------------------
// One table, so a path declared twice is a build error
// ---------------------------------------------------------------------------

/// The point of mounting into the project's own builder: a page and a resource
/// that both claim a path is a build error, not a silent winner.
#[test]
fn a_page_and_a_resource_cannot_both_claim_a_path() {
    let mut routes: RouterBuilder<Handler> = RouterBuilder::new();
    routes
        .route(&at(""), Verb::Get, page(|_: &mut Ctx| Ok("a page")))
        .expect("the page registers");

    let refused = pages::mount(&mut routes, PREFIX, Arc::new(Posts::new()))
        .expect_err("the resource claims a path the page already answers");

    match refused {
        BuildError::DuplicateHandler { template, verb } => {
            assert_eq!(template, at(""));
            assert_eq!(verb, Verb::Get);
        }
        other => panic!("expected a duplicate handler, got {other:?}"),
    }
}

/// The same resource, mounted twice at one prefix.
#[test]
fn one_prefix_takes_one_resource() {
    let mut routes: RouterBuilder<Handler> = RouterBuilder::new();
    pages::mount(&mut routes, PREFIX, Arc::new(Posts::new())).expect("the first mount");

    let refused = pages::mount(&mut routes, PREFIX, Arc::new(Posts::new()))
        .expect_err("the second mount claims every path the first one took");

    assert!(matches!(refused, BuildError::DuplicateHandler { .. }));
}

/// Two resources under two prefixes share a table and do not collide.
#[test]
fn two_prefixes_take_two_resources() {
    let mut routes: RouterBuilder<Handler> = RouterBuilder::new();
    pages::mount(&mut routes, "/api/posts", Arc::new(Posts::new())).expect("the first mount");
    pages::mount(&mut routes, "/api/drafts", Arc::new(Posts::new())).expect("the second mount");

    let router = routes.build().expect("the table builds");
    assert!(router.dispatch(Verb::Get, "/api/posts/1").is_found());
    assert!(router.dispatch(Verb::Get, "/api/drafts/1").is_found());
}

// ---------------------------------------------------------------------------
// A policy refusal reaches the response
// ---------------------------------------------------------------------------

/// `Pred::Never` is a fact about the collection, so it is said: 403.
#[test]
fn a_null_rule_is_a_403_on_the_page_surface_too() {
    let posts = Posts::new();
    seed_eight(&posts);
    let router = mounted(posts);

    let refused = answered(&router, Verb::Get, &at(""), Some("banned"), "");

    assert_eq!(refused.status, 403);
    assert!(refused.body.contains("forbidden"), "{}", refused.body);
    assert_eq!(refused.header("content-type"), Some("application/json"));
}

/// An unsatisfied list rule is an empty page, because a list rule is a filter.
/// A 403 here would be an oracle for "something matches".
#[test]
fn an_unsatisfied_list_rule_is_an_empty_page() {
    let posts = Posts::new();
    posts.db.seed(&[(1, Some(7), "draft", "hidden")]);
    let router = mounted(posts);

    let answer = answered(&router, Verb::Get, &at(""), None, "");

    assert_eq!(answer.status, 200);
    assert!(answer.body.contains("\"items\":[]"), "{}", answer.body);
}

/// A row you may not see and a row that is not there are one answer, byte for
/// byte.
#[test]
fn a_record_you_may_not_see_is_a_404_and_never_a_403() {
    let posts = Posts::new();
    seed_eight(&posts);
    let router = mounted(posts);

    let hidden = answered(&router, Verb::Get, &at("/2"), None, "");
    let absent = answered(&router, Verb::Get, &at("/99"), None, "");

    assert_eq!(hidden.status, 404);
    assert_eq!(absent.status, 404);
    assert_eq!(hidden.body, absent.body, "byte for byte");
}

/// The field mask is checked before the row is touched, which is what makes
/// 403 safe here.
#[test]
fn a_masked_column_is_a_403_that_says_which_one() {
    let posts = Posts::new();
    seed_eight(&posts);
    let router = mounted(posts);

    let refused = answered(&router, Verb::Patch, &at("/1"), Some("7"), "status=draft");

    assert_eq!(refused.status, 403);
    assert!(refused.body.contains("status"), "{}", refused.body);
}

/// A create the rule refuses is a 400: nothing exists yet, so there is nothing
/// to conceal.
#[test]
fn a_create_the_rule_refuses_is_a_400() {
    let router = mounted(Posts::new());

    let refused = answered(
        &router,
        Verb::Post,
        &at(""),
        Some("7"),
        "author_id=8&title=theirs",
    );

    assert_eq!(refused.status, 400);
}

// ---------------------------------------------------------------------------
// The answers
// ---------------------------------------------------------------------------

/// The same envelope the other surface sends, through the same writer.
#[test]
fn a_listing_is_the_envelope_with_the_total_of_the_set_it_paged() {
    let posts = Posts::new();
    seed_eight(&posts);
    let router = mounted(posts);

    let answer = answered(&router, Verb::Get, &at("?page=1&perPage=2"), Some("7"), "");

    assert_eq!(answer.status, 200);
    assert_eq!(answer.header("content-type"), Some("application/json"));
    assert!(
        answer
            .body
            .starts_with("{\"page\":1,\"perPage\":2,\"totalItems\":4,\"totalPages\":2,"),
        "{}",
        answer.body
    );
}

/// A sort this resource does not declare is a 400 rather than a different
/// order than the one the client paginated against.
#[test]
fn a_sort_the_resource_does_not_declare_is_a_400() {
    let posts = Posts::new();
    seed_eight(&posts);
    let router = mounted(posts);

    let refused = answered(&router, Verb::Get, &at("?sort=-status"), Some("7"), "");

    assert_eq!(refused.status, 400);
    assert!(refused.body.contains("status"), "{}", refused.body);
}

#[test]
fn a_create_answers_201_with_a_location_that_points_at_the_new_row() {
    let router = mounted(Posts::new());

    let made = answered(
        &router,
        Verb::Post,
        &at(""),
        Some("7"),
        "author_id=7&title=new",
    );

    assert_eq!(made.status, 201);
    assert_eq!(made.header("location"), Some(at("/1").as_str()));
    assert!(made.body.contains("\"title\":\"new\""), "{}", made.body);
}

/// 204, and no `content-type`: there is no body for one to describe.
#[test]
fn a_delete_is_204_with_no_body_and_no_content_type() {
    let posts = Posts::new();
    seed_eight(&posts);
    let router = mounted(posts);

    let done = answered(&router, Verb::Delete, &at("/1"), Some("admin"), "");

    assert_eq!(done.status, 204);
    assert!(done.body.is_empty());
    assert_eq!(done.header("content-type"), None);
}

// ---------------------------------------------------------------------------
// The two routes a form can reach
// ---------------------------------------------------------------------------

/// A browser cannot send PATCH. The same operation, the same status table.
#[test]
fn a_form_post_to_a_record_updates_it() {
    let posts = Posts::new();
    posts.db.seed(&[(1, Some(7), "published", "one")]);
    let router = mounted(posts);

    let answer = answered(&router, Verb::Post, &at("/1"), Some("7"), "title=renamed");

    assert_eq!(answer.status, 200);
    assert!(
        answer.body.contains("\"title\":\"renamed\""),
        "{}",
        answer.body
    );
}

/// A browser cannot send DELETE either, and the policy still decides.
#[test]
fn a_form_post_to_delete_removes_the_row_and_obeys_the_same_rule() {
    let posts = Posts::new();
    seed_eight(&posts);
    let router = mounted(posts);

    let refused = answered(&router, Verb::Post, &at("/1/delete"), Some("7"), "");
    assert_eq!(refused.status, 403, "only an administrator deletes");

    let done = answered(&router, Verb::Post, &at("/1/delete"), Some("admin"), "");
    assert_eq!(done.status, 204);
}

// ---------------------------------------------------------------------------
// The other door
// ---------------------------------------------------------------------------

/// A refusal is an answer; the application's own machinery failing is not.
/// It leaves as the project's `Error`, which is where that project sends a
/// 500, rather than being rendered to whoever asked.
#[test]
fn a_failure_of_the_application_leaves_as_an_error_and_not_as_a_body() {
    let posts = Posts::new();
    seed_eight(&posts);
    // The table the policy compiled against is gone: this is the storage layer
    // failing, not a rule refusing.
    posts.db.execute("DROP TABLE posts", &[]);
    let router = mounted(posts);

    let failure = call(&router, Verb::Get, &at(""), Some("7"), "")
        .expect_err("a broken database is not an answer");

    assert_eq!(failure.status, 500);
}
