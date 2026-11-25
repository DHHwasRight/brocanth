//! `content` -- what `brocanth create` writes when somebody asks for a
//! content collection. Compiled only under the `scaffold` feature.
//!
//! What a project gets:
//!
//! - `content/posts/hello.md`, the first entry (and `hello.fr.md` with
//!   `i18n`).
//! - `src/plugins/content/posts.rs`: the front matter as a Rust type. The
//!   project owns it and adds fields.
//! - a step in `build.rs` (`Slot::BuildMain`) that reads every file against
//!   that type and stops the build on a bad one, with the file and a
//!   `help:` line.
//! - `src/plugins/content/mod.rs`: the files the step listed, embedded, read
//!   once at start, and the `Content` plugin that puts a `Posts` view on the
//!   context, drafts shown or not by `[content] drafts`.
//! - `src/pages/posts/index.rs` and `src/pages/posts/[slug].rs`, with their
//!   `.rsx` templates: the list, newest first, and one post.
//!
//! **Decision: the build checks, the binary renders.** A bad front matter
//! is a mistake in the source, so it fails `cargo build`, never a request.
//! The binary embeds the files and renders them once at start; the build
//! script links no Markdown or HTML parser. There is no database.
//!
//! **Decision: rsx.** The two pages are templates, and a list of posts
//! built as a `String` is the code an `rsx` project never writes. So the
//! module needs `rsx`.
//!
//! **Decision: one collection, `posts`.** A second one is a copy of the
//! directory, the type, the build step and the two pages under a new name;
//! AGENTS.md says how. A generator for collections is a later step.
//!
//! It is `outside_runtime`: the static export renders the pages and writes
//! each published post, through `src/commands/export.rs`.

use brocanth_scaffold::snippet::{configurable, settings_derive};
use brocanth_scaffold::{
    AgentsSection, Category, Contribution, Dep, Manifest, Module, Project, Scaffold, Setting, Slot,
};

pub use crate::scaffold_feed::Feed;

/// Content collections, as a choice on the `brocanth create` menu.
pub struct Content;

impl Module for Content {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "content",
            "Markdown files with typed front matter, checked by the build, listed at /posts; no database",
        )
        .contributes::<AgentsSection>(
            Contribution::new("content", "how a post is written and how the collection grows"),
            true,
        )
        .category(Category::Enhancer)
        .category(Category::Routes)
        .needs("markdown", "each entry's body is rendered and cleaned by it")
        .needs("rsx", "the list and the post are `.rsx` templates")
        .adapts_to(
            "meta",
            "a post's title and description are the page's title, description and share tags",
        )
        .adapts_to(
            "i18n",
            "`hello.fr.md` translates `hello.md`, and a page lists the request's locale",
        )
        .adapts_to(
            "i18n-routing",
            "the links keep the locale segment, and the tests ask under /en",
        )
        .adapts_to(
            "static",
            "drafts are hidden unless asked for, because the export is what gets published",
        )
        // The `i18n` plugin chooses the locale a page lists.
        .after("i18n")
        .outside_runtime()
        .asked_in("pages", "A blog: Markdown posts with typed front matter")
        .krate(Dep::brocanth("brocanth-content"))
        .krate(
            Dep::brocanth("brocanth-content")
                .no_default_features()
                .feature("build")
                .for_build()
                .note("build.rs checks content/; no Markdown or HTML parser in the build script"),
        )
        .krate(
            Dep::brocanth("brocanth-test")
                .for_dev()
                .note("the page tests read the HTML by element"),
        )
        .krate(Dep::version("serde", "1").feature("derive"))
        .krate(
            Dep::version("serde", "1")
                .feature("derive")
                .for_build()
                .note("build.rs reads the front matter into src/plugins/content/posts.rs"),
        )
        .absent(
            "brocanth-content",
            "no content collections; a post is a page written by hand, with no checked front matter",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("content/posts/hello.md", HELLO_MD);
        if project.has("i18n") {
            s.file("content/posts/hello.fr.md", HELLO_FR_MD);
        }
        s.file("src/plugins/content/posts.rs", POSTS_RS);
        s.file("src/plugins/content/mod.rs", plugin_rs(project));
        s.file("src/pages/posts/index.rs", index_rs(project));
        s.file("src/pages/posts/index.rsx", INDEX_RSX);
        s.file("src/pages/posts/[slug].rs", post_rs(project));
        s.file("src/pages/posts/[slug].rsx", POST_RSX);
        s.at(Slot::PagesRoutes).line("/posts");
        s.at(Slot::PagesRoutes).line("/posts/{slug}");
        s.at(Slot::PluginsMod).line("pub mod content;");
        s.at(Slot::BuildMain).block(BUILD_STEP.to_string());
        configurable(s, project, "content", "Content", settings(project));
        s.at(Slot::AgentsSections).block(AGENTS.to_string());
    }
}

