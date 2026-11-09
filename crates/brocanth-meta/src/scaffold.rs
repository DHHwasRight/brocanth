//! `meta`, as a choice on the `brocanth create` menu.
//!
//! What a project gets:
//!
//! - `src/plugins/meta/mod.rs`: the `[meta]` settings and the plugin that
//!   puts a [`Site`](crate::Site) on the context, with its tests.
//! - `ctx.meta()` and `ctx.head()` in `src/pages/mod.rs`. A page sets its
//!   own title with the first; a layout that declares `meta: &Head` gets
//!   the second from `html!`.
//! - With `rsx`, the root layout declares `meta` and writes the title, the
//!   language and the tags. The `rsx` module writes that layout, because
//!   one file has one writer.
//!
//! It is `outside_runtime`: the static export renders every page through
//! the layouts, and a static site needs share tags as much as a server.
//!
//! Compiled only under the `scaffold` feature.

use brocanth_scaffold::snippet::{configurable, settings_derive, PLUGIN_TRAIT_USE};
use brocanth_scaffold::{Category, Dep, Manifest, Module, Project, Scaffold, Setting, Slot};

/// A title, a description, a canonical URL and share tags for each page.
pub struct PageMeta;

impl Module for PageMeta {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "meta",
            "a title, a description, a canonical URL and Open Graph tags for each page",
        )
        .category(Category::Renderer)
        .category(Category::Enhancer)
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .adapts_to("i18n", "`<html lang>` and `og:locale` follow the request's locale")
        .adapts_to(
            "i18n-routing",
            "a page lists its other locales with hreflang, and its canonical URL keeps the locale segment",
        )
        .adapts_to(
            "rsx",
            "a test reads the tags the root layout writes on the home page",
        )
        // The export renders every page through the layouts, so a static
        // site takes it.
        .outside_runtime()
        .asked_in("pages", "A title, a description and share tags for each page")
        .krate(Dep::brocanth("brocanth-meta"))
        .absent(
            "brocanth-meta",
            "every page has the title `pages::TITLE`, and no description, canonical URL or share tags",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/meta/mod.rs", plugin_rs(project));
        s.at(Slot::PluginsMod).line("pub mod meta;");
        s.at(Slot::PagesItems).block(pages_items(project));
        configurable(s, project, "meta", "Meta", settings(project));
        if !project.has("config") {
            s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
        }
        let mut agents = AGENTS.to_string();
        if project.has("rsx") {
            agents.push_str(AGENTS_RSX);
        }
        s.at(Slot::AgentsSections).block(agents);
    }
}

/// The template a fresh project titles its pages with.
fn template(project: &Project) -> String {
    format!("%s | {}", project.title)
}

/// The `[meta]` section.
fn settings(project: &Project) -> Vec<Setting> {
    vec![
        Setting::text(
            "name",
            project.title,
            "The site's name: the title of a page with none, and og:site_name.",
        ),
        Setting::text(
            "title_template",
            &template(project),
            "How a page's title becomes the document's. `%s` is the page's own title.",
        )
        .optional(),
        Setting::text(
            "base_url",
            "https://example.com",
            "The site's address, scheme and host. A canonical URL and og:url need it.",
        )
        .note("Unset, a page has no canonical URL: a relative one is worse than none.")
        .optional(),
        Setting::text(
            "description",
            "",
            "The description of a page that gives none.",
        )
        .optional(),
        Setting::text(
            "image",
            "/og.png",
            "The og:image of a page that gives none: a path or a full URL.",
        )
        .optional(),
        Setting::text(
            "twitter",
            "@example",
            "The site's account, for twitter:site.",
        )
        .optional(),
    ]
}

fn pages_items(project: &Project) -> String {
    let mut locale = String::new();
    if project.has("i18n") {
        locale.push_str(HEAD_LANG);
    }
    if project.has("i18n-routing") {
        locale.push_str(HEAD_ALTERNATES);
    }
    PAGES_ITEMS.replace("@LOCALE@", &locale)
}

