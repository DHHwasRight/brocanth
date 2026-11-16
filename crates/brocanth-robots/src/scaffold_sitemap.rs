//! `sitemap` -- what `brocanth create` writes when somebody asks for a
//! sitemap.xml.
//!
//! The shape of the `robots` module. The endpoint is a file,
//! `src/pages/sitemap.xml.rs`. A plugin renders the XML once, at startup,
//! from the page table (`crate::pages::handlers()`) and its settings, and
//! the page hands it out. So a page added to `src/pages` is in the sitemap
//! at the next build, and a bad base URL stops the build.
//!
//! **The base URL is a setting of this module.** A sitemap needs the
//! site's absolute address. `robots` reads it from this plugin. `meta` has
//! its own `[meta] base_url`, for the canonical URL and `og:url`, so a
//! project with both holds the address twice, and the note on this setting
//! says to keep the two equal. One owner, a framework-level `[app] url`,
//! needs a framework setting with a flag and an environment variable, and
//! the framework has none yet (`[server] shutdown_grace_secs` has no flag
//! for the same reason).
//!
//! **Unset, a release build stops.** The struct has no address: a debug
//! build (a developer's machine, a test) takes the address `serve` listens
//! on, and a release build refuses to start, because a production sitemap
//! of localhost URLs sends every crawler nowhere, and robots.txt names it.
//!
//! **With `i18n-routing`, a page is listed once per locale**: `/fr/about`
//! and `/en/about`, never `/about`, which answers a redirect. The `i18n`
//! plugin puts its negotiator in the services at `configure`, and this
//! plugin, registered after it, asks it for each page's URLs.
//!
//! `outside_runtime`: the static export writes `/sitemap.xml` as the file
//! `sitemap.xml`.
//!
//! Compiled only under the `scaffold` feature, which is off.

use brocanth_scaffold::snippet::{configurable, module_ref, settings_derive};
use brocanth_scaffold::{Category, Dep, Manifest, Module, Project, Scaffold, Setting, Slot};

/// The base URL a new project starts with: the address `serve` listens on.
pub(crate) const DEFAULT_BASE: &str = "http://localhost:3000";

/// The sitemap.xml, as a choice on the `brocanth create` menu.
pub struct Sitemap;

impl Module for Sitemap {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "sitemap",
            "a sitemap.xml of every page with no parameter, served at /sitemap.xml",
        )
        .category(Category::Routes)
        .outside_runtime()
        .adapts_to(
            "admin",
            "the admin is excluded, at the path src/modules/admin.rs mounts it",
        )
        .adapts_to("jt", "the JSON page /status is excluded")
        .adapts_to("realtime-sse", "the event stream /realtime/events is excluded")
        .adapts_to(
            "static",
            "the note says the export publishes this file, so the base URL must be the published one",
        )
        .adapts_to(
            "i18n-routing",
            "each page is listed once per locale (/fr/about), never at the path that redirects",
        )
        .adapts_to(
            "meta",
            "the note says to keep the base URL equal to `[meta] base_url`",
        )
        .asked_in("pages", "A sitemap.xml that lists every page")
        .krate(Dep::brocanth("brocanth-robots"))
        // The `i18n` plugin puts its negotiator in the services at
        // `configure`; this one reads it there.
        .after("i18n")
        .absent(
            "sitemap",
            "no sitemap.xml; a crawler finds only the pages a link reaches",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/sitemap/mod.rs", plugin_rs(project));
        // The last dot is the extension, so this file answers
        // `/sitemap.xml`, and the static export writes it as `sitemap.xml`.
        s.file("src/pages/sitemap.xml.rs", page_rs(project));
        s.at(Slot::PagesRoutes).line("/sitemap.xml");
        s.at(Slot::PluginsMod).line("pub mod sitemap;");
        configurable(s, project, "sitemap", "Sitemap", settings(project));
    }
}

/// The page paths other modules write that are not pages a visitor reads,
/// as the `exclude` setting starts.
///
/// Page files only: their path is the file's, not an option. The admin is
/// not here: its path is an option in src/modules/admin.rs, so the plugin
/// reads it there ([`admin_exclusion`], docs/module-config.md §9).
fn excluded(project: &Project) -> Vec<&'static str> {
    let mut out = Vec::new();
    if project.has("jt") {
        out.push("/status");
    }
    if project.has("realtime-sse") {
        out.push("/realtime");
    }
    out
}