/// The `[content]` section.
fn settings(project: &Project) -> Vec<Setting> {
    let note = if project.has("static") {
        "Unset, drafts are hidden: the export is what gets published, and it\n\
         never writes a draft. Set it to true to read drafts with `serve`."
    } else {
        "Unset, drafts are shown in a debug build (`cargo run`) and hidden in a\n\
         release build. A draft is never in the feed."
    };
    vec![
        Setting::bool("drafts", false, "Show drafts in the list and at their URL.")
            .note(note)
            .commented(),
    ]
}

/// The step `build.rs` runs, inside its `main`.
const BUILD_STEP: &str = r#"    // -- content: reads every `content/posts/*.md` against the type in
    // `src/plugins/content/posts.rs`, and stops the build on a bad file with
    // its path, its line and a `help:` line. Writes the list of files to
    // `$OUT_DIR/content_posts.rs`, which `src/plugins/content/mod.rs`
    // embeds. The type is compiled here as well as in the binary, so the
    // two cannot disagree about a field.
    {
        #[allow(dead_code)]
        #[path = "src/plugins/content/posts.rs"]
        mod posts;
        brocanth_content::build::collection::<posts::Post>("content/posts")
            .unwrap_or_else(|e| panic!("{e}"));
    }
"#;

const HELLO_MD: &str = r#"+++
title = "Hello, world"
description = "The first post: what a post is and where it lives."
date = 2026-11-22
tags = ["meta"]
+++

This post is `content/posts/hello.md`. The name is the URL:
`/posts/hello`.

## Front matter

The lines between the two `+++` are TOML, read into the `Post` type in
`src/plugins/content/posts.rs`. A missing `title`, a date that is not a
date or a misspelt key stops `cargo build` and names this file.

## The body

Markdown, with tables, footnotes[^1], ~~strikethrough~~ and task lists:

- [x] write the first post
- [ ] write the second

| Field | Required |
|-------|:--------:|
| title | yes |
| date | yes |
| draft | no |

[^1]: Rendered once, when the server starts.
"#;

const HELLO_FR_MD: &str = r#"+++
title = "Bonjour, le monde"
description = "Le premier billet, traduit."
date = 2026-11-22
tags = ["meta"]
+++

Ce billet est `content/posts/hello.fr.md`, la traduction de `hello.md`.
Une page en français le montre à la place de l'original.
"#;

const POSTS_RS: &str = r#"//! The front matter of `content/posts/*.md`: the lines between the two
//! `+++` at the top of each file, as TOML.
//!
//! This file is yours. Add a field and every post may set it; add a
//! required one and `cargo build` names each post that lacks it.
//!
//! `build.rs` compiles this file too, to check every post before the
//! binary is built. So it may use `serde`, `std` and `brocanth_content`,
//! and nothing else from this crate.

use brocanth_content::{Date, FrontMatter};
use serde::Deserialize;

/// One post's front matter.
///
/// `deny_unknown_fields`: a misspelt key (`titel`) is a build error, not a
/// post with no title.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Post {
    pub title: String,
    /// One or two sentences: the list's summary and the page description.
    #[serde(default)]
    pub description: Option<String>,
    /// The day it was published: `date = 2026-11-22`.
    pub date: Date,
    /// The day it last changed, when that is not `date`.
    #[serde(default)]
    pub updated: Option<Date>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Listed only where `[content] drafts` shows drafts, and never in a
    /// feed or an export.
    #[serde(default)]
    pub draft: bool,
}

