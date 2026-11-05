//! `analytics` -- what `brocanth create` writes when somebody asks for page
//! view counts.
//!
//! The module is here and not in the CLI because what it writes is a
//! statement about this crate: `Analytics`, the fields of
//! `AnalyticsConfig`, and the store it picks from the application's
//! services.
//!
//! What a project gets:
//!
//! - one registration in `src/plugins/mod.rs`, after the `db` plugin, so
//!   the plugin finds the pool and writes to `PgStore`;
//! - `[analytics]` in the config file, or flags and variables without one;
//! - with `admin`, a tab at `/admin/analytics`. The admin module writes that
//!   line: it is the admin's file.
//!
//! No page, no route, no script and no cookie: the hook reads responses the
//! application already sends.
//!
//! Not `outside_runtime`: a static host never runs this hook, so the static
//! preset refuses the module rather than shipping a counter that counts
//! nothing.
//!
//! Compiled only under the `scaffold` feature, which is off.

use brocanth_scaffold::snippet::{configurable_hooking, Hooking};
use brocanth_scaffold::{Category, Dep, Manifest, Module, Project, Scaffold, Setting, Slot};

/// First-party analytics, as a choice on the `brocanth create` menu.
pub struct PageViews;

impl Module for PageViews {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "analytics",
            "page views counted on the server: no cookie, no script, no third party; rows in your Postgres",
        )
        .category(Category::Middleware)
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .asked_in(
            "operations",
            "Count page views on the server: no cookie, no script, no third party",
        )
        .needs(
            "db",
            "the views are rows in the database the `db` module opens; in memory they would vanish at every restart",
        )
        .krate(
            Dep::brocanth("brocanth-analytics")
                .feature_with("config", "config")
                .feature_with("admin", "admin")
                .note("the page-view hook and its Postgres store"),
        )
        // The plugin reads the pool from the services the `db` plugin
        // fills, so it registers after it.
        .after("db")
        .absent(
            "brocanth-analytics",
            "no page-view counts; nothing is measured, so nothing needs consent",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        configurable_hooking(
            s,
            project,
            Hooking {
                key: "analytics",
                section: "analytics",
                ty: "brocanth_analytics::Analytics",
                config_ty: "brocanth_analytics::AnalyticsConfig",
                prelude: PRELUDE.to_string(),
                adjust: None,
            },
            settings(project),
        );
        s.at(Slot::AgentsSections).block(AGENTS.to_string());
    }
}

fn settings(project: &Project) -> Vec<Setting> {
    vec![
        Setting::bool("enabled", false, "Count page views. Off installs no hook.").note(
            "False here because this file is the development one: your own visits\n\
             are not the audience. Production reads the plugin's default, true,\n\
             unless its own file says otherwise.",
        ),
        Setting::text(
            "site",
            project.name,
            "The site name on the rows this server counts, and part of the visitor hash. A beacon row takes its origin's host.",
        ),
        Setting::list("exclude", &["/admin"], "Paths never counted, each with everything under it."),
        Setting::list(
            "query_allow",
            &["utm_source", "utm_medium", "utm_campaign", "utm_term", "utm_content"],
            "Query parameters kept on the stored path. Every other one is dropped.",
        )
        .note("A token or an email in a URL never reaches the database."),
        Setting::integer(
            "retention_days",
            760,
            "Days a row is kept. 0 keeps rows forever.",
        )
        .note("760 days is 25 months, the longest retention the CNIL's audience-measurement exemption allows."),
        Setting::integer("flush_interval", 10, "Seconds between two writes of the buffer."),
        Setting::bool(
            "respect_gpc",
            false,
            "Skip a request that sends Sec-GPC: 1 or DNT: 1.",
        )
        .note("Nothing stored identifies a visitor, so the signal has nothing to opt out of.\nOn, to honour it anyway."),
        Setting::text(
            "ip_header",
            "fly-client-ip",
            "A header the edge sets to the client address. Unset, the socket's.",
        )
        .optional(),
        Setting::text(
            "country_header",
            "cf-ipcountry",
            "A header the edge sets to a two-letter country code. Unset, no country.",
        )
        .optional(),
        Setting::bool(
            "migrate",
            true,
            "Create the two tables on the first readiness check.",
        )
        .note("Off, for a project that keeps brocanth_analytics::PgStore::SCHEMA in a migration."),
        Setting::text(
            "collector_path",
            "/_a/v",
            "The path that counts page views a static site sends with the `beacon` module.",
        ),
        Setting::list(
            "collector_origins",
            &[],
            "The static sites allowed to send them: \"https://docs.example.com\". Empty, no collector.",
        )
        .note("A beacon from any other origin is answered 204 and not counted."),
    ]
}