const PAGES_ITEMS: &str = r#"/// What a page says about itself, and the tags a layout writes for it.
/// Written by the `meta` module; the settings are in
/// `src/plugins/meta/mod.rs`.
#[allow(dead_code)]
impl Ctx {
    /// This page's metadata. Set it before `html!`, which renders the
    /// layout that reads it: `ctx.meta().title("About");`.
    pub fn meta(&mut self) -> &mut brocanth_meta::Meta {
        if self.state.get::<brocanth_meta::Meta>().is_none() {
            self.state.insert(brocanth_meta::Meta::default());
        }
        self.state
            .get_mut::<brocanth_meta::Meta>()
            .expect("inserted above")
    }

    /// The page's metadata resolved against the site's: what a layout that
    /// declares `meta: &Head` is given. Its `Display` is the tags.
    pub fn head(&self) -> brocanth_meta::Head {
        let fallback = brocanth_meta::Site::default();
        let site = self
            .provided::<brocanth_meta::Site, _>()
            .unwrap_or(&fallback);
        let page = self.state.get::<brocanth_meta::Meta>();
        #[allow(unused_mut)]
        let mut head = brocanth_meta::Head::new(site, page).path(&self.path);
@LOCALE@        head
    }
}
"#;

const HEAD_LANG: &str = r#"        if let Some(current) = self.state.get::<brocanth_i18n::Current>() {
            head = head.lang(current.tag());
        }
"#;

const HEAD_ALTERNATES: &str = r#"        // One URL per locale: the canonical one keeps the segment, and each
        // other locale is an alternate.
        if let Some(locale) = self.state.get::<brocanth_i18n::PathLocale>() {
            head = head.path(&locale.href(&self.path));
            for (tag, href) in locale.alternates(&self.path) {
                head = head.alternate(tag, &href);
            }
        }
"#;

fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "meta");
    let app_test = if project.has("rsx") {
        app_test(project)
    } else {
        String::new()
    };
    PLUGIN_RS
        .replace("@DERIVE@", &derive)
        .replace("@NOTE@", &note)
        .replace("@NAME@", &format!("{:?}", project.title))
        .replace("@TEMPLATE@", &format!("{:?}", template(project)))
        .replace(
            "@TITLED@",
            &format!("{:?}", template(project).replace("%s", "About")),
        )
        .replace("@APP_TEST@", &app_test)
}

fn app_test(project: &Project) -> String {
    // With `i18n-routing` the home page answers under its locale segment.
    let (path, canonical) = if project.has("i18n-routing") {
        ("/en", r#"<link rel="alternate" hreflang="fr" href="/fr">"#)
    } else {
        ("/", r#"<meta property="og:type" content="website">"#)
    };
    APP_TEST
        .replace("@PATH@", path)
        .replace("@TAG@", canonical)
        .replace("@NAME@", project.title)
}

const PLUGIN_RS: &str = r#"//! Written by `brocanth`. What every page says about itself in `<head>`.
//!
//! [`Meta`] holds the `[meta]` settings and puts a `brocanth_meta::Site` on
//! the context. A page sets its own title, description, image or canonical
//! URL with `ctx.meta()`, before it renders:
//!
//! ```ignore
//! ctx.meta().title("About").description("Who writes this site.");
//! html!(ctx, views::about::About { .. })
//! ```
//!
//! A layout that declares `meta: &Head` is given `ctx.head()`: the page's
//! values, and the site's for what the page left out. Its `Display` is the
//! tags, every value escaped.

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::{ContextBuilder, ProvidesContext};
use brocanth_meta::Site;

/// What this plugin is built from.
///
/// @NOTE@
@DERIVE@
pub struct Settings {
    /// The site's name: the title of a page with none, and `og:site_name`.
    pub name: String,
    /// How a page's title becomes the document's: `%s` is the page's own.
    pub title_template: Option<String>,
    /// The site's address, scheme and host. A canonical URL needs it.
    pub base_url: Option<String>,
    /// The description of a page that gives none.
    pub description: Option<String>,
    /// The `og:image` of a page that gives none.
    pub image: Option<String>,
    /// The site's account, for `twitter:site`: `@blog`.
    pub twitter: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            name: @NAME@.to_string(),
            title_template: Some(@TEMPLATE@.to_string()),
            base_url: None,
            description: None,
            image: None,
            twitter: None,
        }
    }
}

