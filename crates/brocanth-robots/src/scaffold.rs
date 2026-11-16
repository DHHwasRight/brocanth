//! `robots` -- what `brocanth create` writes when somebody asks for a
//! robots.txt.
//!
//! The same shape as the openapi module, for the same reasons. A route in a
//! generated project is a file, so the endpoint is `src/pages/robots.txt.rs`.
//! The text is rendered once, at startup, by a plugin that holds the
//! settings, and the page hands it out. [`Robots::render`] runs at
//! `configure`, so a rule path a crawler would ignore stops the build
//! instead of reaching a crawler.
//!
//! `outside_runtime`: the static export asks every route for its answer and
//! writes `/robots.txt` as the file `robots.txt`, so a static site takes
//! this module too.
//!
//! Compiled only under the `scaffold` feature, which is off.
//!
//! [`Robots::render`]: crate::Robots::render

use brocanth_scaffold::snippet::{configurable, module_ref, settings_derive};
use brocanth_scaffold::{
    Category, ConfigSource, Dep, Manifest, Module, Mount, MountRole, Opt, Project, Scaffold,
    Setting, Slot,
};

pub use crate::scaffold_sitemap::Sitemap;

/// The robots.txt, as a choice on the `brocanth create` menu.
pub struct Robots;

impl Module for Robots {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "robots",
            "a robots.txt from your settings, served at /robots.txt; closed outside production",
        )
        .category(Category::Routes)
        .uses::<ConfigSource>(
            "the per-crawler groups are records, written to the file; the development file opens the site",
        )
        .outside_runtime()
        .uses::<Mount>(
            "every `Backoffice` mount, such as the admin, is disallowed, at the path its module file mounts it",
        )
        // A choice and not a fact: `Disallow: /admin` also tells every
        // reader of robots.txt where the backoffice is.
        .option(Opt::bool(
            "disallow_backoffice",
            true,
            "Disallow every Backoffice mount, such as the admin. The rule also tells every reader of robots.txt where the backoffice is.",
        ))
        .adapts_to(
            "sitemap",
            "the Sitemap: line names /sitemap.xml at the sitemap's base URL, unless `sitemap` is set",
        )
        .adapts_to(
            "static",
            "the file says index = true, because the export is what gets published",
        )
        .adapts_to(
            "i18n-routing",
            "each rule path is also written under each locale segment: /drafts and /fr/drafts",
        )
        .asked_in("pages", "A robots.txt that says what crawlers may read")
        .krate(Dep::brocanth("brocanth-robots"))
        // The `i18n` plugin puts its negotiator in the services at
        // `configure`; this one reads it there.
        .after("i18n")
        .absent(
            "brocanth-robots",
            "no robots.txt; every crawler reads every page, and so does every staging server's",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/robots/mod.rs", plugin_rs(project));
        // The endpoint. The last dot is the extension, so this file answers
        // `/robots.txt`, and the static export writes it as `robots.txt`.
        s.file("src/pages/robots.txt.rs", page_rs(project));
        s.at(Slot::PagesRoutes).line("/robots.txt");
        s.at(Slot::PluginsMod).line("pub mod robots;");
        configurable(s, project, "robots", "Robots", settings(project));
        if project.provider::<ConfigSource>().is_some() {
            // Groups are records, so they are file-only, as every list of
            // records is. Written commented out: an example, not a choice.
            s.at(Slot::ConfigSections).block(GROUPS_TOML.to_string());
        }
    }
}

/// The per-crawler groups, as the file holds them. Commented out, because
/// which crawlers to name is the site owner's decision.
const GROUPS_TOML: &str = r#"
# More groups, for named crawlers, written after the `*` group. One table
# per group. A crawler that matches a named group follows that group only,
# not the `*` rules. robots.txt is a request: a crawler may ignore it.
# Example: ask crawlers that collect training data to read nothing.
# [[robots.groups]]
# agents = ["GPTBot", "CCBot", "Google-Extended", "ClaudeBot"]
# disallow = ["/"]
"#;

/// What the file is written with. With `static` the export is the
/// published site, so the file opens it; otherwise the file is the
/// development one, and a development server is never indexed.
fn index_in_file(project: &Project) -> bool {
    project.has("static")
}

