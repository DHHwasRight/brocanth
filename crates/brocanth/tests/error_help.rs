//! Every error the core builds itself says what to do next, on a `help:`
//! line. A variant that wraps another error passes the other error's text
//! through, and that text carries the line.

use brocanth::plugin_app::BuildError;
use brocanth::plugin_order::OrderError;
use brocanth::plugin_routes::RouteError;
use brocanth::routing::{self, Verb};

fn has_help(error: &dyn std::error::Error) {
    let text = error.to_string();
    assert!(text.contains("\nhelp: "), "no help line in: {text}");
}

fn cycle() -> OrderError {
    OrderError::Cycle {
        first: "a.h".into(),
        second: "b.h".into(),
        path: vec!["a.h".into(), "b.h".into(), "a.h".into()],
    }
}

#[test]
fn every_app_build_error_has_a_help_line() {
    let errors = [
        BuildError::DuplicateInstance { name: "db".into() },
        BuildError::UnknownMiddleware {
            pattern: "/{*rest}".into(),
            name: "csrf".into(),
            suggestion: None,
            offered: vec![],
        },
        BuildError::HooksNotInstalled {
            instance: "auth".into(),
            plugin: "Auth".into(),
        },
        BuildError::HooksNotDeclared {
            instance: "auth".into(),
            plugin: "Auth".into(),
        },
        BuildError::FamilyNotDeclared {
            instance: "auth".into(),
            hook: "session".into(),
            family: "request",
            marker: "Request",
        },
        BuildError::Order(cycle()),
        BuildError::Order(OrderError::Unknown {
            hook: "a.h".into(),
            relation: "after",
            depends_on: "nobody".into(),
            suggestion: None,
            known: vec![],
        }),
        BuildError::HooksOffUnknownInstance {
            instance: "nope",
            suggestion: None,
            registered: vec![],
        },
        BuildError::HooksOffUnknownHook {
            instance: "auth",
            name: "nope",
            suggestion: None,
            registered: vec![],
        },
        BuildError::OrderOnOffHook {
            hook: "a.h".into(),
            relation: "after",
            instance: "b",
            name: "h",
            file: None,
        },
    ];
    for error in &errors {
        has_help(error);
    }
}

#[test]
fn every_route_error_has_a_help_line() {
    let errors = [
        RouteError::Conflict {
            path: "/a".into(),
            verb: Verb::Get,
            first: "x".into(),
            second: "y".into(),
        },
        RouteError::DuplicateName {
            name: "home".into(),
            first: "x".into(),
            second: "y".into(),
        },
        RouteError::BadPath {
            plugin: "x".into(),
            name: "home".into(),
            path: "a".into(),
            reason: "no slash".into(),
        },
        RouteError::NoSuchPlugin {
            plugin: "x".into(),
            suggestion: None,
            registered: vec![],
        },
    ];
    for error in &errors {
        has_help(error);
    }
}

#[test]
fn every_router_build_error_has_a_help_line() {
    let errors = [
        routing::BuildError::DuplicateHandler {
            template: "/a".into(),
            verb: Verb::Get,
        },
        routing::BuildError::Conflict {
            template: "/{a}".into(),
            existing: "/{b}".into(),
        },
        routing::BuildError::Engine {
            template: "/a".into(),
            reason: "no".into(),
        },
        routing::BuildError::Template(
            brocanth::routing::ScopePattern::parse("/{a}.txt").expect_err("partial segment"),
        ),
    ];
    for error in &errors {
        has_help(error);
    }
}
