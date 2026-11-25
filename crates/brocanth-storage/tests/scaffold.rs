//! The `storage-routes` module, written into a project by the CLI's
//! generator.
//!
//! Each shape is checked by rustfmt and compared with
//! `tests/golden/<name>.txt`. Bless with `BROCANTH_BLESS=1`; build each
//! project too with `BROCANTH_BUILD=1`.

use brocanth_scaffold::testing;
use brocanth_storage::scaffold::{FileStorage, Image, StorageRoutes, StorageThumbnails};

fn shape(name: &str, picks: &[&'static str]) {
    let modules: [&dyn brocanth_scaffold::Module; 4] =
        [&FileStorage, &StorageRoutes, &StorageThumbnails, &Image];
    let files = brocanth_cli::testing::generate("my-photos", picks, &modules)
        .unwrap_or_else(|e| panic!("{e}"));
    testing::assert_formatted(&files);
    testing::golden(
        format!("{}/tests/golden/{name}.txt", env!("CARGO_MANIFEST_DIR")),
        &testing::snapshot(&files),
    );
    if testing::building() {
        let files = brocanth_cli::testing::generate_for_build("my-photos", picks, &modules)
            .unwrap_or_else(|e| panic!("{e}"));
        testing::build(&files).unwrap_or_else(|e| panic!("{e}"));
    }
}

/// With a config file and the CSRF check: the settings are a section, and
/// the `lift` hook carries a plain form's token to the check.
#[test]
fn the_routes_sit_around_the_csrf_check() {
    shape(
        "storage-routes",
        &[
            "config",
            "middleware",
            "middleware-csrf",
            "storage",
            "storage-routes",
        ],
    );
}

/// With `image`: `formats` and `max_pixels` under `[storage_routes]`, and
/// the link helpers in `src/plugins/image/mod.rs`.
#[test]
fn image_lists_the_formats_and_caps_the_pixels() {
    shape(
        "image",
        &[
            "config",
            "middleware",
            "middleware-csrf",
            "storage",
            "storage-routes",
            "storage-thumbnails",
            "image",
        ],
    );
}

/// With thumbnails: the image decoders are linked, and `thumb_sizes` lists
/// the sizes `?thumb=` may ask for.
#[test]
fn thumbnails_link_the_decoders_and_list_the_sizes() {
    shape(
        "storage-thumbnails",
        &[
            "config",
            "middleware",
            "middleware-csrf",
            "storage",
            "storage-routes",
            "storage-thumbnails",
        ],
    );
}

/// With `db`: the objects live in Postgres, on the `db` pool.
#[test]
fn with_s3_the_objects_live_in_a_bucket() {
    shape("storage-s3", &["config", "storage", "storage-s3"]);
}

#[test]
fn with_gcs_the_objects_live_in_a_bucket() {
    shape("storage-gcs", &["config", "storage", "storage-gcs"]);
}

/// No backend chosen: the resolver adds the default, `storage-fs`.
#[test]
fn with_no_backend_the_objects_live_on_the_disk() {
    let modules: [&dyn brocanth_scaffold::Module; 1] = [&FileStorage];
    let files = brocanth_cli::testing::generate("my-photos", &["storage"], &modules)
        .unwrap_or_else(|e| panic!("{e}"));
    let toml = &files.iter().find(|(p, _)| p == "brocanth.toml").unwrap().1;
    assert!(
        toml.contains(
            r#"ObjectStore = { provider = "storage-fs", why = "storage requires ObjectStore" }"#
        ),
        "{toml}"
    );
    let plugin = &files
        .iter()
        .find(|(p, _)| p == "src/plugins/storage/mod.rs")
        .unwrap()
        .1;
    assert!(plugin.contains("LocalDisk::open"), "{plugin}");
}

#[test]
fn with_db_the_objects_live_in_postgres() {
    shape("storage-db", &["config", "db", "storage", "storage-db"]);
}

/// Without a config file: the settings are literals and flags.
#[test]
fn the_routes_work_without_a_config_file() {
    shape(
        "storage-routes-no-config",
        &["middleware", "middleware-csrf", "storage", "storage-routes"],
    );
}

/// With `i18n-routing`, the routes moved in src/modules/storage_routes.rs
/// keep no locale segment, and nothing else is edited. The `i18n`
/// registration reads the module file (docs/module-config.md §9). The
/// project's own storage tests ask at the moved path: a locale redirect
/// would answer 307 where they want 403.
#[test]
fn a_moved_path_keeps_no_locale_segment_with_no_other_edit() {
    let picks = [
        "config",
        "rsx",
        "i18n",
        "i18n-routing",
        "middleware",
        "middleware-csrf",
        "storage",
        "storage-routes",
    ];
    let modules: [&dyn brocanth_scaffold::Module; 4] =
        [&FileStorage, &StorageRoutes, &StorageThumbnails, &Image];
    let files = brocanth_cli::testing::generate("my-photos", &picks, &modules)
        .unwrap_or_else(|e| panic!("{e}"));
    let file = |path: &str| -> String {
        files
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, text)| text.clone())
            .unwrap_or_else(|| panic!("{path} is generated"))
    };
    assert!(
        file("src/plugins/mod.rs")
            .contains(".keep_unprefixed(crate::modules::storage_routes::MODULE.path)"),
        "the i18n registration reads the path from the module file"
    );
    let unprefixed = file("config/development.toml")
        .lines()
        .find(|line| line.starts_with("unprefixed = "))
        .expect("[i18n] unprefixed")
        .to_string();
    assert!(
        !unprefixed.contains("/files"),
        "the path is not copied into a setting: {unprefixed}"
    );
    if !testing::building() {
        return;
    }
    let mut files = brocanth_cli::testing::generate_for_build("my-photos", &picks, &modules)
        .unwrap_or_else(|e| panic!("{e}"));
    let (_, module) = files
        .iter_mut()
        .find(|(p, _)| p == "src/modules/storage_routes.rs")
        .expect("the module file");
    let moved = module.replace("path: \"/files\"", "path: \"/uploads\"");
    assert_ne!(*module, moved, "the module file names the default path");
    *module = moved;
    testing::build(&files).unwrap_or_else(|e| panic!("{e}"));
}
