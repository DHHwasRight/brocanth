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
        Setting::bool("enabled", true, "Count page views. Off installs no hook."),
        Setting::text(
            "site",
            project.name,
            "The site name on every row, and part of the visitor hash.",
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
    ]
}

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
