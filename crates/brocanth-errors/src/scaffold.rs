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
use brocanth_scaffold::{
    AgentsSection, Category, Contribution, Dep, Hook, Manifest, Module, Project, Scaffold, Setting,
    Slot,
};

/// HTML error pages, a development page with the cause, and reporters.
pub struct ErrorPages;

impl Module for ErrorPages {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "errors",
            "HTML 404 and 500 pages in your layouts, the cause in a debug build, a reporter hook",
        )
        .contributes::<AgentsSection>(
            Contribution::new("errors", "how an error page is shown and reported"),
            true,
        )
        .category(Category::Renderer)
        .category(Category::Middleware)
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

        // A flag works in every project; a config file is one more place.
        let turn = "with `--errors.details=false`";
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
pub use crate::plugins::errors::{catching, caught, caught_panic@CAUGHT@, ErrorPage};\n";

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
//!
//! A panic reaches [`caught_panic`] through `pages::panicked`, which
//! `src/app.rs` calls with a context from [`panic_context`]: no request
//! state, because the panic may have left it half-written.

use std::sync::Arc;

use brocanth::plugin_context::Context;
use brocanth::plugin_hooks::Request;
use brocanth::routing::{Params, Verb};
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
    if error.status >= 500 {
        let method = ctx.verb.to_string();
        report(ctx, error.status, &error.message, &method, Origin::Page);
    }
    if plain(ctx, &error) {
        return Err(error);
    }
    if error.status >= 500 {
        let (status, message) = (error.status, &error.message);
        crate::plugins::say!(error, "error {status}: {message}"; status, %message);
    }
    Ok(shown(ctx, error, not_found, failed))
}

/// The context the page for a panic gets: the verb, the path and the
/// `accept` header of the request, and the providers of the application.
///
/// Not the request state, and not a provider built from it. A panic can
/// stop a hook halfway through a write, and a page that read that state
/// would read something no hook finished (PLAN.md ADR 8).
pub fn panic_context(request: &Request, plugins: Context<crate::plugins::Installed>) -> Ctx {
    let verb = Verb::from_http(request.method()).unwrap_or(Verb::Get);
    let accept = request
        .headers()
        .get("accept")
        .and_then(|value| value.to_str().ok())
        .map(|value| vec![("accept".to_string(), value.to_string())])
        .unwrap_or_default();
    Ctx::new(verb, request.uri().path(), Params::empty())
        .with_plugins(plugins)
        .sent("", Vec::new(), accept)
}

/// What a panic becomes: the nearest `error.rsx` with a 500.
///
/// The reporters hear the panic message and the path, with
/// `Origin::Panic` and no method. A client that does not read HTML gets the
/// plain 500. `src/app.rs` has logged the panic already.
pub fn caught_panic(ctx: &mut Ctx, message: &str, failed: Option<ErrorPage>) -> Response {
    report(ctx, 500, message, "", Origin::Panic);
    let error = Error::new(500, message);
    if plain(ctx, &error) {
        return Response::new(500, "Internal Server Error");
    }
    shown(ctx, error, None, failed)
}

/// Tells every reporter about a 5xx.
fn report(ctx: &Ctx, status: u16, message: &str, method: &str, origin: Origin) {
    if let Some(reporters) = ctx.provided::<Reporters, _>() {
        reporters.report(&Report {
            status,
            message,
            method,
            path: &ctx.path,
            origin,
        });
    }
}

/// Whether the failure keeps the plain answer: an error that names its own
/// `content-type` is for a program, and so is a client that does not read
/// HTML.
fn plain(ctx: &Ctx, error: &Error) -> bool {
    let typed = error
        .headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("content-type"));
    typed || !brocanth_errors::accepts_html(ctx.header("accept"))
}

/// The page for a failure: the development page for a 5xx in a debug
/// build, else the nearest error page, else the plain page.
fn shown(
    ctx: &mut Ctx,
    error: Error,
    not_found: Option<ErrorPage>,
    failed: Option<ErrorPage>,
) -> Response {
    let caught = Caught::new(error.status, &error.message, &ctx.path);
    if error.status >= 500 {
        let reporters = ctx.provided::<Reporters, _>();
        // A release binary never shows the cause, whatever the setting.
        if cfg!(debug_assertions) && reporters.is_some_and(Reporters::details) {
            let method = ctx.verb.to_string();
            let page =
                brocanth_errors::development_page(&caught, &error.message, &method, ctx.headers());
            return answer(page, error);
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
    answer(body, error)
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

    use brocanth::plugin_routes::BoxFuture;
    use brocanth::routing::{Router, Verb};
    use brocanth_errors::{Report, Reporter, Reporters};
    use brocanth_server::{Request, Response};

    use crate::pages::{Ctx, Error, Handler};

    /// A path no route answers, one whose page fails with a 500, and one
    /// whose page panics.
    const MISSING: &str = "@UNDER@/nowhere";
    const BROKEN: &str = "@UNDER@/users/x";
    const PANICS: &str = "@UNDER@/boom";

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

    /// A page that panics. The generator writes none, so the test puts one
    /// in place of the table `pages::router` builds.
    fn boom(_ctx: &mut Ctx) -> BoxFuture<'_, Result<crate::pages::Response, Error>> {
        panic!("the page broke")
    }

    fn panicking() -> crate::app::App {
        let mut app = crate::app::build().expect("the application builds");
        let mut table = Router::builder();
        table
            .route("/boom", Verb::Get, Box::new(boom) as Handler)
            .expect("a route");
        app.router = table.build().expect("a table");
        app
    }

    struct Panics(Arc<Mutex<Vec<String>>>);

    impl Reporter for Panics {
        fn report(&self, report: &Report<'_>) {
            if let Ok(mut heard) = self.0.lock() {
                heard.push(format!(
                    "{:?} {} [{}] {}: {}",
                    report.origin, report.status, report.method, report.path, report.message
                ));
            }
        }
    }

    #[tokio::test]
    async fn a_panic_answers_the_500_page_and_reaches_the_reporters() {
        let app = panicking();
        let heard = Arc::new(Mutex::new(Vec::new()));
        let cx = app.core.context();
        let reporters: &Reporters = cx.get();
        reporters.add(Panics(Arc::clone(&heard)));

        let request = Request::new(Verb::Get, PANICS)
            .with_header("accept", "text/html")
            .with_header("x-trace", "a-value-the-page-never-reads");
        let response = crate::app::handle(&app, request).await;
        assert_eq!(response.status, 500);
        let html = body(&response);
        assert!(
            response
                .header("content-type")
                .is_some_and(|t| t.starts_with("text/html")),
            "{html}"
        );
        assert!(html.contains("Internal Server Error"), "{html}");
        // The message and the path, and no method: the panic may have left
        // the request half-written, so nothing else is read.
        assert_eq!(
            *heard.lock().expect("lock"),
            ["Panic 500 [] /boom: the page broke"]
        );
        // The page gets the path and the `accept` header, and no other
        // header: a debug build lists what it was given.
        assert!(!html.contains("a-value-the-page-never-reads"), "{html}");
        assert_eq!(
            html.contains("the page broke"),
            cfg!(debug_assertions),
            "{html}"
        );
    }

    #[tokio::test]
    async fn a_client_that_reads_json_keeps_the_plain_500_for_a_panic() {
        let app = panicking();
        let request = Request::new(Verb::Get, PANICS).with_header("accept", "application/json");
        let response = crate::app::handle(&app, request).await;
        assert_eq!(response.status, 500);
        assert_eq!(body(&response), "Internal Server Error");
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
