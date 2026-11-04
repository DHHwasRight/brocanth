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

use brocanth_scaffold::snippet::{configurable, settings_derive, PLUGIN_TRAIT_USE};
use brocanth_scaffold::{Category, Dep, Manifest, Module, Project, Scaffold, Setting, Slot};

/// The robots.txt, as a choice on the `brocanth create` menu.
pub struct Robots;

impl Module for Robots {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "robots",
            "a robots.txt from your settings, served at /robots.txt; closed outside production",
        )
        .category(Category::Routes)
        .outside_runtime()
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .adapts_to("admin", "/admin is disallowed")
        .adapts_to(
            "static",
            "the file says index = true, because the export is what gets published",
        )
        .asked_in("pages", "A robots.txt that says what crawlers may read")
        .krate(Dep::brocanth("brocanth-robots"))
        .after("config")
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
        if !project.has("config") {
            s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
        }
    }
}

/// What the file is written with. With `static` the export is the
/// published site, so the file opens it; otherwise the file is the
/// development one, and a development server is never indexed.
fn index_in_file(project: &Project) -> bool {
    project.has("static")
}

/// The paths every crawler is kept out of, before anybody edits the list.
fn disallowed(project: &Project) -> Vec<&'static str> {
    if project.has("admin") {
        vec!["/admin"]
    } else {
        Vec::new()
    }
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
            &disallowed(project),
            "Paths no crawler may read, each from the root: \"/admin\".",
        ),
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
    let open = !project.has("config") || index_in_file(project);
    let text = if open {
        disallowed(project)
            .into_iter()
            .fold(crate::Robots::new(), crate::Robots::disallow)
    } else {
        crate::Robots::closed()
    };
    text.render().expect("the generator's own paths are valid")
}

/// `src/pages/robots.txt.rs`: the route, and a test that sends a request
/// through the whole application.
fn page_rs(project: &Project) -> String {
    format!(
        r#"{PAGE_RS}
#[cfg(test)]
mod tests {{
    use brocanth::routing::Verb;
    use brocanth_server::Request;

    /// What this project answered the day it was generated. Change the
    /// `[robots]` settings and this line with them.
    const GENERATED: &str = {expected:?};

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
        assert_eq!(std::str::from_utf8(body).expect("utf-8"), GENERATED);
    }}
}}
"#,
        expected = expected(project)
    )
}

fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "robots");
    let disallow = disallowed(project)
        .iter()
        .map(|path| format!("{path:?}.to_string()"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r##"//! The robots.txt, as this application answers it.
//!
//! Rendered once at startup from `Settings`, then served by
//! `src/pages/robots.txt.rs`. A rule path that does not start with `/`, or
//! a sitemap that is not a full URL, stops the build here: a crawler would
//! ignore that line, and the page it was meant to hide would be indexed.
//!
//! `index = false` answers `User-agent: *` / `Disallow: /` and nothing
//! else. That is the answer for development and staging.
//!
//! Only the root `/robots.txt` of a host is read. Under a base path, as on
//! a GitHub Pages project site, crawlers never ask for this file.

use std::sync::OnceLock;

use brocanth::plugin_api::{{Lifecycle, Plugin, PluginError}};
use brocanth::plugin_context::NoContext;
use brocanth_robots::Robots as Text;

/// The rendered text, set once at `configure`.
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
}}

impl Default for Settings {{
    /// Open: a production server with no `[robots]` section is indexed.
    fn default() -> Self {{
        Self {{
            index: true,
            disallow: vec![{disallow}],
            allow: Vec::new(),
            sitemap: None,
        }}
    }}
}}

pub struct Robots {{
    settings: Settings,
}}

impl Robots {{
    /// The builder these settings describe.
    fn text(&self) -> Text {{
        if !self.settings.index {{
            return Text::closed();
        }}
        let mut text = Text::new();
        for path in &self.settings.disallow {{
            text = text.disallow(path.as_str());
        }}
        for path in &self.settings.allow {{
            text = text.allow(path.as_str());
        }}
        if let Some(url) = &self.settings.sitemap {{
            text = text.sitemap(url.as_str());
        }}
        text
    }}
}}

impl Plugin for Robots {{
    type Config = Settings;
    type Provides = NoContext;
    const NAME: &'static str = "robots";

    fn new(config: Settings) -> Self {{
        Self {{ settings: config }}
    }}

    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        let rendered = self
            .text()
            .render()
            .map_err(|e| PluginError::new(format!("[robots] {{e}}")))?;
        let _ = RENDERED.set(rendered);
        Ok(())
    }}
}}

#[cfg(test)]
mod tests {{
    use super::*;

    #[test]
    fn closed_settings_answer_disallow_everything() {{
        let robots = Robots::new(Settings {{
            index: false,
            ..Settings::default()
        }});
        assert_eq!(
            robots.text().render().unwrap(),
            "User-agent: *\nDisallow: /\n"
        );
    }}

    #[test]
    fn open_settings_write_each_rule() {{
        let robots = Robots::new(Settings {{
            disallow: vec!["/private".to_string()],
            allow: vec!["/private/help".to_string()],
            sitemap: Some("https://example.com/sitemap.xml".to_string()),
            ..Settings::default()
        }});
        let text = robots.text().render().unwrap();
        assert!(text.contains("Disallow: /private\n"), "{{text}}");
        assert!(text.contains("Allow: /private/help\n"), "{{text}}");
        assert!(
            text.ends_with("Sitemap: https://example.com/sitemap.xml\n"),
            "{{text}}"
        );
    }}
}}
"##
    )
}
