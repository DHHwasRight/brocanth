//! `debug` and `debug-toolbar` -- what `brocanth create` writes when
//! somebody asks for development tools. Compiled only under the `scaffold`
//! feature.
//!
//! `debug` is the choice in the menu, and writes nothing: it is where the
//! development tools sit, as `ci` is where the providers sit. Its one child
//! today is `debug-toolbar`. Both are `development_only`, so
//! `brocanth check --deploy` refuses a project that keeps them.

use brocanth_scaffold::snippet::{configurable_hooking, Hooking};
use brocanth_scaffold::{
    AgentsSection, Category, Contribution, Dep, Hook, Manifest, Module, Phase, Project, Scaffold,
    Setting, Slot,
};

/// Why a deploy refuses both modules.
const DEVELOPMENT_ONLY: &str = "the panel shows SQL, hook names and every setting to whoever loads a page; a release build shows nothing, and a deploy must not carry it";

/// Development tools, as a choice on the menu.
pub struct Debug;

impl Module for Debug {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "debug",
            "development tools, refused by `check --deploy`; pick them below",
        )
        .category(Category::Middleware)
        .asked_in("operations", "Development tools")
        .development_only(DEVELOPMENT_ONLY)
    }

    fn contribute(&self, _project: &Project, _s: &mut Scaffold) {}
}

/// The toolbar: a panel on each HTML page in a debug build.
pub struct DebugToolbar;

impl Module for DebugToolbar {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "debug-toolbar",
            "a panel on each HTML page in a debug build: timing, SQL statements, hooks, settings",
        )
        .category(Category::Middleware)
        .part_of("debug")
        .because("it is one of the development tools `debug` stands for")
        .development_only(DEVELOPMENT_ONLY)
        .contributes::<AgentsSection>(
            Contribution::new(
                "debug-toolbar",
                "what the development panel shows, and why it never ships",
            ),
            true,
        )
        .adapts_to(
            "model",
            "the ORM's statements are counted per request and listed by shape",
        )
        .hook(
            "start",
            "start the clock and the statement count, first of all request hooks",
            Hook::on().phase(Phase::Identify),
        )
        .hook(
            "panel",
            "write the panel before `</body>` of each HTML answer",
            Hook::on().phase(Phase::Identify),
        )
        .asked_in(
            "operations",
            "A panel on each page in development: timing, SQL, hooks, settings",
        )
        .krate(
            Dep::brocanth("brocanth-debug")
                .feature_with("config", "config")
                .feature_with("model", "model")
                .note("the development panel; inert in a release build"),
        )
        .absent(
            "debug-toolbar",
            "no development panel; a page's SQL and timing are in the logs, if anywhere",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file(
            "src/plugins/debug_toolbar/mod.rs",
            if project.has("model") {
                TOOLBAR_RS.replace("@SQL@", SQL_TEST)
            } else {
                TOOLBAR_RS.replace("@SQL@", NO_SQL_TEST)
            },
        );
        s.at(Slot::PluginsMod).line("pub mod debug_toolbar;");
        configurable_hooking(
            s,
            project,
            Hooking {
                key: "debug-toolbar",
                section: "debug_toolbar",
                ty: "brocanth_debug::DebugToolbar",
                config_ty: "brocanth_debug::ToolbarConfig",
                prelude: COMMENT.to_string(),
                adjust: None,
            },
            settings(),
        );
        // The chains exist only once the application is built, so the
        // toolbar learns them here, at start, with the settings report.
        s.at(Slot::AppContext).block(OBSERVE.to_string());
        s.at(Slot::AgentsSections).block(AGENTS.to_string());
    }
}

fn settings() -> Vec<Setting> {
    vec![
        Setting::bool("enabled", true, "Show the panel on each HTML page.")
            .note("A release build shows none, whatever this says."),
        Setting::integer(
            "max_statements",
            50,
            "The most SQL statement shapes the panel lists.",
        ),
    ]
}