impl FrontMatter for Post {
    fn title(&self) -> &str {
        &self.title
    }

    fn date(&self) -> Option<Date> {
        Some(self.date)
    }

    fn updated(&self) -> Option<Date> {
        self.updated
    }

    fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    fn tags(&self) -> &[String] {
        &self.tags
    }

    fn draft(&self) -> bool {
        self.draft
    }
}
"#;

fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "content");
    // On a static site the default is `false`, the type's own: derive it,
    // or clippy's `derivable_impls` flags the hand-written one.
    let (derive, default_impl) = if project.has("static") {
        (
            derive.replacen("#[derive(Debug", "#[derive(Debug, Default", 1),
            "",
        )
    } else {
        (derive, DEFAULT_DEBUG)
    };
    let default_doc = if project.has("static") {
        "    /// Unset, `false`: the export never writes a draft.\n"
    } else {
        "    /// Unset, `true` in a debug build and `false` in a release one.\n"
    };
    PLUGIN_RS
        .replace("@DERIVE@", &derive)
        .replace("@NOTE@", &note)
        .replace("@DEFAULT@", default_impl)
        .replace("@DEFAULT_DOC@\n", default_doc)
}

const DEFAULT_DEBUG: &str = r#"
impl Default for Settings {
    fn default() -> Self {
        Self {
            drafts: cfg!(debug_assertions),
        }
    }
}
"#;

const PLUGIN_RS: &str = r#"//! Written by `brocanth`. The `posts` collection: every
//! `content/posts/*.md`, checked by `build.rs` and read once, at start.
//!
//! `build.rs` reads each file's front matter into [`Post`] (`posts.rs`,
//! beside this file) and stops the build on a bad one. The binary embeds
//! the files it listed and renders each body through `brocanth-markdown`
//! the first time the collection is read. Nothing is read from disk while a
//! request is served, and there is no database.
//!
//! A page reads the posts with `ctx.provided::<Posts, _>()`: a view with
//! drafts shown or hidden, as `[content] drafts` says.

use std::sync::LazyLock;

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::{ContextBuilder, ContextItem, ProvidesContext};
use brocanth_content::{Collection, View};

mod posts;

pub use posts::Post;

/// The files `build.rs` checked, embedded with `include_str!`.
mod sources {
    include!(concat!(env!("OUT_DIR"), "/content_posts.rs"));
}

/// Where the posts are served: `src/pages/posts/`.
pub const BASE: &str = "/posts";

/// Every post, read once. The files and the type cannot change while the
/// binary runs, so one copy serves every application in the process.
static POSTS: LazyLock<Result<Collection<Post>, String>> =
    LazyLock::new(|| Collection::parse(BASE, sources::SOURCES));

/// The collection, drafts included, or why it did not read.
pub fn collection() -> Result<&'static Collection<Post>, String> {
    POSTS.as_ref().map_err(Clone::clone)
}

/// The paths a static export writes: `/posts` and each published post,
/// never a draft.
#[allow(dead_code)]
pub fn paths() -> Vec<String> {
    collection().map(Collection::paths).unwrap_or_default()
}

