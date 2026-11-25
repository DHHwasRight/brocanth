//! `feed` -- what `brocanth create` writes when somebody asks for a feed of
//! the posts. Compiled only under the `scaffold` feature.
//!
//! The shape of the `sitemap` module: the endpoint is a file,
//! `src/pages/feed.xml.rs`, and a plugin renders the XML once, at start,
//! from the `content` collection and the `[feed]` settings.
//!
//! **Decision: `site_url` is required.** A feed is read away from the site,
//! so every URL in it is absolute, and a feed of `localhost` URLs sends
//! every reader nowhere. Unset or empty, `/feed.xml` answers 500 with the
//! fix in the log, and `serve` refuses to start: the plugin registers a
//! readiness check, which the start runs once before it binds. A project's
//! own tests build the application without serving, so they run without
//! one. With `config`, the development file sets the address `serve`
//! listens on; on a static site it is empty, because the export is what
//! gets published.
//!
//! **Decision: its own setting, not `[sitemap] base_url` or `[meta]
//! base_url`.** The flag `--feed.site_url` is what the module promises, and
//! `feed` needs neither module. The note says to keep them equal.
//!
//! **Decision: the link is in the root layout.** `rsx` writes that file, so
//! `rsx` adapts to `feed`: `<link rel="alternate" type="application/atom+xml"
//! href="/feed.xml">` in `<head>`, on every page.
//!
//! It is `outside_runtime`: the static export writes `/feed.xml` as the file
//! `feed.xml`.

use brocanth_scaffold::snippet::{configurable, settings_derive};
use brocanth_scaffold::{
    AgentsSection, Category, Contribution, Dep, Manifest, Module, Project, Scaffold, Setting, Slot,
};

/// The Atom feed, as a choice on the `brocanth create` menu.
pub struct Feed;

impl Module for Feed {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "feed",
            "an Atom feed of the posts at /feed.xml, with absolute URLs from `--feed.site_url`",
        )
        .contributes::<AgentsSection>(
            Contribution::new("feed", "what the feed needs before it is published"),
            true,
        )
        .category(Category::Routes)
        .needs("content", "the feed lists the posts of the collection")
        .adapts_to(
            "static",
            "the development file leaves `site_url` empty: the export is the published site",
        )
        .adapts_to(
            "i18n-routing",
            "the test reads the layout's link on the home page under /en",
        )
        .outside_runtime()
        .asked_in("pages", "An Atom feed of the posts")
        .krate(Dep::brocanth("brocanth-content"))
        // Its test reads the layout's `<link>`. `content` asks for the
        // same line, and the two merge into one.
        .krate(Dep::brocanth("brocanth-test").for_dev())
        .absent(
            "feed",
            "no feed; a reader learns of a new post by visiting the site",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/feed/mod.rs", plugin_rs(project));
        // The last dot is the extension, so this file answers `/feed.xml`,
        // and the static export writes it as `feed.xml`.
        s.file("src/pages/feed.xml.rs", PAGE_RS);
        s.at(Slot::PagesRoutes).line("/feed.xml");
        s.at(Slot::PluginsMod).line("pub mod feed;");
        configurable(s, project, "feed", "Feed", settings(project));
        s.at(Slot::AgentsSections).block(AGENTS.to_string());
    }
}

/// The address `serve` listens on.
const DEVELOPMENT_SITE: &str = "http://localhost:3000";

/// The `[feed]` section.
fn settings(project: &Project) -> Vec<Setting> {
    let (value, note) = if project.has("static") {
        (
            "",
            "Required. `export` reads this file, and the export is the site that\n\
             gets published, so set the published address before you export:\n\
             \"https://example.com\". Empty, the export and `serve` stop and say so.",
        )
    } else {
        (
            DEVELOPMENT_SITE,
            "Required. This file is the development one; set the production\n\
             address in production's file, with the environment or with\n\
             --feed.site_url. Empty, the server refuses to start.",
        )
    };
    vec![
        Setting::text(
            "site_url",
            value,
            "The site's address, scheme and host: every URL in the feed starts with it.",
        )
        .note(note),
        Setting::text("title", project.title, "The feed's title."),
        Setting::text(
            "author",
            project.title,
            "The author Atom names for the feed.",
        ),
        Setting::integer("limit", 20, "The most posts the feed lists, newest first."),
    ]
}

const PAGE_RS: &str = r#"//! `/feed.xml` -- because the file is called `feed.xml.rs`.
//!
//! The XML was rendered once, at start, by `src/plugins/feed/mod.rs`. This
//! page hands it out. With no `site_url` there is no feed: a 500, with the
//! fix in the log.

use crate::pages::{Ctx, Error, IntoResponse};

/// GET `/feed.xml`.
///
/// An hour of cache: the feed changes only when the application is built
/// again.
pub fn handle(_ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let body = crate::plugins::feed::xml().map_err(Error::internal)?;
    Ok((
        body.to_string(),
        200,
        vec![
            (
                "content-type".to_string(),
                "application/atom+xml; charset=utf-8".to_string(),
            ),
            ("cache-control".to_string(), "max-age=3600".to_string()),
        ],
    ))
}
"#;

fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "feed");
    PLUGIN_RS
        .replace("@DERIVE@", &derive)
        .replace("@NOTE@", &note)
        .replace("@TITLE@", &format!("{:?}", project.title))
        .replace(
            "@HOME@",
            if project.has("i18n-routing") {
                "/en"
            } else {
                "/"
            },
        )
}

const PLUGIN_RS: &str = r##"//! Written by `brocanth`. The Atom feed of the posts, served at
//! `/feed.xml` by `src/pages/feed.xml.rs`.
//!
//! Rendered once at start from the published posts (no draft, no
//! translation) and `Settings`. Every URL in it is `site_url` and a path: a
//! feed is read away from the site. An unset `site_url` is no feed, and the
//! start refuses: a readiness check says so before the server binds.

use std::sync::OnceLock;

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::NoContext;
use brocanth_content::feed::Atom;

/// Where the feed is served: `src/pages/feed.xml.rs`. The root layout links
/// it.
pub const PATH: &str = "/feed.xml";

/// The XML, or why there is none, set once at `configure`.
static RENDERED: OnceLock<Result<String, String>> = OnceLock::new();

/// The feed's XML, or why there is none.
pub fn xml() -> Result<&'static str, String> {
    match RENDERED.get() {
        Some(Ok(xml)) => Ok(xml),
        Some(Err(why)) => Err(why.clone()),
        None => Err("the feed plugin is not registered\n\
             help: register crate::plugins::feed::Feed in src/plugins/mod.rs"
            .to_string()),
    }
}

/// What this plugin is built from.
///
/// @NOTE@
@DERIVE@
pub struct Settings {
    /// The site's address, scheme and host. Required.
    pub site_url: String,
    /// The feed's title.
    pub title: String,
    /// The author Atom names for the feed.
    pub author: String,
    /// The most posts the feed lists, newest first.
    pub limit: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            site_url: String::new(),
            title: @TITLE@.to_string(),
            author: @TITLE@.to_string(),
            limit: 20,
        }
    }
}

/// The feed these settings describe, over the published posts.
pub fn render(settings: &Settings) -> Result<String, String> {
    let posts = crate::plugins::content::collection()?;
    let feed = Atom::new(&settings.site_url, PATH, &settings.title, &settings.author)?;
    Ok(feed.entries(posts.published(), settings.limit).render())
}

pub struct Feed {
    settings: Settings,
}

impl Plugin for Feed {
    type Config = Settings;
    type Provides = NoContext;
    const NAME: &'static str = "feed";

    fn new(config: Settings) -> Self {
        Self { settings: config }
    }

    /// Renders the feed. Without a `site_url` it registers a check that
    /// fails, so `serve` stops before it binds and `/readyz` says why; a
    /// test that builds the application and serves nothing still runs.
    fn configure(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        let rendered = render(&self.settings);
        if let Err(why) = &rendered {
            let why = why.clone();
            lc.ready_when(move || {
                let why = why.clone();
                async move { Err(why) }
            });
        }
        let _ = RENDERED.set(rendered);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use brocanth::routing::Verb;
    use brocanth_server::Request;
    use brocanth_test::Html;

    use super::*;

    fn settings(site_url: &str) -> Settings {
        Settings {
            site_url: site_url.to_string(),
            ..Settings::default()
        }
    }

    #[test]
    fn the_feed_lists_the_posts_at_absolute_urls() {
        let xml = render(&settings("https://example.com")).expect("a feed");
        assert!(
            xml.contains("<feed xmlns=\"http://www.w3.org/2005/Atom\">"),
            "{xml}"
        );
        assert!(
            xml.contains("<id>https://example.com/posts/hello</id>"),
            "{xml}"
        );
        assert!(xml.contains("<title>Hello, world</title>"), "{xml}");
    }

    #[test]
    fn no_site_url_is_no_feed_and_says_how_to_set_it() {
        let why = render(&settings("")).expect_err("refused");
        assert!(why.contains("help: set `site_url` under [feed]"), "{why}");
    }

    #[tokio::test]
    async fn every_page_links_the_feed() {
        let app = crate::app::build().expect("the application builds");
        let response = crate::app::handle(&app, Request::new(Verb::Get, "@HOME@")).await;
        let body = response.body.bytes().expect("a whole body");
        let page = Html::new(String::from_utf8_lossy(body));
        let feed = page.tags("link").into_iter().find(|link| {
            link.attr("rel") == Some("alternate")
                && link.attr("type") == Some("application/atom+xml")
        });
        let feed = feed.unwrap_or_else(|| panic!("no feed link:\n{}", page.source()));
        assert_eq!(feed.attr("href"), Some("/feed.xml"));
    }
}
"##;

const AGENTS: &str = "\n\
    ## Feed\n\
    \n\
    `/feed.xml` is an Atom feed of the published posts: no draft, no\n\
    translation, at most `[feed] limit`. Every URL in it starts with\n\
    `[feed] site_url`, which is required: unset, the server refuses to\n\
    start. Set it per deployment (`--feed.site_url=https://example.com` or\n\
    the environment). Do not write a second feed route, and do not make a\n\
    URL in a post relative to the feed: each entry carries `xml:base`, so a\n\
    link from the root (`/about`) resolves.\n";
