//! The bundle, mounted, and the statuses it answers with.
//!
//! ADR 3: the plugin describes five routes relative to a prefix it does not
//! choose, and the application mounts them in one line at a prefix it does.

#![cfg(feature = "plugin")]

mod common;

use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::{App, AppBuilder};
use brocanth::plugin_context::{Base, Context};
use brocanth::plugin_hooks::{NoHooks, Request, RequestState, Response};
use brocanth::plugin_routes::RouteCx;
use brocanth::routing::Verb;
use brocanth_resource::{Failure, ResourceRoutes, SubjectSource};
use common::{seed_eight, Posts, Visitor};

/// The subject comes out of a header here. In an application it comes out of
/// the context, which this crate never has to know: `SubjectSource<E>` is
/// generic over the provider list exactly as `RouteSource<E>` is.
impl SubjectSource<Base> for Posts {
    fn subject(&self, cx: &RouteCx<'_, Base>) -> Result<Visitor, Failure> {
        match cx.request().headers().get("x-user") {
            None => Ok(Visitor::anonymous()),
            Some(value) => match value.to_str() {
                Ok("admin") => Ok(Visitor::admin()),
                Ok("banned") => Ok(Visitor::banned()),
                Ok(id) => id
                    .parse()
                    .map(Visitor::user)
                    .map_err(|_| Failure::NoSubject),
                Err(_) => Err(Failure::NoSubject),
            },
        }
    }
}

struct Mounted {
    app: App<Base, NoHooks>,
    context: Context<Base>,
}

fn mount(posts: Posts) -> Mounted {
    let mut app = AppBuilder::new()
        .plugin(ResourceRoutes::new(posts))
        .build()
        .expect("the application builds");
    let taken = app
        .mount_routes::<ResourceRoutes<Posts>>("/api/collections/posts/records")
        .expect("the bundle mounts");
    assert_eq!(taken, 5);
    let context = app.context();
    Mounted { app, context }
}

impl Mounted {
    fn call(&self, name: &str, request: Request, params: &[(&str, &str)]) -> Response {
        let route = self
            .app
            .route_plan()
            .named(name)
            .unwrap_or_else(|| panic!("no route named `{name}`"));
        let params = brocanth::routing_params::Params::new(params);
        let mut state = RequestState::new();
        let mut cx = RouteCx::new(&request, &params, &self.context, &mut state);
        route
            .handler
            .call_sync(&mut cx)
            .expect("every route in this bundle is synchronous")
            .expect("the handler answers rather than failing")
    }
}

fn get(uri: &str, who: Option<&str>) -> Request {
    request("GET", uri, who, Vec::new())
}

fn request(method: &str, uri: &str, who: Option<&str>, body: Vec<u8>) -> Request {
    let mut builder = http::Request::builder().method(method).uri(uri);
    if let Some(who) = who {
        builder = builder.header("x-user", who);
    }
    builder.body(body).expect("a request")
}

fn body_of(response: &Response) -> String {
    String::from_utf8(
        response
            .body()
            .buffered()
            .expect("a buffered body")
            .to_vec(),
    )
    .expect("UTF-8")
}

#[test]
fn five_routes_appear_under_the_prefix_the_application_chose() {
    let mounted = mount(Posts::new());
    let plan = mounted.app.route_plan();
    let path = |name: &str| plan.named(name).expect("a route").path.clone();

    assert_eq!(path("posts.list"), "/api/collections/posts/records");
    assert_eq!(path("posts.create"), "/api/collections/posts/records");
    assert_eq!(path("posts.view"), "/api/collections/posts/records/{id}");
    assert_eq!(path("posts.update"), "/api/collections/posts/records/{id}");
    assert_eq!(path("posts.delete"), "/api/collections/posts/records/{id}");

    assert_eq!(plan.named("posts.create").unwrap().verb, Verb::Post);
    assert_eq!(plan.named("posts.update").unwrap().verb, Verb::Patch);
    assert_eq!(plan.named("posts.delete").unwrap().verb, Verb::Delete);
}

#[test]
fn a_listing_is_an_envelope_whose_total_counts_what_the_pages_hold() {
    let posts = Posts::new();
    seed_eight(&posts);
    let mounted = mount(posts);

    let response = mounted.call("posts.list", get("/?page=1&perPage=2", Some("7")), &[]);

    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "application/json"
    );
    let body = body_of(&response);
    assert!(
        body.starts_with("{\"page\":1,\"perPage\":2,\"totalItems\":4,\"totalPages\":2,"),
        "{body}"
    );
    assert!(
        body.contains("\"title\":\"one\"") && body.contains("\"title\":\"two\""),
        "{body}"
    );
}

#[test]
fn skip_total_leaves_the_fields_out_rather_than_filling_them_with_a_sentinel() {
    let posts = Posts::new();
    seed_eight(&posts);
    let mounted = mount(posts);

    let body = body_of(&mounted.call("posts.list", get("/?skipTotal=1", Some("7")), &[]));

    assert!(!body.contains("totalItems"), "{body}");
    assert!(!body.contains("totalPages"), "{body}");
    assert!(body.contains("\"items\":["), "{body}");
}