/// The posts as a page sees them: drafts shown or hidden.
#[derive(Clone, Copy)]
pub struct Posts(pub View<'static, Post>);

impl ContextItem for Posts {}

impl std::ops::Deref for Posts {
    type Target = View<'static, Post>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// What this plugin is built from.
///
/// @NOTE@
@DERIVE@
pub struct Settings {
    /// Show drafts in the list and at their URL.
@DEFAULT_DOC@
    pub drafts: bool,
}
@DEFAULT@
/// Reads the collection at start and puts a [`Posts`] view on each
/// request's context.
pub struct Content {
    settings: Settings,
}

impl Plugin for Content {
    type Config = Settings;
    type Provides = ProvidesContext<Posts>;
    const NAME: &'static str = "content";

    fn new(config: Settings) -> Self {
        Self { settings: config }
    }

    /// Reads every post now, so a file that does not read stops the start
    /// and not a request. `build.rs` checked them, so this does not fail on
    /// a file it checked.
    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        collection()
            .map(|_| ())
            .map_err(|e| PluginError::new(format!("[content] {e}")))
    }

    fn provide(&self, cx: &mut ContextBuilder) {
        if let Ok(posts) = collection() {
            cx.insert(Posts(posts.view(self.settings.drafts)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_post_reads_and_hello_is_published() {
        let posts = collection().expect("build.rs checked every file");
        let hello = posts
            .view(false)
            .get("hello", None)
            .expect("content/posts/hello.md");
        assert_eq!(hello.path, "/posts/hello");
        assert!(!hello.html.is_empty());
        assert!(paths().contains(&"/posts/hello".to_string()));
    }

    #[test]
    fn a_draft_is_in_no_published_path() {
        let posts = collection().expect("build.rs checked every file");
        for post in posts.all().iter().filter(|post| post.data.draft) {
            assert!(!paths().contains(&post.path), "{} is a draft", post.source);
        }
    }
}
"#;

/// The locale a page lists, as an expression of `ctx`.
fn locale_expr(project: &Project) -> &'static str {
    if project.has("i18n") {
        "Some(lang)"
    } else {
        "None"
    }
}

/// The statement that names the request's locale, before the list.
fn locale_let(project: &Project) -> &'static str {
    if project.has("i18n") {
        "    // The locale the `i18n` plugin chose: a translation where one exists.
    \
         let lang = brocanth_i18n::Locales::tag(crate::plugins::i18n::locale(ctx));
"
    } else {
        ""
    }
}

/// A link to `path` that keeps the request's locale segment, as a closure
/// over `ctx`.
fn href_fn(project: &Project) -> &'static str {
    if project.has("i18n-routing") {
        "    // The locale segment the request came under: `/fr/posts/hello`.\n    \
         let segment = ctx.hooked::<brocanth_i18n::PathLocale>().cloned();\n    \
         let href = |path: &str| {\n        \
         segment\n            \
         .as_ref()\n            \
         .map_or_else(|| path.to_string(), |l| l.href(path))\n    \
         };\n"
    } else {
        "    let href = |path: &str| path.to_string();\n"
    }
}

/// The path a test asks for: under `/en` with `i18n-routing`.
fn test_path(project: &Project, path: &str) -> String {
    if project.has("i18n-routing") {
        format!("/en{path}")
    } else {
        path.to_string()
    }
}

fn index_rs(project: &Project) -> String {
    let meta = if project.has("meta") {
        "    ctx.meta()\n        .title(\"Posts\")\n        .description(\"Every post, newest first.\");\n"
    } else {
        ""
    };
    let french = if project.has("i18n") {
        let path = if project.has("i18n-routing") {
            "/fr/posts".to_string()
        } else {
            "/posts?locale=fr".to_string()
        };
        format!(
            r#"
    #[tokio::test]
    async fn a_french_reader_gets_the_translation() {{
        let (status, page) = get("{path}").await;
        assert_eq!(status, 200);
        page.assert_text("Bonjour, le monde");
    }}
"#
        )
    } else {
        String::new()
    };
    INDEX_RS
        .replace("@LOCALE@", locale_expr(project))
        .replace("@LOCALE_LET@\n", locale_let(project))
        .replace("@HREF@\n", href_fn(project))
        .replace("@META@\n", meta)
        .replace("@PATH@", &test_path(project, "/posts"))
        .replace(
            "@LINK@",
            &if project.has("i18n-routing") {
                "/en/posts/hello".to_string()
            } else {
                "/posts/hello".to_string()
            },
        )
        .replace("@FRENCH@", &french)
}

const INDEX_RS: &str = r##"//! `/posts` -- every post, newest first.
//!
//! The posts are `content/posts/*.md`, read once at start by
//! `src/plugins/content/mod.rs`. A draft is listed only where
//! `[content] drafts` shows drafts.

use crate::pages::{html, views, Ctx, Error, IntoResponse};
use crate::plugins::content::Posts;

pub fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let posts = *ctx
        .provided::<Posts, _>()
        .ok_or_else(|| Error::internal("the content plugin is not registered"))?;
@HREF@
@LOCALE_LET@
    let list: Vec<_> = posts
        .list(@LOCALE@)
        .into_iter()
        .map(|post| (post, href(&post.path)))
        .collect();
@META@
    html!(ctx, views::posts::index::Index { posts: &list })
}

#[cfg(test)]
mod tests {
    use brocanth::routing::Verb;
    use brocanth_server::Request;
    use brocanth_test::Html;

    /// The status and the page of a GET, through the whole application.
    async fn get(path: &str) -> (u16, Html) {
        let app = crate::app::build().expect("the application builds");
        let response = crate::app::handle(&app, Request::new(Verb::Get, path)).await;
        let body = response.body.bytes().expect("a whole body");
        (response.status, Html::new(String::from_utf8_lossy(body)))
    }

    #[tokio::test]
    async fn the_list_links_each_post_with_its_date() {
        let (status, page) = get("@PATH@").await;
        assert_eq!(status, 200);
        page.assert_link_to("@LINK@");
        let link = page.link_to("@LINK@").expect("asserted above");
        assert_eq!(link.text(), "Hello, world");
        let dated = page
            .tags("time")
            .iter()
            .any(|time| time.attr("datetime") == Some("2026-11-22"));
        assert!(dated, "no <time> for the post:\n{}", page.source());
    }
@FRENCH@}
"##;

const INDEX_RSX: &str = r#"// The list of posts. Each item is a post and the link to it, which the
// handler made: the link keeps the locale segment when the site has one.

use brocanth_content::Entry;

use crate::plugins::content::Post;

pub fn Index(posts: &[(&Entry<Post>, String)]) -> Element {
    <h1>Posts</h1>
    if posts.is_empty() {
        <p>No post yet. Add one to <code>content/posts/</code>.</p>
    } else {
        <ul>
            for (post, href) in posts {
                <li>
                    <a href={href}>{&post.data.title}</a>
                    <time datetime={post.data.date.to_string()}>{post.data.date.to_string()}</time>
                    if post.data.draft {
                        <em>draft</em>
                    }
                    if let Some(description) = &post.data.description {
                        <p>{description}</p>
                    }
                </li>
            }
        </ul>
    }
}
"#;

fn post_rs(project: &Project) -> String {
    let meta = if project.has("meta") {
        "    // The post's title and description are the page's, and its share\n    \
         // tags say it is an article.\n    \
         let meta = ctx.meta();\n    \
         meta.title(post.data.title.clone()).kind(\"article\");\n    \
         if let Some(description) = &post.data.description {\n        \
         meta.description(description.clone());\n    \
         }\n"
    } else {
        ""
    };
    let title_test = if project.has("meta") {
        "        // The page's <title> is the post's.\n        \
         let title = page.tags(\"title\");\n        \
         assert!(\n            \
         title.iter().any(|t| t.text().starts_with(\"Hello, world\")),\n            \
         \"{}\",\n            \
         page.source()\n        \
         );\n"
    } else {
        ""
    };
    POST_RS
        .replace("@LOCALE@", locale_expr(project))
        .replace("@LOCALE_LET@\n", locale_let(project))
        .replace("@HREF@\n", href_fn(project))
        .replace("@META@\n", meta)
        .replace("@PATH@", &test_path(project, "/posts/hello"))
        .replace("@MISSING@", &test_path(project, "/posts/no-such-post"))
        .replace("@TITLE_TEST@\n", title_test)
}

const POST_RS: &str = r##"//! `/posts/{slug}` -- one post. The slug is the file name:
//! `content/posts/hello.md` is `/posts/hello`.

use crate::pages::{html, views, Ctx, Error, IntoResponse};
use crate::plugins::content::Posts;

pub fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let posts = *ctx
        .provided::<Posts, _>()
        .ok_or_else(|| Error::internal("the content plugin is not registered"))?;
    let slug = ctx.require("slug")?.to_string();
@LOCALE_LET@
    // A slug with no file, or a draft this server hides, is not found.
    let post = posts
        .get(&slug, @LOCALE@)
        .ok_or_else(|| Error::new(404, "Not Found"))?;
@HREF@
    let index = href(crate::plugins::content::BASE);
@META@
    html!(
        ctx,
        views::posts::_slug_::Article {
            post,
            index: &index,
        }
    )
}

