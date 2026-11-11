//! The suite holds for the in-memory backend. That also proves the cases
//! agree with each other.

use std::sync::Arc;

use brocanth_cache::MemoryCache;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_case_holds_for_the_memory_cache() {
    let report = brocanth_cache_conformance::run("memory", Arc::new(MemoryCache::new())).await;
    report.assert_ok();
    assert_eq!(
        report.outcomes.len(),
        brocanth_cache_conformance::case_names().len()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn the_suite_runs_on_a_current_thread_runtime_too() {
    brocanth_cache_conformance::assert_conformant("memory", Arc::new(MemoryCache::new())).await;
}
