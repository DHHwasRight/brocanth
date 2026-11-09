//! The `cache-redis` module, written into a project by the CLI's generator.
//!
//! Bless with `BROCANTH_BLESS=1`; build the project too with
//! `BROCANTH_BUILD=1`.

use brocanth_cache_redis::scaffold::CacheRedis;
use brocanth_scaffold::testing;

/// With `db` and `admin`: the cache is still on Redis, with its own `url`,
/// `brocanth-cache` links no SQL driver, and the admin gets the cache tab.
#[test]
fn cache_redis_writes_the_plugin_over_redis() {
    let picks = &["config", "db", "cache", "cache-redis", "admin"];
    let files = brocanth_cli::testing::generate("my-app", picks, &[&CacheRedis])
        .unwrap_or_else(|e| panic!("{e}"));
    testing::golden(
        format!(
            "{}/tests/golden/cache-redis.txt",
            env!("CARGO_MANIFEST_DIR")
        ),
        &testing::snapshot(&files),
    );
    testing::assert_formatted(&files);
    let file = |path: &str| {
        files
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, text)| text.as_str())
            .unwrap_or_else(|| panic!("no {path} in the project"))
    };
    let plugin = file("src/plugins/cache/mod.rs");
    assert!(plugin.contains("RedisCache::open(&self.settings.url)"));
    assert!(!plugin.contains("Sweeper"));
    let cargo = file("Cargo.toml");
    assert!(cargo.contains("brocanth-cache-redis"));
    let line = cargo
        .lines()
        .find(|l| l.starts_with("brocanth-cache "))
        .expect("a brocanth-cache line");
    assert!(!line.contains("postgres"), "{line}");
    assert!(file("src/plugins/mod.rs").contains("CachePanel::installed()"));
    if testing::building() {
        let built = brocanth_cli::testing::generate_for_build("my-app", picks, &[&CacheRedis])
            .unwrap_or_else(|e| panic!("{e}"));
        testing::build(&built).unwrap_or_else(|e| panic!("{e}"));
    }
}