/// The snippet a static site carries, as a choice on the menu.
///
/// Its own module and not a child of `analytics`, because the two live in
/// different projects. The static site is exported to files and cannot hold
/// `analytics`, which needs a running server and a database; a child cannot
/// be chosen without its parent, so a child would be refused by the very
/// preset it exists for. `beacon` is `outside_runtime`: what it adds runs
/// when the export renders a page, and in the visitor's browser.
pub struct StaticBeacon;

impl Module for StaticBeacon {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "beacon",
            "a few lines of script that report each page view to an analytics server; no cookie",
        )
        .category(Category::Middleware)
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .asked_in(
            "operations",
            "Report page views of these pages to a brocanth server with `analytics`",
        )
        .conflicts(
            "analytics",
            "both would count one page: the hook on the server, and the beacon in the browser",
        )
        .krate(
            Dep::brocanth("brocanth-analytics")
                .no_default_features()
                .feature("runtime")
                .feature_with("config", "config")
                .note("the beacon snippet, without the store"),
        )
        // No `absent` line: its crate is `analytics`'s too, so "not here"
        // would be untrue in a project that counts on the server.
        .outside_runtime()
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        configurable_hooking(
            s,
            project,
            Hooking {
                key: "beacon",
                section: "beacon",
                ty: "brocanth_analytics::Beacon",
                config_ty: "brocanth_analytics::BeaconConfig",
                prelude: BEACON_PRELUDE.to_string(),
                adjust: None,
            },
            vec![
                Setting::text(
                    "collector",
                    "https://app.example.com/_a/v",
                    "The analytics server's collector URL. Unset, no page carries the snippet.",
                )
                .optional()
                .note("That server lists this site in `[analytics] collector_origins`."),
                Setting::bool(
                    "respect_gpc",
                    false,
                    "Send nothing from a browser with Global Privacy Control or Do Not Track on.",
                ),
            ],
        );
        s.at(Slot::AgentsSections).block(BEACON_AGENTS.to_string());
    }
}

const BEACON_PRELUDE: &str = r#"
        // -- brocanth-analytics: the beacon ---------------------------------
        //
        // A response hook that puts a few lines of script before `</body>`
        // of every HTML page, the exported files included. The script tells
        // `[beacon] collector` that a page was read: no cookie, nothing
        // stored in the browser.
"#;

const BEACON_AGENTS: &str = "\n\
    ## The beacon\n\
    \n\
    `brocanth_analytics::Beacon` puts a short script in every HTML page\n\
    once `[beacon] collector` names an analytics server. Do not copy the\n\
    script into a template: the hook adds it, and the export writes it\n\
    into each file. The server must list this site's origin in\n\
    `[analytics] collector_origins`, or every beacon is ignored.\n";

/// The comment above the registration.
const PRELUDE: &str = r#"
        // -- brocanth-analytics: page views ---------------------------------
        //
        // A request hook, after dispatch: a GET answered with HTML is one
        // page view. It stores no address, no user agent and no cookie; the
        // visitor is a hash whose salt is deleted when the day ends. The
        // views go in two tables of the `[db]` database, written every
        // `flush_interval` seconds, never during a request.
"#;

/// What the generated `AGENTS.md` says about analytics.
const AGENTS: &str = "\n\
    ## Analytics\n\
    \n\
    `brocanth_analytics::Analytics` is registered in `src/plugins/mod.rs`.\n\
    It counts every GET answered with `text/html`, except paths under\n\
    `[analytics] exclude`, bots and prefetches. Nothing to add to a page:\n\
    no script, no cookie, no banner.\n\
    \n\
    Do not put personal data in a URL. The path and the allowed query\n\
    parameters are stored; an email-shaped segment is replaced, but a name\n\
    or an account number is not. Do not add a column to\n\
    `brocanth_analytics_views` that identifies a person.\n\
    \n\
    With the admin, the numbers are at `/admin/analytics`, behind the\n\
    admin's policy like every tab.\n";
