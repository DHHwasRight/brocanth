//! The `robots` module, written into a project by the CLI's generator.
//!
//! The pattern for a module crate, in the repository or outside it: generate
//! a project with the module under test, check that rustfmt leaves every
//! Rust file alone, and compare the project with a golden file. Bless with
//! `BROCANTH_BLESS=1`; build each project too with `BROCANTH_BUILD=1`.
//!
//! The module is passed as a `&dyn Module`, so the test runs the module this
//! crate compiles, and a module the CLI does not list works the same way.

use brocanth_robots::scaffold::{Robots, Sitemap};
use brocanth_scaffold::testing;

/// Generates `picks`, checks it, and compares it with
/// `tests/golden/<name>.txt`.
fn shape(name: &str, picks: &[&'static str]) {
    let files = brocanth_cli::testing::generate("my-blog", picks, &[&Robots, &Sitemap])
        .unwrap_or_else(|e| panic!("{e}"));
    testing::assert_formatted(&files);
    testing::golden(
        format!("{}/tests/golden/{name}.txt", env!("CARGO_MANIFEST_DIR")),
        &testing::snapshot(&files),
    );
    if testing::building() {
        let files =
            brocanth_cli::testing::generate_for_build("my-blog", picks, &[&Robots, &Sitemap])
                .unwrap_or_else(|e| panic!("{e}"));
        testing::build(&files).unwrap_or_else(|e| panic!("{e}"));
    }
}

/// With a config file and an admin to keep crawlers out of: the
/// development file answers `Disallow: /`.
#[test]
fn a_server_project_serves_robots_txt_from_its_settings() {
    shape("robots", &["config", "admin", "robots"]);
}

/// On a static site the file opens the site, because the export is what
/// gets published.
#[test]
fn a_static_site_exports_robots_txt_as_a_file() {
    shape("robots-static", &["static", "config", "commands", "robots"]);
}

/// A sitemap with no config file: the base URL is a flag, and the admin
/// page is kept out of the list.
#[test]
fn a_server_project_serves_sitemap_xml_from_its_pages() {
    shape("sitemap", &["admin", "sitemap"]);
}

/// Both crawler files on a static site: robots.txt names the sitemap, and
/// the export writes both.
#[test]
fn a_static_site_exports_sitemap_xml_and_robots_txt_names_it() {
    shape(
        "sitemap-static",
        &["static", "config", "commands", "robots", "sitemap"],
    );
}

/// A key no module has comes back as an error with its help line.
#[test]
fn a_refused_project_says_why() {
    let wrong =
        brocanth_cli::testing::generate("my-blog", &["robots", "no-such-module"], &[&Robots])
            .expect_err("an unknown key is refused");
    assert!(wrong.contains("no-such-module"), "{wrong}");
    assert!(wrong.contains("help:"), "{wrong}");
}

/// A module the CLI does not list: a hooking plugin from a third-party
/// crate, registered through `snippet::hooking_line`.
struct Correlation;

impl brocanth_scaffold::Module for Correlation {
    fn manifest(&self) -> brocanth_scaffold::Manifest {
        brocanth_scaffold::Manifest::new("correlation", "an id per request")
            .category(brocanth_scaffold::Category::Middleware)
    }

    fn contribute(&self, _: &brocanth_scaffold::Project, s: &mut brocanth_scaffold::Scaffold) {
        use brocanth_scaffold::snippet;
        let ty = "brocanth_correlation::Correlation";
        snippet::hooking(
            s,
            "correlation",
            ty,
            format!(
                "        // Reads `x-request-id`, or makes one.\n{}",
                snippet::hooking_line(ty, "brocanth_correlation::CorrelationConfig::default()")
            ),
        );
    }
}

#[test]
fn a_module_the_cli_does_not_list_joins_the_project() {
    let files = brocanth_cli::testing::generate(
        "my-blog",
        &["config", "robots", "correlation"],
        &[&Robots, &Correlation],
    )
    .unwrap_or_else(|e| panic!("{e}"));
    testing::assert_formatted(&files);
    let plugins = &files
        .iter()
        .find(|(path, _)| path == "src/plugins/mod.rs")
        .expect("src/plugins/mod.rs")
        .1;
    assert!(
        plugins
            .contains("<brocanth_correlation::Correlation as brocanth::plugin_api::Plugin>::new("),
        "{plugins}"
    );
    assert!(
        plugins.contains("<brocanth_correlation::Correlation as HookSource>::Families"),
        "{plugins}"
    );
}
