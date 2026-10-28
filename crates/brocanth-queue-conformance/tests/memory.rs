//! The suite, checked against the reference backend.
//!
//! A conformance suite that has never passed proves nothing, so it runs here
//! against the in-process backend twice: once unbounded, once declaring the
//! limits a managed queue declares. The second run is the one that matters.
//! The same storage, two capability sets, and the only difference allowed in
//! the report is which cases were skipped.

use std::sync::Arc;

use brocanth_queue::DynBackend;
use brocanth_queue_memory::Memory;

#[tokio::test(flavor = "multi_thread")]
async fn the_reference_backend_conforms() {
    let b: Arc<dyn DynBackend> = Arc::new(Memory::new());
    brocanth_queue_conformance::assert_conformant(b).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_same_store_conforms_with_a_managed_queue_s_limits() {
    let b: Arc<dyn DynBackend> = Arc::new(Memory::sqs_shaped());
    brocanth_queue_conformance::assert_conformant(b).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn every_case_runs_under_one_capability_set_or_the_other() {
    // A gated case that no backend ever runs is a case that proves nothing.
    // Between an unbounded store and one declaring a managed queue's limits,
    // each case must run at least once.
    let full = brocanth_queue_conformance::run(Arc::new(Memory::new())).await;
    let narrow = brocanth_queue_conformance::run(Arc::new(Memory::sqs_shaped())).await;
    assert!(full.failures().is_empty(), "{full}");
    assert!(narrow.failures().is_empty(), "{narrow}");

    let dead: Vec<&str> = full
        .outcomes
        .iter()
        .zip(narrow.outcomes.iter())
        .inspect(|(a, b)| assert_eq!(a.name, b.name, "both runs must keep case order"))
        .filter(|(a, b)| {
            matches!(a.status, brocanth_queue_conformance::Status::Skip(_))
                && matches!(b.status, brocanth_queue_conformance::Status::Skip(_))
        })
        .map(|(a, _)| a.name)
        .collect();
    assert!(
        dead.is_empty(),
        "these cases were skipped by both capability sets, so nothing \
         exercises them: {dead:?}"
    );

    // The gates do something: the two runs do not skip the same set.
    let skips = |r: &brocanth_queue_conformance::Report| -> Vec<&'static str> {
        r.skipped().into_iter().map(|o| o.name).collect()
    };
    assert_ne!(
        skips(&full),
        skips(&narrow),
        "both capability sets skipped exactly the same cases, so the gates \
         are not reading the capabilities"
    );
}