/// The `[sitemap]` section.
fn settings(project: &Project) -> Vec<Setting> {
    let mut note = if project.has("static") {
        "`export` reads this file, and the export is the site that gets\n\
         published. Set the published address before you publish:\n\
         \"https://example.com\", or \"https://user.github.io/repo\"."
            .to_string()
    } else {
        "This file is the development one. Set the production address in\n\
         production's file or with the environment: unset, a release build\n\
         refuses to start."
            .to_string()
    };
    if project.has("meta") {
        note.push_str("\nKeep it equal to `base_url` under [meta].");
    }
    vec![
        Setting::text(
            "base_url",
            DEFAULT_BASE,
            "The site's address: scheme, host, and base path if any.",
        )
        .note(note)
        .optional(),
        Setting::list(
            "extra",
            &[],
            "More paths to list, such as the pages of a route with a parameter: \"/users/7\".",
        ),
        Setting::list(
            "exclude",
            &excluded(project),
            "Path prefixes kept out of the sitemap: \"/drafts\".",
        )
        .note(if project.has("admin") {
            "The admin is never listed: src/plugins/sitemap/mod.rs reads its path from\n\
             src/modules/admin.rs."
        } else {
            ""
        }),
    ]
}

/// The plugin's `new`. With `admin`, it adds the admin's path to `exclude`,
/// read from src/modules/admin.rs and never copied into the setting: a
/// moved admin needs no edit, and a file that sets `exclude` cannot list
/// the admin (docs/module-config.md §9).
fn admin_exclusion(project: &Project) -> String {
    if !project.has("admin") {
        return "    fn new(config: Settings) -> Self {\n        \
                Self { settings: config }\n    }\n"
            .to_string();
    }
    format!(
        "    fn new(mut config: Settings) -> Self {{\n\
         \x20       // The admin, where src/modules/admin.rs mounts it: no page a\n\
         \x20       // visitor reads.\n\
         \x20       let admin = {}.to_string();\n\
         \x20       if !config.exclude.contains(&admin) {{\n\
         \x20           config.exclude.push(admin);\n\
         \x20       }}\n\
         \x20       Self {{ settings: config }}\n\
         \x20   }}\n",
        module_ref("admin", "mount_at")
    )
}

/// `src/pages/sitemap.xml.rs`, with its test. With `admin`, the test checks
/// that the admin is not listed, wherever it is mounted.
fn page_rs(project: &Project) -> String {
    let index = if project.has("i18n-routing") {
        "        // The index page, once per locale, at the base URL of the\n        \
         // development file.\n        \
         assert!(xml.contains(\"/en</loc>\"), \"{xml}\");\n"
    } else {
        "        // The index page, at the base URL of the development file.\n        \
         assert!(xml.contains(\"/</loc>\"), \"{xml}\");\n"
    };
    let admin = if project.has("admin") {
        "        // The admin, wherever src/modules/admin.rs mounts it.\n        \
         let admin = format!(\"{}</loc>\", crate::modules::admin::MODULE.mount_at);\n        \
         assert!(!xml.contains(&admin), \"{xml}\");\n"
    } else {
        ""
    };
    format!(
        r#"{PAGE_RS}
#[cfg(test)]
mod tests {{
    use brocanth::routing::Verb;
    use brocanth_server::Request;

    #[tokio::test]
    async fn the_application_answers_sitemap_xml_with_its_pages() {{
        let app = crate::app::build().expect("the application builds");
        let response = crate::app::handle(&app, Request::new(Verb::Get, "/sitemap.xml")).await;
        assert_eq!(response.status, 200);
        assert_eq!(
            response.header("content-type"),
            Some("application/xml; charset=utf-8")
        );
        let body = response.body.bytes().expect("a whole body");
        let xml = std::str::from_utf8(body).expect("utf-8");
        assert!(xml.starts_with("<?xml"), "{{xml}}");
{index}        // No route with a parameter, and not this file itself.
        assert!(!xml.contains('{{'), "{{xml}}");
        assert!(!xml.contains("sitemap.xml"), "{{xml}}");
{admin}    }}
}}
"#
    )
}

const PAGE_RS: &str = r#"//! `/sitemap.xml` -- because the file is called `sitemap.xml.rs`.
//!
//! The XML was rendered once, at startup, by `src/plugins/sitemap/mod.rs`.
//! This page hands it out and does nothing else.

use crate::pages::{Ctx, Error, IntoResponse};

/// GET `/sitemap.xml`.
///
/// An hour of cache: the list changes only when the application is built
/// again.
pub fn handle(_ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let body = crate::plugins::sitemap::text().ok_or_else(|| {
        Error::internal("the sitemap plugin is not registered, so there is no sitemap.xml to serve")
    })?;
    Ok((
        body.to_string(),
        200,
        vec![
            (
                "content-type".to_string(),
                "application/xml; charset=utf-8".to_string(),
            ),
            ("cache-control".to_string(), "max-age=3600".to_string()),
        ],
    ))
}
"#;

fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "sitemap");
    let items: Vec<String> = excluded(project)
        .iter()
        .map(|path| format!("{path:?}.to_string()"))
        .collect();
    // rustfmt keeps a `vec!` on one line while its items fit in 60
    // columns, and writes one item per line after that.
    let exclude = if items.is_empty() {
        "Vec::new()".to_string()
    } else if items.join(", ").len() <= 60 {
        format!("vec![{}]", items.join(", "))
    } else {
        let lines: String = items
            .iter()
            .map(|item| format!("                {item},\n"))
            .collect();
        format!("vec![\n{lines}            ]")
    };
    let new = admin_exclusion(project);
    // Every default is the type's own when nothing is excluded: derive it,
    // or clippy's `derivable_impls` flags the hand-written one.
    let (derive, default_impl) = if items.is_empty() {
        (
            derive.replacen("#[derive(Debug", "#[derive(Debug, Default", 1),
            String::new(),
        )
    } else {
        (
            derive,
            format!(
                "\nimpl Default for Settings {{\n    fn default() -> Self {{\n        Self {{\n            \
                 base_url: None,\n            extra: Vec::new(),\n            exclude: {exclude},\n        \
                 }}\n    }}\n}}\n"
            ),
        )
    };
    let (routing_doc, localized) = if project.has("i18n-routing") {
        (
            "//!\n//! With `i18n-routing`, each page is listed once per locale, as the `i18n`\n\
             //! negotiator says: `/fr/about`, never `/about`, which redirects.\n",
            LOCALIZED_ROUTING,
        )
    } else {
        ("", LOCALIZED_PLAIN)
    };
    let routing_test = if project.has("i18n-routing") {
        ROUTING_TEST
    } else {
        ""
    };
    format!(
        r##"//! The sitemap.xml, as this application answers it.
//!
//! Rendered once at startup from the page table and `Settings`, then
//! served by `src/pages/sitemap.xml.rs`. Every GET page with no parameter
//! is listed, except a file (a last segment with a dot, such as
//! `/robots.txt`) and the paths under `exclude`. A route with a parameter
//! is many pages: list the ones that exist in `extra`.
//!
//! A base URL that is not a full `http(s)` address stops the build here: a
//! crawler ignores every relative `<loc>`. An unset one stops a release
//! build, and a debug build takes the development address.
//!
//! To give a page a `<lastmod>`, call `path_modified` in `sitemap` below.
{routing_doc}
use std::sync::OnceLock;

use brocanth::plugin_api::{{Lifecycle, Plugin, PluginError}};
use brocanth::plugin_context::NoContext;
use brocanth::routing::Verb;
use brocanth_robots::sitemap::{{listed, under, Sitemap as Xml}};

/// The address `serve` listens on: the base URL of a debug build that
/// sets none.
const DEVELOPMENT_BASE: &str = {DEFAULT_BASE:?};

/// The rendered XML, set once at `configure`.
static RENDERED: OnceLock<String> = OnceLock::new();

/// The full URL of `/sitemap.xml`, set once at `configure`.
static LOCATION: OnceLock<String> = OnceLock::new();

/// The XML, or `None` when this plugin was never registered.
pub fn text() -> Option<&'static str> {{
    RENDERED.get().map(String::as_str)
}}

/// The full URL of `/sitemap.xml`, for the `Sitemap:` line of a
/// robots.txt. `None` before `configure`.
#[allow(dead_code)]
pub fn location() -> Option<&'static str> {{
    LOCATION.get().map(String::as_str)
}}

/// What this plugin is built from.
///
/// {note}
{derive}
pub struct Settings {{
    /// The site's address: scheme, host, and base path if any.
    pub base_url: Option<String>,
    /// More paths to list.
    pub extra: Vec<String>,
    /// Path prefixes kept out of the sitemap.
    pub exclude: Vec<String>,
}}
{default_impl}
pub struct Sitemap {{
    settings: Settings,
}}

impl Sitemap {{
    /// The base URL. Unset, a debug build takes the development address,
    /// and a release build stops: a production sitemap of localhost URLs
    /// sends every crawler nowhere.
    fn base(&self) -> Result<&str, PluginError> {{
        match self.settings.base_url.as_deref().filter(|b| !b.is_empty()) {{
            Some(base) => Ok(base),
            None if cfg!(debug_assertions) => Ok(DEVELOPMENT_BASE),
            None => Err(PluginError::new(
                "[sitemap] `base_url` is not set, so no <loc> can name this site\n\
                 help: set `base_url` under [sitemap] to the published address, \
                 such as \"https://example.com\"",
            )),
        }}
    }}

