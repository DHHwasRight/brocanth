//! The conformance suite, on every backend this crate ships.
//!
//! `MemoryCache` runs it always. `PgCache` runs it when a database is set
//! (see `tests/common/mod.rs`), and skips loudly otherwise.

mod common;

use std::sync::Arc;

use brocanth_cache::MemoryCache;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_memory_cache_keeps_every_promise_of_the_trait() {
    brocanth_cache_conformance::assert_conformant("memory", Arc::new(MemoryCache::new())).await;
}

#[cfg(feature = "postgres")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_postgres_cache_keeps_every_promise_of_the_trait() {
    let Some(cache) = common::cache("the_postgres_cache_keeps_every_promise_of_the_trait").await
    else {
        return;
    };
    brocanth_cache_conformance::assert_conformant("postgres", Arc::new(cache)).await;
}