/// A stand-in path for the `i`th `Backoffice` mount in the expected text.
/// The test reads the real path from the mount's module file.
fn placeholder(i: usize) -> String {
    format!("/backoffice-mount-{i}")
}

/// The paths a fresh project keeps every crawler out of: each `Backoffice`
/// mount's, through `disallow_backoffice`, and not through the `disallow`
/// setting. A stand-in each, for the expected text.
fn disallowed(project: &Project) -> Vec<String> {
    (0..project.mounts_with(MountRole::Backoffice).len())
        .map(placeholder)
        .collect()
}

/// Each `Backoffice` mount's path, behind `disallow_backoffice` in
/// src/modules/robots.rs.
fn admin_rule(project: &Project) -> String {
    let backoffice = project.mounts_with(MountRole::Backoffice);
    if backoffice.is_empty() {
        return String::new();
    }
    let mut out = format!(
        "        // The backoffice, such as the admin, where its module file mounts\n\
         \x20       // it. Turn it off with `disallow_backoffice` in src/modules/robots.rs.\n\
         \x20       if {} {{\n",
        module_ref("robots", "disallow_backoffice"),
    );
    for (key, field) in backoffice {
        out.push_str(&format!(
            "            text = text.disallow({});\n",
            module_ref(key, field)
        ));
    }
    out.push_str("        }\n");
    out
}

/// The `[robots]` section.
fn settings(project: &Project) -> Vec<Setting> {
    vec![
        Setting::bool(
            "index",
            index_in_file(project),
            "Let crawlers read this site. False answers `Disallow: /` and nothing else.",
        )
        .note(if index_in_file(project) {
            "True here because `export` reads this file, and the export is the\n\
             site that gets published. A preview build that must stay out of\n\
             search sets it false."
        } else {
            "False here because this file is the development one. Production reads\n\
             the plugin's default, true, unless its own file says otherwise. Set it\n\
             false on staging, or a search engine lists staging beside production."
        }),
        Setting::list(
            "disallow",
            &[],
            "Paths no crawler may read, each from the root: \"/admin\".",
        )
        .note(if project.has("i18n-routing") {
            "Each path is also written under each locale segment: \"/drafts\" keeps\n\
             crawlers out of /fr/drafts too."
        } else {
            ""
        }),
        Setting::list(
            "allow",
            &[],
            "Paths a crawler may read under a disallowed one: \"/admin/help\".",
        ),
        Setting::text(
            "sitemap",
            "https://example.com/sitemap.xml",
            "Where the sitemap is, as a full URL.",
        )
        .optional(),
    ]
}

const PAGE_RS: &str = r#"//! `/robots.txt` -- because the file is called `robots.txt.rs`.
//!
//! The text was rendered once, at startup, by `src/plugins/robots/mod.rs`.
//! This page hands it out and does nothing else.

use crate::pages::{Ctx, Error, IntoResponse};

/// GET `/robots.txt`.
///
/// An hour of cache: crawlers keep the file for a day anyway, and an hour
/// is how long a changed rule takes to reach one that asked just before.
pub fn handle(_ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let body = crate::plugins::robots::text().ok_or_else(|| {
        Error::internal("the robots plugin is not registered, so there is no robots.txt to serve")
    })?;
    Ok((
        body.to_string(),
        200,
        vec![
            (
                "content-type".to_string(),
                "text/plain; charset=utf-8".to_string(),
            ),
            ("cache-control".to_string(), "max-age=3600".to_string()),
        ],
    ))
}
"#;

/// What a fresh project answers before anybody edits the settings: the
/// file's values with `config`, the plugin's default without it.
fn expected(project: &Project) -> String {
    let open = project.provider::<ConfigSource>().is_none() || index_in_file(project);
    let text = if open {
        let text = disallowed(project)
            .iter()
            .fold(crate::Robots::new(), |text, path| text.disallow(path));
        if project.has("sitemap") {
            let base = crate::scaffold_sitemap::DEFAULT_BASE;
            text.sitemap(format!("{base}/sitemap.xml"))
        } else {
            text
        }
    } else {
        crate::Robots::closed()
    };
    text.render().expect("the generator's own paths are valid")
}

/// The text as a literal after `const GENERATED: &str =`, on the same line
/// when it fits and on the next when it does not, as rustfmt writes it.
fn generated_literal(text: &str) -> String {
    let literal = format!("{text:?}");
    if "    const GENERATED: &str = ".len() + literal.len() < 100 {
        format!(" {literal}")
    } else {
        format!("\n        {literal}")
    }
}

