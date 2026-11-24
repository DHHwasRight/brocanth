//! The `content` and `feed` modules, written into a project by the CLI's
//! generator.
//!
//! Each shape is generated, checked with rustfmt, and compared with
//! `tests/golden/<name>.txt`. Bless with `BROCANTH_BLESS=1`; build each
//! project and run its own tests with `BROCANTH_BUILD=1`: the build script
//! checks the posts, and the pages answer through `app::handle`.

use brocanth_content::scaffold::{Content, Feed};
use brocanth_markdown::scaffold::MarkdownModule;
use brocanth_scaffold::testing;

const MODULES: &[&dyn brocanth_scaffold::Module] = &[&Content, &Feed, &MarkdownModule];

fn generate(picks: &[&'static str]) -> Vec<(String, String)> {
    brocanth_cli::testing::generate("my-blog", picks, MODULES).unwrap_or_else(|e| panic!("{e}"))
}

fn shape(name: &str, picks: &[&'static str]) -> Vec<(String, String)> {
    let files = generate(picks);
    testing::assert_formatted(&files);
    testing::golden(
        format!("{}/tests/golden/{name}.txt", env!("CARGO_MANIFEST_DIR")),
        &testing::snapshot(&files),
    );
    if testing::building() {
        let built = brocanth_cli::testing::generate_for_build("my-blog", picks, MODULES)
            .unwrap_or_else(|e| panic!("{e}"));
        testing::build(&built).unwrap_or_else(|e| panic!("{e}"));
    }
    files
}

fn file<'a>(files: &'a [(String, String)], path: &str) -> &'a str {
    files
        .iter()
        .find(|(p, _)| p == path)
        .map(|(_, text)| text.as_str())
        .unwrap_or_else(|| panic!("{path} is generated"))
}

/// The collection alone: a post, its type, the build step and two pages.
#[test]
fn a_blog_gets_a_post_its_type_and_two_pages() {
    let files = shape("content", &["rsx", "markdown", "content"]);
    let build = file(&files, "build.rs");
    assert!(build.contains("brocanth_content::build::collection::<posts::Post>(\"content/posts\")"));
    let manifest = file(&files, "Cargo.toml");
    assert!(manifest.contains("[build-dependencies]"), "{manifest}");
}

/// With `meta`, `feed` and a config file: the post's title is the page's,
/// the layout links the feed, and the development file has a site URL.
#[test]
fn with_meta_and_a_feed_the_post_is_an_article_and_the_layout_links_the_feed() {
    let files = shape(
        "content-feed",
        &["config", "rsx", "markdown", "content", "feed", "meta"],
    );
    assert!(file(&files, "src/pages/layout.rsx").contains("href=\"/feed.xml\""));
    assert!(
        file(&files, "config/development.toml").contains("site_url = \"http://localhost:3000\"")
    );
}

/// With `i18n`, a translation beside the post and one build script for the
/// catalogs and the posts.
#[test]
fn with_i18n_the_catalogs_and_the_posts_share_one_build_script() {
    let files = shape(
        "content-i18n",
        &[
            "config",
            "rsx",
            "markdown",
            "content",
            "feed",
            "i18n",
            "i18n-routing",
        ],
    );
    let build = file(&files, "build.rs");
    assert!(build.contains("brocanth_i18n::build::compile"), "{build}");
    assert!(
        build.contains("brocanth_content::build::collection"),
        "{build}"
    );
    file(&files, "content/posts/hello.fr.md");
}

/// On a static site the export writes each published post, drafts stay
/// hidden by default, and the feed has no site URL until one is set.
#[test]
fn a_static_site_exports_the_posts_and_needs_its_published_address() {
    let files = shape(
        "content-static",
        &[
            "static", "config", "commands", "rsx", "markdown", "content", "feed",
        ],
    );
    assert!(
        file(&files, "src/commands/export.rs").contains(".paths(crate::plugins::content::paths())")
    );
    assert!(file(&files, "config/development.toml").contains("site_url = \"\""));
}

/// The feed needs the collection, and the collection needs its renderer
/// and its templates.
#[test]
fn the_requirements_are_refused_by_name() {
    for (picks, missing) in [
        (&["rsx", "markdown", "feed"][..], "content"),
        (&["rsx", "content"][..], "markdown"),
        (&["markdown", "content"][..], "rsx"),
    ] {
        let wrong =
            brocanth_cli::testing::generate("my-blog", picks, MODULES).expect_err("refused");
        assert!(wrong.contains(missing), "{wrong}");
        assert!(wrong.contains("help:"), "{wrong}");
    }
}