#[cfg(test)]
mod tests {
    use brocanth::routing::Verb;
    use brocanth_server::Request;
    use brocanth_test::Html;

    /// The status and the page of a GET, through the whole application.
    async fn get(path: &str) -> (u16, Html) {
        let app = crate::app::build().expect("the application builds");
        let response = crate::app::handle(&app, Request::new(Verb::Get, path)).await;
        let body = response.body.bytes().expect("a whole body");
        (response.status, Html::new(String::from_utf8_lossy(body)))
    }

    #[tokio::test]
    async fn a_post_shows_its_rendered_body() {
        let (status, page) = get("@PATH@").await;
        assert_eq!(status, 200);
        let headed = page.tags("h1").iter().any(|h| h.text() == "Hello, world");
        assert!(headed, "no <h1> with the title:\n{}", page.source());
        // The Markdown, rendered: a heading with its id, and a table.
        let anchored = page
            .tags("h2")
            .iter()
            .any(|h| h.attr("id") == Some("front-matter"));
        assert!(anchored, "no <h2 id=\"front-matter\">:\n{}", page.source());
        assert!(
            !page.tags("table").is_empty(),
            "no <table>:\n{}",
            page.source()
        );
@TITLE_TEST@
    }

    #[tokio::test]
    async fn a_slug_with_no_file_is_not_found() {
        let (status, _) = get("@MISSING@").await;
        assert_eq!(status, 404);
    }
}
"##;

