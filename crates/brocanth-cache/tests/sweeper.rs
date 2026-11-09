//! The plugin's periodic sweep: it runs after `start` and stops at
//! `shutdown`.
//!
//! A test double again, for the reason `tests/panel.rs` gives: the question
//! is "did the plugin call `purge_expired`, and when did it stop", which
//! Postgres has no part in. `tests/postgres.rs` has the test that the
//! scheduled sweep removes real rows.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_shutdown::ShutdownReason;
use brocanth_cache::{Cache, CacheConfig, CachePlugin, Key, Namespace, Result, Tag};

/// A cache that counts the sweeps. Nothing else is asked of it.
#[derive(Default)]
struct Counter {
    purges: AtomicU64,
}

impl Counter {
    fn purges(&self) -> u64 {
        self.purges.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Cache for Counter {
    fn max_value_bytes(&self) -> usize {
        1024
    }
    async fn migrate(&self) -> Result<()> {
        Ok(())
    }
    async fn get(&self, _key: &Key) -> Result<Option<Vec<u8>>> {
        unimplemented!("the sweep reads no entry")
    }
    async fn set(&self, _key: &Key, _value: &[u8], _ttl: Duration) -> Result<()> {
        unimplemented!("the sweep writes no entry")
    }
    async fn set_tagged(&self, _: &Key, _: &[u8], _: Duration, _: &[Tag]) -> Result<()> {
        unimplemented!("the sweep writes no entry")
    }
    async fn invalidate_tags(&self, _tags: &[Tag]) -> Result<u64> {
        unimplemented!("the sweep invalidates no tag")
    }
    async fn add(&self, _key: &Key, _value: &[u8], _ttl: Duration) -> Result<bool> {
        unimplemented!("the sweep writes no entry")
    }
    async fn delete(&self, _key: &Key) -> Result<bool> {
        unimplemented!("the sweep deletes no single entry")
    }
    async fn clear(&self, _namespace: &Namespace) -> Result<u64> {
        unimplemented!("the sweep clears no namespace")
    }
    async fn lease(&self, _key: &Key, _ttl: Duration) -> Result<bool> {
        unimplemented!("the sweep takes no lease")
    }
    async fn release_lease(&self, _key: &Key) -> Result<()> {
        unimplemented!("the sweep takes no lease")
    }
    async fn purge_expired(&self, _limit: u64) -> Result<u64> {
        self.purges.fetch_add(1, Ordering::SeqCst);
        Ok(0)
    }
}

/// Waits until `cond` holds, for at most five seconds.
async fn eventually(cond: impl Fn() -> bool) -> bool {
    for _ in 0..500 {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    cond()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_plugin_sweeps_after_start_and_stops_at_shutdown() {
    let counter = Arc::new(Counter::default());
    let config = CacheConfig::new(counter.clone()).sweep_every(Duration::from_millis(10));
    let mut app = AppBuilder::new()
        .plugin(CachePlugin::new(config))
        .build()
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(counter.purges(), 0, "no sweep before `start`");

    app.start().unwrap();
    assert!(
        eventually(|| counter.purges() >= 2).await,
        "the plugin swept {} times after `start`",
        counter.purges()
    );

    let report = app.shutdown(ShutdownReason::Requested, None);
    assert!(report.is_clean(), "{report:?}");
    // The abort lands at the task's next await. Let it, then count.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let after = counter.purges();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(counter.purges(), after, "a sweep ran after `shutdown`");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_zero_interval_starts_no_sweep() {
    let counter = Arc::new(Counter::default());
    let config = CacheConfig::new(counter.clone()).sweep_every(Duration::ZERO);
    let mut app = AppBuilder::new()
        .plugin(CachePlugin::new(config))
        .build()
        .unwrap();
    app.start().unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(counter.purges(), 0);
    let _ = app.shutdown(ShutdownReason::Requested, None);
}
