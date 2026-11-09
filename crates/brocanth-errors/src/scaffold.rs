//! `errors`, as a choice on the `brocanth create` menu.
//!
//! What a project gets:
//!
//! - `src/plugins/errors/mod.rs`: `catching` and `caught`, which turn a
//!   page's failure into the nearest error page, and the tests.
//! - `src/pages/not-found.rsx` and `src/pages/error.rsx`, with `rsx`.
//! - [`Errors`](crate::Errors), registered with its setting. It publishes
//!   the reporters and reports a hook's failure.
//! - The `errors` word in `pages!("src/pages", errors)` and the 404 arm of
//!   `dispatch` in `src/app.rs`. The generator writes both, because both
//!   files are its own (`crates/brocanth-cli/src/commands/create.rs`).
//!
//! Compiled only under the `scaffold` feature.

use brocanth_scaffold::snippet::{configurable_hooking, Hooking};
use brocanth_scaffold::{Category, Dep, Hook, Manifest, Module, Project, Scaffold, Setting, Slot};

/// HTML error pages, a development page with the cause, and reporters.
pub struct ErrorPages;

impl Module for ErrorPages {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "errors",
            "HTML 404 and 500 pages in your layouts, the cause in a debug build, a reporter hook",
        )
        .category(Category::Renderer)
        .category(Category::Middleware)
        .adapts_to(
            "config",
            "its setting is read from the config file, not written as a literal",
        )
        .adapts_to(
            "rsx",
            "writes `not-found.rsx` and `error.rsx`, rendered inside the layouts",
        )
        .adapts_to(
            "i18n-routing",
            "its tests ask for paths under the `/en` segment, which the redirect leaves alone",
        )
        // The name the plugin registers its error hook under. On: a hook's
        // failure is a 5xx a reporter must hear about.
        .hook(
            "report",
            "tell the reporters about a hook's failure",
            Hook::on(),
        )
        // The 404 arm needs the page table; `rsx` writes the layouts the
        // error pages sit in.
        .after("rsx")
        .asked_in("pages", "HTML pages for a missing page and a failure")
        .krate(Dep::brocanth("brocanth-errors").feature_with("config", "config"))
        .absent(
            "errors",
            "a missing page answers a plain `Not Found`, a failure a plain `Internal Server Error`",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/errors/mod.rs", plugin_rs(project));
        s.at(Slot::PluginsMod).once("pub mod errors;");
        // The names `pages!(.., errors)` expands to, from the one place a
        // page imports everything else.
        // `Caught` only with `rsx`: only an `.rsx` error page names it.
        let caught = if project.has("rsx") { ", Caught" } else { "" };
        s.at(Slot::PagesItems)
            .block(PAGES_ITEMS.replace("@CAUGHT@", caught));
        if project.has("rsx") {
            s.file("src/pages/not-found.rsx", NOT_FOUND_RSX);
            s.file("src/pages/error.rsx", ERROR_RSX);
        }

        let turn = if project.has("config") {
            "under [errors] in config/<env>.toml"
        } else {
            "with `--errors.details=false`"
        };
        configurable_hooking(
            s,
            project,
            Hooking {
                key: "errors",
                section: "errors",
                ty: "brocanth_errors::Errors",
                config_ty: "brocanth_errors::ErrorsConfig",
                prelude: PRELUDE.replace("@TURN@", turn),
                adjust: None,
            },
            vec![Setting::bool(
                "details",
                true,
                "Show the cause of a 500 in a debug build. A release build never shows it.",
            )
            .note("Turn it off to see error.rsx while you design it.")],
        );

        let mut agents = AGENTS.to_string();
        if project.has("rsx") {
            agents.push_str(AGENTS_RSX);
        }
        s.at(Slot::AgentsSections).block(agents);
    }
}

fn plugin_rs(project: &Project) -> String {
    // The starter's `/users/{id}` page parses its parameter, so `/users/x`
    // is a real 500 to test with. With `rsx` the 404 is the layout's page.
    let marker = if project.has("rsx") {
        "Page not found"
    } else {
        "<h1>Not Found</h1>"
    };
    // With `i18n-routing` a path without a locale segment is a redirect,
    // so the tests ask under one.
    let under = if project.has("i18n-routing") {
        "/en"
    } else {
        ""
    };
    PLUGIN_RS
        .replace("@NOT_FOUND@", marker)
        .replace("@UNDER@", under)
}

const PAGES_ITEMS: &str = "/// What `pages!(\"src/pages\", errors)` expands to: every page goes \
through\n/// `catching`, and a failure through `caught`. See `src/plugins/errors/mod.rs`.\n\
pub use crate::plugins::errors::{catching, caught@CAUGHT@, ErrorPage};\n";

