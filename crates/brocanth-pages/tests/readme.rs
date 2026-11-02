//! The README's examples, compiled and run.
//!
//! `pages!` and `html!` read a directory while the crate compiles, so a
//! doctest cannot hold them: it has no page tree beside it. This test has
//! one, under `tests/readme/src/pages`, laid out as a generated project lays
//! out `src/pages`. If the README changes, change this file and the tree
//! with it.

#[path = "readme/src/pages/mod.rs"]
mod pages;

use brocanth::routing::{Dispatch, Router, Verb};
use pages::{Ctx, Handler, Response};

fn table() -> Router<Handler> {
    pages::router().expect("the pages produce a table")
}

fn call(router: &Router<Handler>, verb: Verb, path: &str, held: u32) -> Response {
    match router.dispatch(verb, path) {
        Dispatch::Found { handler, matched } => {
            let params = matched
                .params()
                .iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect();
            let mut ctx = Ctx { params, held };
            handler(&mut ctx).expect("the handler answers")
        }
        _ => panic!("{verb} {path} is not routed"),
    }
}

#[test]
fn a_file_is_a_route_and_a_function_is_a_verb() {
    let router = table();
    let mut patterns: Vec<&str> = router.routes().iter().map(|(t, _)| t.pattern()).collect();
    patterns.sort();
    assert_eq!(patterns, ["/", "/users/{id}"]);
    assert_eq!(
        router.allow_for("/users/7").map(|a| a.to_string()),
        Some("GET, HEAD, POST".to_owned())
    );
    assert_eq!(
        pages::handlers(),
        &[
            (Verb::Get, "/", "index::handle"),
            (Verb::Get, "/users/{id}", "users::_id_::handle"),
            (Verb::Post, "/users/{id}", "users::_id_::handle_post"),
        ]
    );
}

#[test]
fn the_helpers_take_exactly_the_routes_parameters() {
    assert_eq!(pages::routes::root::path(), "/");
    assert_eq!(pages::routes::users::id::path(7), "/users/7");
}

#[test]
fn layouts_stack_from_the_root_down() {
    let router = table();
    assert_eq!(
        call(&router, Verb::Get, "/", 0).body,
        "<html><main><h1>Home</h1></main></html>"
    );
    assert_eq!(
        call(&router, Verb::Get, "/users/7", 0).body,
        "<html><main><section><h1>User 7</h1></section></main></html>"
    );
}

#[test]
fn a_partial_leaves_out_the_layers_the_client_holds() {
    let router = table();
    assert_eq!(
        call(&router, Verb::Get, "/users/7", 1).body,
        "<section><h1>User 7</h1></section>"
    );
    assert_eq!(
        call(&router, Verb::Get, "/users/7", 2).body,
        "<h1>User 7</h1>"
    );
    // Deeper than the chain is the page alone, not an error.
    assert_eq!(
        call(&router, Verb::Get, "/users/7", 9).body,
        "<h1>User 7</h1>"
    );
}

#[test]
fn a_second_verb_lives_in_the_same_file() {
    let router = table();
    let answer = call(&router, Verb::Post, "/users/7", 0);
    assert_eq!(answer.status, 303);
    assert_eq!(answer.body, "/users/7");
}

#[test]
fn an_id_that_is_not_a_number_is_a_400() {
    let mut ctx = Ctx {
        params: vec![("id".to_owned(), "seven".to_owned())],
        held: 0,
    };
    let failed = pages::users::_id_::handle(&mut ctx)
        .err()
        .expect("the parse fails");
    assert_eq!(failed.0, 400);
}