    /// The builder these settings and these routes describe. `localized`
    /// gives the URLs one page answers at.
    fn sitemap<'a>(
        &self,
        routes: impl IntoIterator<Item = (Verb, &'a str)>,
        localized: &dyn Fn(&str) -> Vec<String>,
    ) -> Result<Xml, PluginError> {{
        let kept = |path: &str| !self.settings.exclude.iter().any(|p| under(path, p));
        let mut xml = Xml::new(self.base()?);
        for (verb, pattern) in routes {{
            if verb == Verb::Get && listed(pattern) && kept(pattern) {{
                for path in localized(pattern) {{
                    xml = xml.path(path);
                }}
            }}
        }}
        for path in &self.settings.extra {{
            if kept(path) {{
                for path in localized(path) {{
                    xml = xml.path(path);
                }}
            }}
        }}
        Ok(xml)
    }}
}}

impl Plugin for Sitemap {{
    type Config = Settings;
    type Provides = NoContext;
    const NAME: &'static str = "sitemap";

{new}
    fn configure(&mut self, {lc}: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        let routes = crate::pages::handlers()
            .iter()
            .map(|(verb, pattern, _)| (*verb, *pattern));
{localized}        let xml = self.sitemap(routes, &localized)?;
        let text = xml
            .render()
            .map_err(|e| PluginError::new(format!("[sitemap] {{e}}")))?;
        let _ = RENDERED.set(text);
        let _ = LOCATION.set(xml.location());
        Ok(())
    }}
}}

#[cfg(test)]
mod tests {{
    use super::*;

    const ROUTES: &[(Verb, &str)] = &[
        (Verb::Get, "/"),
        (Verb::Get, "/about"),
        (Verb::Post, "/signup"),
        (Verb::Get, "/users/{{id}}"),
        (Verb::Get, "/robots.txt"),
        (Verb::Get, "/drafts/one"),
    ];

    fn rendered(settings: Settings) -> Result<String, String> {{
        let plugin = Sitemap::new(settings);
        plugin
            .sitemap(ROUTES.iter().copied(), &|path| vec![path.to_string()])
            .map_err(|e| e.to_string())?
            .render()
            .map_err(|e| e.to_string())
    }}

    #[test]
    fn pages_with_no_parameter_are_listed_and_the_rest_are_not() {{
        let xml = rendered(Settings {{
            base_url: Some("https://example.com".to_string()),
            extra: vec!["/users/7".to_string()],
            exclude: vec!["/drafts".to_string()],
        }})
        .unwrap();
        for present in ["/</loc>", "/about</loc>", "/users/7</loc>"] {{
            let loc = format!("<loc>https://example.com{{present}}");
            assert!(xml.contains(&loc), "{{loc}} in {{xml}}");
        }}
        for absent in ["/signup", "{{id}}", "robots.txt", "/drafts"] {{
            assert!(!xml.contains(absent), "{{absent}} in {{xml}}");
        }}
    }}

    #[test]
    fn a_relative_base_url_stops_the_build_with_the_fix() {{
        let error = rendered(Settings {{
            base_url: Some("example.com".to_string()),
            ..Settings::default()
        }})
        .unwrap_err();
        assert!(error.contains("help:"), "{{error}}");
    }}
{routing_test}}}
"##,
        lc = if project.has("i18n-routing") {
            "lc"
        } else {
            "_lc"
        },
    )
}

/// The URLs of one page, with no locale segment.
const LOCALIZED_PLAIN: &str = "        let localized = |path: &str| vec![path.to_string()];\n";

/// The URLs of one page, one per locale, from the negotiator the `i18n`
/// plugin put in the services.
const LOCALIZED_ROUTING: &str =
    "        // One URL per locale, from the negotiator the `i18n` plugin put in
        // the services: `/fr/about`, never `/about`, which redirects.
        let i18n = lc
            .services()
            .get::<brocanth_i18n::Negotiator<crate::plugins::i18n::Locale>>()
            .cloned();
        let localized = move |path: &str| {
            i18n.as_ref()
                .map_or_else(|| vec![path.to_string()], |n| n.localized(path))
        };
";

/// With `i18n-routing`: the test that a page is listed once per locale.
const ROUTING_TEST: &str = r#"
    #[test]
    fn each_page_is_listed_once_per_locale_and_never_where_it_redirects() {
        let i18n = brocanth_i18n::Negotiator::<crate::plugins::i18n::Locale>::new(
            brocanth_i18n::NegotiatorConfig::default().with_paths(true),
        );
        let plugin = Sitemap::new(Settings {
            base_url: Some("https://example.com".to_string()),
            ..Settings::default()
        });
        let xml = plugin
            .sitemap(ROUTES.iter().copied(), &|path| i18n.localized(path))
            .unwrap()
            .render()
            .unwrap();
        for present in ["/en/about</loc>", "/fr/about</loc>", "/fr</loc>"] {
            assert!(xml.contains(present), "{present} in {xml}");
        }
        assert!(!xml.contains("example.com/about</loc>"), "{xml}");
    }
"#;
