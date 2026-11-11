//! The `cache` and `cache-memory` modules, written into a project by the
//! CLI's generator.
//!
//! Each shape is generated, checked against rustfmt, and compared with
//! `tests/golden/<name>.txt`. Bless with `BROCANTH_BLESS=1`; build each
//! project too with `BROCANTH_BUILD=1`.

use brocanth_cache::scaffold::{Cache, CacheMemory};
use brocanth_scaffold::testing;

/// Generates `picks`, checks it, and compares it with
/// `tests/golden/<name>.txt`.
fn shape(name: &str, picks: &[&'static str]) -> Vec<(String, String)> {
    let files = brocanth_cli::testing::generate("my-app", picks, &[&Cache, &CacheMemory])
        .unwrap_or_else(|e| panic!("{e}"));
    testing::golden(
        format!("{}/tests/golden/{name}.txt", env!("CARGO_MANIFEST_DIR")),
        &testing::snapshot(&files),
    );
    testing::assert_formatted(&files);
    if testing::building() {
        let built =
            brocanth_cli::testing::generate_for_build("my-app", picks, &[&Cache, &CacheMemory])
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
        .unwrap_or_else(|| panic!("no {path} in the project"))
}

/// Alone, the cache opens a pool of its own from `[cache] url`, and
/// `brocanth-cache` takes its `postgres` feature.
#[test]
fn the_cache_alone_opens_its_own_pool() {
    let files = shape("cache", &["config", "cache"]);
    let plugin = file(&files, "src/plugins/cache/mod.rs");
    assert!(plugin.contains("PgCache::connect_lazy(&self.settings.url)"));
    assert!(plugin.contains("Sweeper::spawn"));
    assert!(file(&files, "Cargo.toml").contains("\"postgres\""));
}

/// With `db`, the cache reads the `db` pool, has no `url` of its own, and
/// with `admin` adds its tab behind `integrate.admin`.
#[test]
fn with_db_the_cache_reuses_the_pool_and_the_admin_gets_a_tab() {
    let files = shape("cache-db", &["config", "db", "cache", "admin"]);
    let plugin = file(&files, "src/plugins/cache/mod.rs");
    assert!(plugin.contains("PgCache::from_pool(db.pool().clone())"));
    assert!(!plugin.contains("pub url: String"));
    assert!(!file(&files, "config/development.toml").contains("[cache]\nurl"));
    let plugins = file(&files, "src/plugins/mod.rs");
    assert!(plugins.contains("if crate::modules::cache::MODULE.integrate.admin {"));
    assert!(plugins.contains("CachePanel::installed()"));
}

/// `cache-memory` writes the plugin over `MemoryCache`, with no URL, and
/// `brocanth-cache` links no SQL driver.
#[test]
fn cache_memory_keeps_the_cache_in_the_process() {
    let files = shape("cache-memory", &["config", "cache", "cache-memory"]);
    let plugin = file(&files, "src/plugins/cache/mod.rs");
    assert!(plugin.contains("MemoryCache::new()"));
    assert!(!plugin.contains("url"));
    let cargo = file(&files, "Cargo.toml");
    let line = cargo
        .lines()
        .find(|l| l.starts_with("brocanth-cache "))
        .expect("a brocanth-cache line");
    assert!(!line.contains("postgres"), "{line}");
}

/// The two backends write the same file, so a project takes one.
#[test]
fn two_cache_backends_are_refused() {
    let err = brocanth_cli::testing::generate(
        "my-app",
        &["config", "cache", "cache-memory", "cache-redis"],
        &[&Cache, &CacheMemory],
    )
    .expect_err("two backends");
    assert!(err.contains("cache-memory"), "{err}");
    assert!(err.contains("cache-redis"), "{err}");
}