const PLUGIN_RS: &str = r#"//! Written by `brocanth`. What a failure becomes.
//!
//! `pages!("src/pages", errors)` wraps every page in [`catching`]. A page
//! that returns an error reaches [`caught`] with the error pages nearest its
//! file: `not-found.rsx` for a 404, `error.rsx` for the rest, each inside
//! the layouts above it. A path no route matched reaches it too, through
//! `pages::not_found`, which `src/app.rs` calls.
//!
//! A 5xx is logged and told to every reporter. A debug build shows the
//! cause on a page of its own; a release build never does.

use std::sync::Arc;

use brocanth_errors::{Origin, Report, Reporters};

use crate::pages::{Ctx, Error, Handler, Response};

pub use brocanth_errors::Caught;

/// An error page: `not-found.rsx` or `error.rsx`, inside its layouts.
/// `pages!` writes one function per file.
pub type ErrorPage = fn(&mut Ctx, &Caught) -> Result<Response, Error>;

/// A page whose failure goes to [`caught`], with the error pages nearest
/// its file. `pages!` wraps every page in it.
pub fn catching(
    handler: Handler,
    not_found: Option<ErrorPage>,
    failed: Option<ErrorPage>,
) -> Handler {
    // Shared, because the future a request gets must not borrow the table.
    let handler = Arc::new(handler);
    Box::new(move |ctx| {
        let handler = Arc::clone(&handler);
        Box::pin(async move {
            let answered = (handler.as_ref())(&mut *ctx).await;
            match answered {
                Ok(response) => Ok(response),
                Err(error) => caught(ctx, error, not_found, failed),
            }
        })
    })
}

/// What a failure becomes.
///
/// An error that names its own `content-type` is for a program, and so is
/// a client that does not read HTML: both keep the plain answer, which
/// `Err` hands back to `src/app.rs`. Every other failure answers a page
/// with the error's status and headers.
pub fn caught(
    ctx: &mut Ctx,
    error: Error,
    not_found: Option<ErrorPage>,
    failed: Option<ErrorPage>,
) -> Result<Response, Error> {
    let reporters = ctx.provided::<Reporters, _>().cloned();
    if error.status >= 500 {
        if let Some(reporters) = &reporters {
            let method = ctx.verb.to_string();
            reporters.report(&Report {
                status: error.status,
                message: &error.message,
                method: &method,
                path: &ctx.path,
                origin: Origin::Page,
            });
        }
    }
    let typed = error
        .headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("content-type"));
    if typed || !brocanth_errors::accepts_html(ctx.header("accept")) {
        return Err(error);
    }

    let caught = Caught::new(error.status, &error.message, &ctx.path);
    if error.status >= 500 {
        let (status, message) = (error.status, &error.message);
        crate::plugins::say!(error, "error {status}: {message}"; status, %message);
        // A release binary never shows the cause, whatever the setting.
        if cfg!(debug_assertions) && reporters.as_ref().is_some_and(Reporters::details) {
            let method = ctx.verb.to_string();
            let page =
                brocanth_errors::development_page(&caught, &error.message, &method, ctx.headers());
            return Ok(answer(page, error));
        }
    }

    let page = if caught.is_not_found() {
        not_found
    } else {
        failed
    };
    let body = match page.map(|page| page(ctx, &caught)) {
        Some(Ok(rendered)) => rendered.body,
        Some(Err(broken)) => {
            // The error page failed too. The plain page cannot.
            let message = broken.message;
            crate::plugins::say!(error, "the error page failed: {message}"; %message);
            brocanth_errors::plain_page(&caught)
        }
        None => brocanth_errors::plain_page(&caught),
    };
    Ok(answer(body, error))
}