/// The test's expected text. A literal, unless it names a `Backoffice`
/// mount: then a function that reads each path from its module file, so a
/// moved admin does not fail the project's own test
/// (docs/module-config.md §9).
fn generated_item(project: &Project) -> (String, &'static str) {
    let text = expected(project);
    let backoffice = project.mounts_with(MountRole::Backoffice);
    let rules: Vec<String> = (0..backoffice.len())
        .map(|i| format!("Disallow: {}\n", placeholder(i)))
        .filter(|rule| text.contains(rule.as_str()))
        .collect();
    if rules.is_empty() {
        let item = format!(
            "    /// What this project answered the day it was generated. Change the\n\
             \x20   /// `[robots]` settings and this line with them.\n\
             \x20   const GENERATED: &str ={};\n",
            generated_literal(&text)
        );
        return (item, "GENERATED");
    }
    let mut template = text.replace('{', "{{").replace('}', "}}");
    for rule in &rules {
        template = template.replace(rule.as_str(), "Disallow: {}\n");
    }
    // No comma after the last argument, as rustfmt leaves a `format!`.
    let paths = backoffice
        .iter()
        .map(|(key, field)| format!("\n            {}", module_ref(key, field)))
        .collect::<Vec<_>>()
        .join(",");
    let item = format!(
        "    /// What this project answered the day it was generated, with the\n\
         \x20   /// backoffice where its module file mounts it. Change the `[robots]`\n\
         \x20   /// settings and this text with them.\n\
         \x20   fn generated() -> String {{\n\
         \x20       format!(\n\
         \x20           {template:?},{paths}\n\
         \x20       )\n\
         \x20   }}\n"
    );
    (item, "generated()")
}

/// `src/pages/robots.txt.rs`: the route, and a test that sends a request
/// through the whole application.
fn page_rs(project: &Project) -> String {
    let (generated, call) = generated_item(project);
    format!(
        r#"{PAGE_RS}
#[cfg(test)]
mod tests {{
    use brocanth::routing::Verb;
    use brocanth_server::Request;

{generated}
    #[tokio::test]
    async fn the_application_answers_robots_txt_as_text() {{
        let app = crate::app::build().expect("the application builds");
        let response = crate::app::handle(&app, Request::new(Verb::Get, "/robots.txt")).await;
        assert_eq!(response.status, 200);
        assert_eq!(
            response.header("content-type"),
            Some("text/plain; charset=utf-8")
        );
        let body = response.body.bytes().expect("a whole body");
        assert_eq!(std::str::from_utf8(body).expect("utf-8"), {call});
    }}
}}
"#
    )
}

const SITEMAP_FROM_SETTING: &str = "        if let Some(url) = &self.settings.sitemap {
            text = text.sitemap(url.as_str());
        }";

/// With the `sitemap` module: the setting wins, and the sitemap plugin's
/// own URL is the default.
const SITEMAP_FROM_PLUGIN: &str =
    "        // Unset, the line names the sitemap this application serves.
        let own = self.settings.sitemap.as_deref();
        let sitemap = own.or(crate::plugins::sitemap::location());
        if let Some(url) = sitemap {
            text = text.sitemap(url);
        }";

const RENDER_AT_CONFIGURE: &str =
    "    fn configure(&mut self, @LC@: &mut Lifecycle<'_>) -> Result<(), PluginError> {
@LOCALIZED@        let _ = RENDERED.set(self.rendered(&localized)?);
        Ok(())
    }";

/// After `configure`, because the sitemap plugin sets its URL there.
const RENDER_AFTER_CONFIG: &str =
    "    // After `configure`: the sitemap plugin sets its URL there, and every
    // plugin finishes one stage before any plugin starts the next.
    fn after_config(&mut self, @LC@: &mut Lifecycle<'_>) -> Result<(), PluginError> {
@LOCALIZED@        let _ = RENDERED.set(self.rendered(&localized)?);
        Ok(())
    }";

/// A rule path, with no locale segment.
const LOCALIZED_PLAIN: &str = "        let localized = |path: &str| vec![path.to_string()];\n";