/// Puts the site's metadata on the context, where `ctx.head()` reads it.
pub struct Meta {
    site: Site,
}

impl Plugin for Meta {
    type Config = Settings;
    type Provides = ProvidesContext<Site>;
    const NAME: &'static str = "meta";

    fn new(config: Settings) -> Self {
        let mut site = Site::new(config.name);
        let set = |value: Option<String>| value.filter(|v| !v.is_empty());
        if let Some(template) = set(config.title_template) {
            site = site.title_template(template);
        }
        if let Some(url) = set(config.base_url) {
            site = site.base_url(url);
        }
        if let Some(text) = set(config.description) {
            site = site.description(text);
        }
        if let Some(url) = set(config.image) {
            site = site.image(url);
        }
        if let Some(handle) = set(config.twitter) {
            site = site.twitter(handle);
        }
        Self { site }
    }

    /// A template with no `%s` or a base URL with no scheme stops the
    /// start: every page would carry the wrong tag.
    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        self.site
            .check()
            .map_err(|why| PluginError::new(format!("[meta] {why}")))
    }

    fn provide(&self, cx: &mut ContextBuilder) {
        cx.insert(self.site.clone());
    }
}

#[cfg(test)]
mod tests {
    use brocanth::routing::{Params, Verb};

    use super::*;
    use crate::pages::Ctx;

    #[test]
    fn a_page_title_goes_through_the_template() {
        let mut ctx = Ctx::new(Verb::Get, "/about", Params::empty());
        let site = Meta::new(Settings::default()).site;
        ctx.meta().title("About").description("Who writes this.");
        let head = brocanth_meta::Head::new(&site, ctx.hooked::<brocanth_meta::Meta>());
        assert_eq!(head.title(), @TITLED@);
        assert!(head.to_string().contains("content=\"Who writes this.\""));
    }

    #[test]
    fn a_template_with_no_hole_stops_the_start() {
        let meta = Meta::new(Settings {
            title_template: Some("Same title everywhere".to_string()),
            ..Settings::default()
        });
        let why = meta.site.check().expect_err("refused");
        assert!(why.contains("help:"), "{why}");
    }
@APP_TEST@}
"#;

const APP_TEST: &str = r##"
    #[tokio::test]
    async fn the_home_page_carries_the_site_title_and_its_tags() {
        let app = crate::app::build().expect("the application builds");
        let request = brocanth_server::Request::new(Verb::Get, "@PATH@");
        let response = crate::app::handle(&app, request).await;
        let body = response.body.bytes().expect("a whole body");
        let body = std::str::from_utf8(body).expect("utf-8");
        assert!(body.contains("<title>@NAME@</title>"), "{body}");
        let tag = r#"@TAG@"#;
        assert!(body.contains(tag), "{body}");
    }
"##;

const AGENTS: &str = "\n\
    ## Page metadata\n\
    \n\
    Every page has a title and share tags. A page sets its own with\n\
    `ctx.meta()` before it renders: `ctx.meta().title(\"About\")`, then\n\
    `.description(..)`, `.image(..)`, `.canonical(..)`, `.kind(\"article\")`\n\
    or `.noindex()`. What a page leaves out comes from `[meta]`. Do not\n\
    write a `<title>` or a `<meta>` tag in a page: the root layout writes\n\
    them from `ctx.head()`. Set `base_url` in production, or no page has a\n\
    canonical URL.\n\
    \n\
    A page that builds its own HTML writes `ctx.head()` in its `<head>`:\n\
    its `Display` is the tags, escaped. A JSON answer (`.jt`) takes\n\
    `jt::Raw(&ctx.head().to_json())`.\n";

const AGENTS_RSX: &str = "\n\
    The root layout declares `meta: &Head`, and `html!` fills it with\n\
    `ctx.head()`. It writes `<title>{meta.title()}</title>` and `{! meta }`.\n\
    Only a layout that writes `<head>` declares `meta`.\n";