#[test]
fn an_unsatisfied_list_rule_is_an_empty_page_and_a_null_rule_is_a_403() {
    let posts = Posts::new();
    posts.db.seed(&[(1, Some(7), "draft", "hidden")]);
    let mounted = mount(posts);

    // Nothing matches: 200 and no items. A 403 here would be an oracle for
    // "something matches this filter".
    let response = mounted.call("posts.list", get("/", None), &[]);
    assert_eq!(response.status(), 200);
    assert!(body_of(&response).contains("\"items\":[]"));

    // `Pred::Never`: a fact about the collection, so it is said.
    let response = mounted.call("posts.list", get("/", Some("banned")), &[]);
    assert_eq!(response.status(), 403);
}

#[test]
fn a_record_you_may_not_see_is_a_404_and_never_a_403() {
    let posts = Posts::new();
    seed_eight(&posts);
    let mounted = mount(posts);

    // Row 2 is a draft of author 7's. Anonymous may not see it, and row 99
    // does not exist. One answer.
    let hidden = mounted.call("posts.view", get("/2", None), &[("id", "2")]);
    let absent = mounted.call("posts.view", get("/99", None), &[("id", "99")]);

    assert_eq!(hidden.status(), 404);
    assert_eq!(absent.status(), 404);
    assert_eq!(body_of(&hidden), body_of(&absent), "byte for byte");
}

#[test]
fn a_masked_column_is_a_403_that_says_which_one() {
    let posts = Posts::new();
    seed_eight(&posts);
    let mounted = mount(posts);

    let response = mounted.call(
        "posts.update",
        request("PATCH", "/1", Some("7"), b"status=draft".to_vec()),
        &[("id", "1")],
    );

    assert_eq!(response.status(), 403);
    assert!(
        body_of(&response).contains("status"),
        "{}",
        body_of(&response)
    );
}

#[test]
fn a_lost_race_is_a_409_rather_than_a_quiet_200() {
    let posts = Posts::new();
    posts.db.seed(&[(1, Some(7), "published", "one")]);
    *posts.db.race.lock().unwrap() = Some((
        "UPDATE posts SET author_id = 99 WHERE id = 1".to_owned(),
        Vec::new(),
    ));
    let mounted = mount(posts);

    let response = mounted.call(
        "posts.update",
        request("PATCH", "/1", Some("7"), b"title=renamed".to_vec()),
        &[("id", "1")],
    );

    assert_eq!(response.status(), 409);
}

#[test]
fn a_create_answers_201_with_a_location_that_points_at_the_new_row() {
    let mounted = mount(Posts::new());

    let response = mounted.call(
        "posts.create",
        request("POST", "/", Some("7"), b"author_id=7&title=new".to_vec()),
        &[],
    );

    assert_eq!(response.status(), 201);
    assert_eq!(
        response.headers().get("location").unwrap(),
        "/api/collections/posts/records/1"
    );
    assert!(body_of(&response).contains("\"title\":\"new\""));
}

#[test]
fn a_create_the_rule_refuses_is_a_400_and_one_nobody_may_make_is_a_403() {
    let mounted = mount(Posts::new());

    let refused = mounted.call(
        "posts.create",
        request("POST", "/", Some("7"), b"author_id=8&title=theirs".to_vec()),
        &[],
    );
    assert_eq!(refused.status(), 400);

    let shut = mounted.call(
        "posts.create",
        request("POST", "/", None, b"author_id=8&title=theirs".to_vec()),
        &[],
    );
    assert_eq!(shut.status(), 403);
}

#[test]
fn a_delete_is_204_and_a_delete_rule_of_never_is_403() {
    let posts = Posts::new();
    seed_eight(&posts);
    let mounted = mount(posts);

    let refused = mounted.call(
        "posts.delete",
        request("DELETE", "/1", Some("7"), Vec::new()),
        &[("id", "1")],
    );
    assert_eq!(refused.status(), 403);

    let done = mounted.call(
        "posts.delete",
        request("DELETE", "/1", Some("admin"), Vec::new()),
        &[("id", "1")],
    );
    assert_eq!(done.status(), 204);
    assert!(done.body().is_empty());
}

#[test]
fn an_update_that_hides_the_row_from_its_own_author_is_a_204() {
    let posts = Posts::new();
    posts.db.seed(&[(1, Some(7), "draft", "one")]);
    let mounted = mount(posts);

    let response = mounted.call(
        "posts.update",
        request("PATCH", "/1", Some("7"), b"author_id=8".to_vec()),
        &[("id", "1")],
    );

    assert_eq!(
        response.status(),
        204,
        "the write stood; there is nothing to show"
    );
}

#[test]
fn a_sort_the_resource_does_not_declare_is_a_400_rather_than_a_different_order() {
    let posts = Posts::new();
    seed_eight(&posts);
    let mounted = mount(posts);

    let response = mounted.call("posts.list", get("/?sort=-status", Some("7")), &[]);

    assert_eq!(response.status(), 400);
    assert!(body_of(&response).contains("status"));
}

#[test]
fn a_page_that_is_not_a_number_is_a_400_rather_than_a_quiet_page_one() {
    let posts = Posts::new();
    seed_eight(&posts);
    let mounted = mount(posts);

    let response = mounted.call("posts.list", get("/?page=two", Some("7")), &[]);
    assert_eq!(response.status(), 400);
}