/// A rule path under each locale segment, from the negotiator the `i18n`
/// plugin put in the services.
const LOCALIZED_ROUTING: &str =
    "        // Each rule path under each locale segment too, from the negotiator
        // the `i18n` plugin put in the services: /drafts and /fr/drafts.
        let i18n = lc
            .services()
            .get::<brocanth_i18n::Negotiator<crate::plugins::i18n::Locale>>()
            .cloned();
        let localized = move |path: &str| {
            i18n.as_ref()
                .map_or_else(|| vec![path.to_string()], |n| n.localized(path))
        };
";

fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "robots");
    let group_derive = if project.provider::<ConfigSource>().is_some() {
        "#[derive(Debug, Clone, Default, serde::Deserialize)]\n#[serde(default, deny_unknown_fields)]"
    } else {
        "#[derive(Debug, Clone, Default)]"
    };
    let sitemap_line = if project.has("sitemap") {
        SITEMAP_FROM_PLUGIN
    } else {
        SITEMAP_FROM_SETTING
    };
    let routing = project.has("i18n-routing");
    let stage = if project.has("sitemap") {
        RENDER_AFTER_CONFIG
    } else {
        RENDER_AT_CONFIGURE
    }
    .replace("@LC@", if routing { "lc" } else { "_lc" })
    .replace(
        "@LOCALIZED@",
        if routing {
            LOCALIZED_ROUTING
        } else {
            LOCALIZED_PLAIN
        },
    );
    let admin = admin_rule(project);
    format!(
        r##"//! The robots.txt, as this application answers it.
//!
//! Rendered once at startup from `Settings`, then served by
//! `src/pages/robots.txt.rs`. A rule path that does not start with `/`, or
//! a sitemap that is not a full URL, stops the build here: a crawler would
//! ignore that line, and the page it was meant to hide would be indexed.
//!
//! `index = false` answers `User-agent: *` / `Disallow: /` and nothing
//! else. That is the answer for development and staging. The rules are
//! still checked then, so a bad one stops the development server, not the
//! first production one.
//!
//! Only the root `/robots.txt` of a host is read. Under a base path, as on
//! a GitHub Pages project site, crawlers never ask for this file.

use std::sync::OnceLock;

use brocanth::plugin_api::{{Lifecycle, Plugin, PluginError}};
use brocanth::plugin_context::NoContext;
use brocanth_robots::{{Group, Robots as Text}};

/// The rendered text, set once at startup.
static RENDERED: OnceLock<String> = OnceLock::new();

/// The text, or `None` when this plugin was never registered.
pub fn text() -> Option<&'static str> {{
    RENDERED.get().map(String::as_str)
}}

/// What this plugin is built from.
///
/// {note}
{derive}
pub struct Settings {{
    /// Let crawlers read this site.
    pub index: bool,
    /// Paths no crawler may read.
    pub disallow: Vec<String>,
    /// Paths a crawler may read under a disallowed one.
    pub allow: Vec<String>,
    /// Where the sitemap is, as a full URL.
    pub sitemap: Option<String>,
    /// Groups for named crawlers, written after the `*` group. File-only:
    /// `[[robots.groups]]`.
    pub groups: Vec<CrawlerGroup>,
}}

/// One `[[robots.groups]]` table.
{group_derive}
pub struct CrawlerGroup {{
    /// The crawlers this group names, by product token: "GPTBot".
    pub agents: Vec<String>,
    /// Paths these crawlers may read under a disallowed one.
    pub allow: Vec<String>,
    /// Paths these crawlers may not read.
    pub disallow: Vec<String>,
}}

impl Default for Settings {{
    /// Open: a production server with no `[robots]` section is indexed.
    fn default() -> Self {{
        Self {{
            index: true,
            disallow: Vec::new(),
            allow: Vec::new(),
            sitemap: None,
            groups: Vec::new(),
        }}
    }}
}}

pub struct Robots {{
    settings: Settings,
}}

/// A rule path, then each other URL `localized` gives for it.
fn rule_paths(path: &str, localized: &dyn Fn(&str) -> Vec<String>) -> Vec<String> {{
    let mut out = vec![path.to_string()];
    for other in localized(path) {{
        if !out.contains(&other) {{
            out.push(other);
        }}
    }}
    out
}}