const POST_RSX: &str = r#"// One post. `post.html` is the body, rendered from Markdown and cleaned by
// the sanitizer's allow-list when the server started, so it is written
// with `{! }`. Every other value is escaped.

use brocanth_content::Entry;

use crate::plugins::content::Post;

pub fn Article(post: &Entry<Post>, index: &str) -> Element {
    <article>
        <h1>{&post.data.title}</h1>
        <p>
            <time datetime={post.data.date.to_string()}>{post.data.date.to_string()}</time>
            if let Some(updated) = post.data.updated {
                , updated <time datetime={updated.to_string()}>{updated.to_string()}</time>
            }
        </p>
        if !post.data.tags.is_empty() {
            <ul>
                for tag in &post.data.tags {
                    <li>{tag}</li>
                }
            </ul>
        }
        {! &post.html }
    </article>
    <p><a href={index}>All posts</a></p>
}
"#;

const AGENTS: &str = "\n\
    ## Content\n\
    \n\
    A post is a file: `content/posts/<slug>.md`, served at\n\
    `/posts/<slug>`. Its front matter is TOML between two `+++` lines, read\n\
    into `Post` in `src/plugins/content/posts.rs`. `build.rs` checks every\n\
    file against that type, so a bad post fails `cargo build` with its path\n\
    and a `help:` line: fix the file, not the check. Add a field to `Post`\n\
    to give posts a new value; a template reads it as `post.data.<field>`.\n\
    \n\
    `draft = true` keeps a post out of the list and its URL where\n\
    `[content] drafts` is false (a release build), and always out of the\n\
    feed and the static export. A translation is `<slug>.<locale>.md`\n\
    beside the default file. Do not read `content/` at run time and do not\n\
    store posts in a database: the binary embeds them.\n\
    \n\
    A second collection copies the pattern under a new name: a directory, a\n\
    type, a step in `build.rs`, a plugin and two pages.\n";