/// A page with the error's status, and the headers it carried: a
/// `www-authenticate` for a 401, a `retry-after` for a 429.
fn answer(body: String, error: Error) -> Response {
    let mut response = Response::new(error.status, body);
    response.headers = error.headers;
    response
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use brocanth::routing::Verb;
    use brocanth_errors::{Report, Reporter, Reporters};
    use brocanth_server::{Request, Response};

    /// A path no route answers, and one whose page fails with a 500.
    const MISSING: &str = "@UNDER@/nowhere";
    const BROKEN: &str = "@UNDER@/users/x";

    fn body(response: &Response) -> String {
        let bytes = response.body.bytes().expect("a whole body");
        String::from_utf8(bytes.to_vec()).expect("utf-8")
    }

    #[tokio::test]
    async fn a_path_no_route_matches_answers_an_html_404() {
        let app = crate::app::build().expect("the application builds");
        let request = Request::new(Verb::Get, MISSING).with_header("accept", "text/html");
        let response = crate::app::handle(&app, request).await;
        assert_eq!(response.status, 404);
        let body = body(&response);
        assert!(body.contains("@NOT_FOUND@"), "{body}");
    }

    #[tokio::test]
    async fn a_client_that_reads_json_keeps_the_plain_answer() {
        let app = crate::app::build().expect("the application builds");
        let request = Request::new(Verb::Get, MISSING).with_header("accept", "application/json");
        let response = crate::app::handle(&app, request).await;
        assert_eq!(response.status, 404);
        assert_eq!(body(&response), "Not Found");
    }

    struct Heard(Arc<Mutex<Vec<String>>>);

    impl Reporter for Heard {
        fn report(&self, report: &Report<'_>) {
            if let Ok(mut heard) = self.0.lock() {
                heard.push(format!("{} {}", report.status, report.path));
            }
        }
    }

    #[tokio::test]
    async fn a_failure_is_reported_and_shows_its_cause_only_in_a_debug_build() {
        let app = crate::app::build().expect("the application builds");
        let heard = Arc::new(Mutex::new(Vec::new()));
        let cx = app.core.context();
        let reporters: &Reporters = cx.get();
        reporters.add(Heard(Arc::clone(&heard)));

        // `/users/{id}` parses its parameter, so this fails with a 500. The
        // reporter hears the path the page answered.
        let request = Request::new(Verb::Get, BROKEN).with_header("accept", "text/html");
        let response = crate::app::handle(&app, request).await;
        assert_eq!(response.status, 500);
        assert_eq!(*heard.lock().expect("lock"), ["500 /users/x"]);
        let cause = "invalid digit found in string";
        assert_eq!(
            body(&response).contains(cause),
            cfg!(debug_assertions),
            "{}",
            body(&response)
        );
    }
}
"#;

const NOT_FOUND_RSX: &str = r#"// The page for a 404: a path nothing answers, or a page that returned
// `Error::new(404, ..)`. The layouts above this file wrap it, as they wrap a
// page. A `not-found.rsx` in a directory below answers for that directory.
//
// `caught` is what `pages!` gives an error page: the status, the reason
// phrase, the message of a 4xx, and the path.

use crate::pages::Caught;

pub fn NotFound(caught: &Caught) -> Element {
    <h1>Page not found</h1>
    <p>Nothing answers at <code>{caught.path}</code>.</p>
    if let Some(message) = &caught.message {
        <p>{message}</p>
    }
    <p><a href="/">Back to the home page</a></p>
}
"#;

const ERROR_RSX: &str = r#"// The page for every failure but a 404: a page that returned an error, a
// 500 included. The layouts above this file wrap it. An `error.rsx` in a
// directory below answers for that directory.
//
// A 5xx has no message here: the cause is in the log, and a debug build
// shows it on a page of its own. A 4xx has the message the page gave.

use crate::pages::Caught;

pub fn ErrorPage(caught: &Caught) -> Element {
    <h1>{caught.status} {caught.reason}</h1>
    if let Some(message) = &caught.message {
        <p>{message}</p>
    } else {
        <p>Something went wrong on our side. Try again in a moment.</p>
    }
    <p><a href="/">Back to the home page</a></p>
}
"#;

/// The comment above the registration.
const PRELUDE: &str = r#"
        // -- brocanth-errors: error pages and reporters --------------------
        //
        // Publishes the reporters another plugin adds to, and reports a
        // hook's failure. A page's failure goes through
        // `crate::plugins::errors::caught`. Whether a debug build shows the
        // cause is a setting, @TURN@.
"#;

const AGENTS: &str = "\n\
    ## Errors\n\
    \n\
    A page fails with `Err(Error::new(status, message))` or `?`, and\n\
    `src/plugins/errors/mod.rs` turns the failure into a page. Do not build\n\
    an error page in a handler. A 404 is `Error::new(404, \"no user 7\")`. A\n\
    4xx message is shown to the visitor; a 5xx message is logged and never\n\
    shown, so put the cause in it.\n\
    \n\
    An error with a `content-type` header, and a client that does not\n\
    accept HTML, keep the plain answer: an API stays an API.\n\
    \n\
    To hear about every 5xx, implement `brocanth_errors::Reporter` and add\n\
    it in a plugin's `after_config`:\n\
    `lc.services().get::<brocanth_errors::Reporters>()`. A reporter must\n\
    not wait.\n";

const AGENTS_RSX: &str = "\n\
    `not-found.rsx` answers a 404 and `error.rsx` every other failure, for\n\
    the pages in its directory and below it, inside the layouts above it.\n\
    The nearest file wins. The components are `NotFound` and `ErrorPage`,\n\
    and each may declare `caught: &Caught`. After you add one where there\n\
    was none, run `touch src/pages/mod.rs`.\n";