impl Robots {{
    /// The builder these settings describe, as if `index` were true.
    /// `localized` gives the other URLs a rule path covers.
    fn open(&self, localized: &dyn Fn(&str) -> Vec<String>) -> Result<Text, PluginError> {{
        let mut text = Text::new();
{admin}        for path in &self.settings.disallow {{
            for path in rule_paths(path, localized) {{
                text = text.disallow(path);
            }}
        }}
        for path in &self.settings.allow {{
            for path in rule_paths(path, localized) {{
                text = text.allow(path);
            }}
        }}
        for named in &self.settings.groups {{
            let Some((first, rest)) = named.agents.split_first() else {{
                return Err(PluginError::new(
                    "[robots] a [[robots.groups]] table names no crawler\n\
                     help: give it agents = [\"GPTBot\"], or delete the table",
                ));
            }};
            let mut group = Group::new(first.as_str());
            for agent in rest {{
                group = group.agent(agent.as_str());
            }}
            for path in &named.disallow {{
                for path in rule_paths(path, localized) {{
                    group = group.disallow(path);
                }}
            }}
            for path in &named.allow {{
                for path in rule_paths(path, localized) {{
                    group = group.allow(path);
                }}
            }}
            text = text.group(group);
        }}
{sitemap_line}
        Ok(text)
    }}

    /// The text these settings describe, or why it cannot be served. The
    /// open text is rendered even when `index` is false, so a bad rule
    /// stops the development server too.
    fn rendered(&self, localized: &dyn Fn(&str) -> Vec<String>) -> Result<String, PluginError> {{
        let failed = |e: brocanth_robots::RobotsError| PluginError::new(format!("[robots] {{e}}"));
        let open = self.open(localized)?.render().map_err(failed)?;
        if !self.settings.index {{
            return Text::closed().render().map_err(failed);
        }}
        Ok(open)
    }}
}}

impl Plugin for Robots {{
    type Config = Settings;
    type Provides = NoContext;
    const NAME: &'static str = "robots";

    fn new(config: Settings) -> Self {{
        Self {{ settings: config }}
    }}

{stage}
}}

#[cfg(test)]
mod tests {{
    use super::*;

    fn rendered(robots: &Robots) -> Result<String, PluginError> {{
        robots.rendered(&|path| vec![path.to_string()])
    }}

    fn ai_crawlers() -> CrawlerGroup {{
        CrawlerGroup {{
            agents: vec!["GPTBot".to_string(), "CCBot".to_string()],
            disallow: vec!["/".to_string()],
            ..CrawlerGroup::default()
        }}
    }}

    #[test]
    fn closed_settings_answer_disallow_everything_and_nothing_else() {{
        let robots = Robots::new(Settings {{
            index: false,
            groups: vec![ai_crawlers()],
            ..Settings::default()
        }});
        assert_eq!(rendered(&robots).unwrap(), "User-agent: *\nDisallow: /\n");
    }}

    #[test]
    fn a_bad_rule_stops_a_closed_site_too() {{
        let robots = Robots::new(Settings {{
            index: false,
            disallow: vec!["private".to_string()],
            ..Settings::default()
        }});
        let error = rendered(&robots).unwrap_err().to_string();
        assert!(error.contains("help:"), "{{error}}");
    }}

    #[test]
    fn open_settings_write_each_rule_then_each_group() {{
        let robots = Robots::new(Settings {{
            disallow: vec!["/private".to_string()],
            allow: vec!["/private/help".to_string()],
            sitemap: Some("https://example.com/sitemap.xml".to_string()),
            groups: vec![ai_crawlers()],
            ..Settings::default()
        }});
        let text = rendered(&robots).unwrap();
        assert!(text.contains("Disallow: /private\n"), "{{text}}");
        assert!(text.contains("Allow: /private/help\n"), "{{text}}");
        assert!(
            text.contains("\nUser-agent: GPTBot\nUser-agent: CCBot\nDisallow: /\n"),
            "{{text}}"
        );
        assert!(
            text.ends_with("Sitemap: https://example.com/sitemap.xml\n"),
            "{{text}}"
        );
    }}

    #[test]
    fn a_group_with_no_crawler_stops_the_build() {{
        let robots = Robots::new(Settings {{
            groups: vec![CrawlerGroup::default()],
            ..Settings::default()
        }});
        let error = rendered(&robots).unwrap_err().to_string();
        assert!(error.contains("help:"), "{{error}}");
    }}
}}
"##
    )
}
