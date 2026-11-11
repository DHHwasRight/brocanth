//! The `analytics` module, written into a project by the CLI's generator.
//!
//! Build and test the project too with `BROCANTH_BUILD=1`.

use brocanth_analytics::scaffold::PageViews;
use brocanth_scaffold::testing;

fn file<'a>(files: &'a [(String, String)], path: &str) -> &'a str {
    files
        .iter()
        .find(|(p, _)| p == path)
        .map(|(_, text)| text.as_str())
        .unwrap_or_else(|| panic!("{path} is generated"))
}

/// The admin is never counted, wherever src/modules/admin.rs mounts it.
/// The registration reads `mount_at` from the module file and the
/// `exclude` setting does not copy it (docs/module-config.md §9). With
/// `BROCANTH_BUILD=1`, a project with the admin moved builds and passes its
/// tests with no other edit.
#[test]
fn a_moved_admin_is_never_counted_with_no_other_edit() {
    let picks = ["config", "db", "admin", "analytics"];
    let files = brocanth_cli::testing::generate("my-blog", &picks, &[&PageViews])
        .unwrap_or_else(|e| panic!("{e}"));
    testing::assert_formatted(&files);
    let plugins = file(&files, "src/plugins/mod.rs");
    assert!(
        plugins.contains(".exclude_path(crate::modules::admin::MODULE.mount_at)"),
        "the registration reads the admin's path from its module file:\n{plugins}"
    );
    let exclude = file(&files, "config/development.toml")
        .lines()
        .find(|line| line.starts_with("exclude = "))
        .expect("[analytics] exclude");
    assert_eq!(exclude, "exclude = []", "the admin's path is not copied");

    // Without the admin, nothing names it.
    let files =
        brocanth_cli::testing::generate("my-blog", &["config", "db", "analytics"], &[&PageViews])
            .unwrap_or_else(|e| panic!("{e}"));
    assert!(!file(&files, "src/plugins/mod.rs").contains("exclude_path"));

    if !testing::building() {
        return;
    }
    let mut files = brocanth_cli::testing::generate_for_build("my-blog", &picks, &[&PageViews])
        .unwrap_or_else(|e| panic!("{e}"));
    brocanth_cli::testing::move_admin(&mut files, "/backoffice").unwrap_or_else(|e| panic!("{e}"));
    testing::build(&files).unwrap_or_else(|e| panic!("{e}"));
}