const COMMENT: &str = "
        // -- debug toolbar -------------------------------------------------
        //
        // A panel on each HTML page: timing, SQL, hooks, settings. Inert in a
        // release build; `brocanth check --deploy` refuses it.
        //
";

/// Into `plugins::announce!`: hand the toolbar the hook chains and the
/// settings report. `cx` is the generator's; the line keeps it used when
/// this is the only context line.
const OBSERVE: &str = "    if let Some(toolbar) = $app.plugin::<brocanth_debug::DebugToolbar>() {
        toolbar.observe($app.pipeline(), crate::plugins::settings_report());
    }
    let _ = &cx;
";

/// `src/plugins/debug_toolbar/mod.rs`: the tests that hold the panel to its
/// promise. The plugin is the library's.
const TOOLBAR_RS: &str = r#"//! The development panel: `brocanth_debug::DebugToolbar`, registered in
//! `src/plugins/mod.rs` and set by `[debug_toolbar]`.
//!
//! Each HTML answer gets a panel before `</body>`: the verb, the path, the
//! status and the time; the SQL statements by shape; the hooks of each chain
//! in run order and the ones turned off; the settings in effect, secrets
//! masked. A release build shows nothing: the plugin installs no hook when
//! `debug_assertions` is off.
//!
//! Remove it before a deploy: `brocanth remove debug-toolbar debug`.
//! `brocanth check --deploy` refuses a project that still has it.

#[cfg(test)]
mod tests {
    use brocanth::routing::Verb;
    use brocanth_server::Request;

    /// The page at `path`, after one redirect: a locale segment, a
    /// trailing slash.
    async fn page(path: &str) -> String {
        let app = crate::app::build().expect("the application builds");
        let mut response = crate::app::handle(&app, Request::new(Verb::Get, path)).await;
        if (300..400).contains(&response.status) {
            if let Some(to) = response.header("location").map(str::to_string) {
                response = crate::app::handle(&app, Request::new(Verb::Get, to)).await;
            }
        }
        String::from_utf8_lossy(response.body.bytes().expect("a whole body")).into_owned()
    }

    #[tokio::test]
    async fn a_page_in_a_debug_build_shows_its_timing_hooks_and_settings() {
        let body = page("/").await;
        assert_eq!(brocanth_debug::ACTIVE, cfg!(debug_assertions));
        if !brocanth_debug::ACTIVE {
            assert!(
                !body.contains("debug: GET /"),
                "a release build shows no panel"
            );
            return;
        }
        assert!(body.contains("debug: GET /"), "{body}");
        assert!(body.contains(" · 200 · "), "{body}");
        assert!(body.contains("debug_toolbar.start"), "{body}");
        assert!(body.contains("debug_toolbar.max_statements"), "{body}");
    }
@SQL@}
"#;

const SQL_TEST: &str = r#"
    #[tokio::test]
    async fn the_panel_counts_the_statements_of_the_orm() {
        if brocanth_debug::ACTIVE {
            let body = page("/").await;
            assert!(body.contains(" SQL</summary>"), "{body}");
        }
    }
"#;

const NO_SQL_TEST: &str = r#"
    #[tokio::test]
    async fn without_the_orm_the_panel_says_how_to_count_statements() {
        if brocanth_debug::ACTIVE {
            assert!(page("/").await.contains("brocanth add model"));
        }
    }
"#;

const AGENTS: &str = "\n\
    ## Development panel\n\
    \n\
    In a debug build each HTML page ends with a panel: timing, the ORM's SQL\n\
    statements by shape, the hooks in run order, the settings in effect. Use\n\
    it to find an N+1: one shape issued many times for one page. Fix it with\n\
    an eager load, not by raising `max_statements`.\n\
    \n\
    Never rely on the panel in a test of page content, and never remove the\n\
    `development_only` check: the panel shows every setting to whoever\n\
    loads a page. Run `brocanth remove debug-toolbar debug` before a deploy;\n\
    `brocanth check --deploy` refuses the project until then.\n";
