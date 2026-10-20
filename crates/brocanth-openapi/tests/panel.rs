//! The document's tab, rendered through the real admin.
//!
//! The document is the one `tests/generated_document.rs` builds — derived
//! from a router, described in places and undescribed in others, which is
//! what a real one looks like. The admin is real and the policy is real.
//!
//! The questions are asked with [`brocanth_test::Html`] rather than with
//! `contains`, because a path occurs in an OpenAPI preview in several
//! places at once — as a link, as text, inside an operationId — and
//! "`/posts/{id}` is a link to its own screen" is the question these tests
//! mean.

#![cfg(feature = "admin")]

use std::sync::Arc;

use brocanth::routing::{Router, RouterBuilder, Verb};
use brocanth_admin::policy::{policy_fn, Principal, Request};
use brocanth_admin::rt::block_on;
use brocanth_admin::{Admin, AdminConfig, Discovery, Fixture, Submission};
use brocanth_openapi::admin::ApiPanel;
use brocanth_openapi::{Document, Schema, SecurityScheme, SpecBuilder};
use brocanth_test::Html;

struct Operator;

fn blog_router() -> Router<&'static str> {
    let mut routes: RouterBuilder<&'static str> = Router::builder();
    routes.route("/posts", Verb::Get, "posts_index").unwrap();
    routes.route("/posts", Verb::Post, "posts_create").unwrap();
    routes
        .route("/posts/{id}", Verb::Get, "posts_show")
        .unwrap();
    routes
        .route("/posts/{id}", Verb::Delete, "posts_destroy")
        .unwrap();
    routes.build().unwrap()
}

/// A document with one described operation, one undescribed one, a
/// deprecated one and a document-wide security requirement. Each of those
/// is a thing the screen has to say correctly.
fn blog_document() -> Document {
    SpecBuilder::new("Blog", "1.0.0")
        .component(
            "Post",
            Schema::object()
                .required("id", Schema::integer())
                .required("title", Schema::string()),
        )
        .security_scheme("bearer", SecurityScheme::bearer("JWT"))
        .require("bearer")
        .routes_from_router(&blog_router())
        .describe("/posts", Verb::Get, |op| {
            op.summary("List posts")
                .query_param("page", Schema::integer().minimum(1.0))
                .json_response(
                    200,
                    "A page of posts",
                    Schema::array(Schema::reference("Post")),
                )
        })
        .describe("/posts/{id}", Verb::Delete, |op| {
            op.summary("Delete a post").deprecated()
        })
        .build()
        .expect("the document derives")
}

fn admin(document: &Arc<Document>) -> Admin {
    Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .panel(ApiPanel::new(Arc::clone(document)))
            .with_policy(policy_fn(|_: &Principal<'_>, _: Request<'_>| true)),
    )
}

fn get(admin: &Admin, path: &str) -> Html {
    let who = Operator;
    let response =
        block_on(admin.dispatch(Principal::new(&who), "GET", path, "", &Submission::new()))
            .expect("dispatch");
    assert_eq!(response.status(), 200, "{path}");
    Html::new(response.html().expect("a page"))
}

fn status(admin: &Admin, method: &str, path: &str) -> u16 {
    let who = Operator;
    block_on(admin.dispatch(Principal::new(&who), method, path, "", &Submission::new()))
        .expect("dispatch")
        .status()
}

fn document() -> Arc<Document> {
    Arc::new(blog_document())
}

#[test]
fn every_operation_is_listed_and_each_one_links_to_its_own_screen() {
    let page = get(&admin(&document()), "/admin/api");

    page.assert_text("Blog");
    page.assert_text("4 operations over 2 paths");

    // Four routes, four links, and each `operationId` is the one the
    // builder derived rather than one this tab invented.
    for id in [
        "get_posts",
        "post_posts",
        "get_posts_by_id",
        "delete_posts_by_id",
    ] {
        page.assert_link_to(&format!("/admin/api/op/{id}"));
    }
}

#[test]
fn an_undescribed_operation_still_appears_with_an_empty_summary() {
    let page = get(&admin(&document()), "/admin/api");

    // `/posts` POST was never described, so it has no summary. The row is
    // there anyway: an undescribed route is a true thing about the
    // document and a preview that hid it would be lying about coverage.
    page.assert_text("List posts");
    page.assert_link_to("/admin/api/op/post_posts");
    page.assert_no_text("Create a post");
}

#[test]
fn a_deprecated_operation_says_so_on_the_list_and_on_its_screen() {
    let a = admin(&document());

    get(&a, "/admin/api").assert_text("deprecated");
    get(&a, "/admin/api/op/delete_posts_by_id").assert_text("deprecated");
}

#[test]
fn one_operations_screen_names_its_parameters_its_schemas_and_its_statuses() {
    let page = get(&admin(&document()), "/admin/api/op/get_posts");

    page.assert_text("List posts");
    page.assert_text("page");
    page.assert_text("query");
    // A schema is named rather than drawn, and an array of a component is
    // named after the component.
    page.assert_text("Post[]");
    page.assert_text("A page of posts");
    // And a way back to the list that is a link, not a browser button.
    page.assert_link_to("/admin/api");
}

#[test]
fn a_path_parameter_the_router_implied_is_on_the_screen() {
    // `/posts/{id}` was never described, and `routes_from_router` still
    // produced its path parameter. That is this crate's own claim, and the
    // tab is where somebody would notice it had stopped being true.
    let page = get(&admin(&document()), "/admin/api/op/get_posts_by_id");

    page.assert_text("id");
    page.assert_text("path");
    page.assert_text("required");
}

#[test]
fn an_inherited_security_requirement_is_shown_as_inherited() {
    let page = get(&admin(&document()), "/admin/api/op/get_posts");

    page.assert_text("Authentication");
    page.assert_text("bearer — inherited from the document");
}

#[test]
fn the_tab_is_in_the_rail_and_the_screens_have_no_buttons() {
    let a = admin(&document());

    for path in ["/admin/api", "/admin/api/op/get_posts"] {
        let page = get(&a, path);
        page.within("nav", "aria-label", "Sections")
            .assert_link_to("/admin/api");
        assert!(page.buttons().is_empty(), "{path} draws no buttons");
        assert!(page.forms().is_empty(), "{path} posts nothing");
    }
}

#[test]
fn an_unknown_operation_and_a_post_are_both_the_admins_404() {
    let a = admin(&document());

    assert_eq!(status(&a, "GET", "/admin/api/op/not_an_operation"), 404);
    assert_eq!(status(&a, "GET", "/admin/api/op"), 404);
    assert_eq!(status(&a, "POST", "/admin/api"), 404);
}

/// Prints the two screens, for a human who wants to see them.
#[test]
#[ignore = "prints pages, asserts nothing"]
fn show() {
    let a = admin(&document());
    for path in ["/admin/api", "/admin/api/op/get_posts"] {
        println!("\n===== {path} =====");
        println!("{}", get(&a, path).source());
    }
}
