//! `cache-redis` -- the cache on Redis or Valkey, as a child of `cache`.
//!
//! The module writes `src/plugins/cache/mod.rs` through
//! `brocanth_cache::scaffold::write_plugin`, so the handle, the readiness
//! shape and the registration are the ones every cache backend writes. The
//! text that names `RedisCache` lives here, beside it.
//!
//! The parent then writes no plugin file, and `brocanth-cache` loses its
//! `postgres` feature: a project on Redis links no SQL driver for its cache.
//!
//! Compiled only under the `scaffold` feature, which is off.

use brocanth_cache::scaffold::{url_setting, write_plugin, Backend, CONFIGURE_URL};
use brocanth_scaffold::{Category, Dep, Manifest, Module, Project, Scaffold};

/// The cache on Redis or Valkey, as a choice under `cache`.
pub struct CacheRedis;

impl Module for CacheRedis {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "cache-redis",
            "the cache on Redis or Valkey instead of Postgres; one more server to run",
        )
        .category(Category::Enhancer)
        .part_of_but_off("cache")
        .because("it is a backend for the cache, so there is nothing to store in it without one")
        .asked_in("data", "")
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .krate(Dep::brocanth("brocanth-cache-redis").note("the cache's Redis backend"))
        .after("cache")
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        write_plugin(
            s,
            project,
            Backend {
                doc: DOC,
                cache_items: &[],
                uses: "use brocanth_cache_redis::RedisCache;\n",
                settings: vec![url_setting(
                    "redis://localhost:6379",
                    "The Redis or Valkey URL the cache lives in.",
                )],
                fields: concat!(
                    "    /// The Redis or Valkey URL the cache lives in: one server, a primary,\n",
                    "    /// or Sentinel. Not Redis Cluster: see `brocanth_cache_redis`.\n",
                    "    pub url: String,\n",
                ),
                defaults: "            url: \"redis://localhost:6379\".to_string(),\n",
                configure: CONFIGURE_URL,
                open: OPEN,
                ready: READY,
                sweep: false,
            },
        );
    }
}

const DOC: &str = "\
//! The cache is a Redis or Valkey server. It costs one more server to run,
//! back up and watch, and it is the right trade when the cache is a workload
//! of its own: many writes a second, reads sent to replicas, or a need to
//! evict under `maxmemory`. The server expires entries by itself, so there is
//! no sweep and no table to create.
";

const OPEN: &str = r#"        // The connection opens on the first command, so nothing here waits.
        // Its task is spawned on the runtime, so it needs the runtime entered.
        let runtime = tokio::runtime::Handle::try_current().map_err(|e| {
            PluginError::new(format!(
                "[cache] needs a Tokio runtime at startup to connect ({e})\n\
                 help: build the application inside the runtime that serves it"
            ))
        })?;
        let cache = {
            let _entered = runtime.enter();
            RedisCache::open(&self.settings.url)
                .map_err(|e| PluginError::new(format!("[cache] {e}")))?
        };
        let cache: Arc<dyn brocanth_cache::Cache> = Arc::new(cache);
"#;

const READY: &str = r#"        // Ready when one read answers. A miss is a pass: the question is
        // whether the server answers. `serve` awaits this once before it
        // listens, so an unreachable server stops the start there.
        let probe = Arc::clone(&cache);
        lc.ready_when(move || {
            let probe = Arc::clone(&probe);
            async move {
                let key = brocanth_cache::Namespace::new("brocanth.readyz")
                    .and_then(|ns| brocanth_cache::Key::new(ns, "probe"))
                    .map_err(|e| e.to_string())?;
                probe.get(&key).await.map(drop).map_err(|e| {
                    format!("{e}\nhelp: check `[cache] url`, and that the Redis server runs")
                })
            }
        });
"#;
