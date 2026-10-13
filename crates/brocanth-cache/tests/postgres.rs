//! The whole suite, against a real Postgres.
//!
//! Every interesting decision in this crate is in SQL — the `ON CONFLICT …
//! WHERE` that makes a lease atomic, the `WITH ORDINALITY` join, the `ctid`
//! delete, `UNLOGGED` — so there is nothing worth testing that does not touch a
//! server. See `tests/common/mod.rs` for how that is arranged, and why these
//! skip loudly rather than quietly passing.
//!
//! ```sh
//! BROCANTH_CACHE_TEST_POSTGRES=postgres://localhost/brocanth_cache_test \
//!   cargo test -p brocanth-cache
//! ```

#![cfg(feature = "postgres")]

mod common;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use brocanth_cache::{sweep, Cache, CacheError, CacheExt, FetchError, FetchPolicy, Namespace};
use common::{cache, cache_with_manual_clock, clean_up, namespace, MINUTE};

// ---------------------------------------------------------------------------
// It stores things
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_value_round_trips() {
    let Some(cache) = cache("a_value_round_trips").await else {
        return;
    };
    let ns = namespace("round-trip");
    let key = ns.key("greeting").unwrap();

    assert_eq!(cache.get(&key).await.unwrap(), None, "nothing stored yet");

    // Arbitrary bytes, including a NUL and something that is not UTF-8, because
    // the contract is bytes and not a string.
    let value: Vec<u8> = vec![0x00, 0xff, 0xfe, b'h', b'i', 0x80];
    cache.set(&key, &value, MINUTE).await.unwrap();

    assert_eq!(cache.get(&key).await.unwrap(), Some(value.clone()));

    // A second set replaces.
    cache.set(&key, b"second", MINUTE).await.unwrap();
    assert_eq!(cache.get(&key).await.unwrap(), Some(b"second".to_vec()));

    // Delete says whether anything was there, and is idempotent.
    assert!(cache.delete(&key).await.unwrap());
    assert!(!cache.delete(&key).await.unwrap());
    assert_eq!(cache.get(&key).await.unwrap(), None);

    clean_up(&cache, &ns).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn several_keys_come_back_in_the_order_they_were_asked_for() {
    let Some(cache) = cache("several_keys_come_back_in_order").await else {
        return;
    };
    let ns = namespace("get-many");
    let keys: Vec<_> = (0..5).map(|i| ns.key(format!("k{i}")).unwrap()).collect();

    // Store every other one, so the misses are interleaved with the hits and a
    // backend that lost the ordinality would be caught.
    for (i, key) in keys.iter().enumerate() {
        if i % 2 == 0 {
            cache
                .set(key, format!("v{i}").as_bytes(), MINUTE)
                .await
                .unwrap();
        }
    }

    let got = cache.get_many(&keys).await.unwrap();
    assert_eq!(got.len(), keys.len());
    for (i, slot) in got.iter().enumerate() {
        if i % 2 == 0 {
            assert_eq!(slot.as_deref(), Some(format!("v{i}").as_bytes()), "at {i}");
        } else {
            assert_eq!(slot.as_deref(), None, "at {i}");
        }
    }

    assert!(cache.get_many(&[]).await.unwrap().is_empty());
    clean_up(&cache, &ns).await;
}

// ---------------------------------------------------------------------------
// Expiry
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_expired_value_is_not_returned() {
    // A manual clock, so this is exact rather than a sleep that is flaky on a
    // loaded machine. See `src/clock.rs` for why the clock is a parameter.
    let Some((cache, clock)) = cache_with_manual_clock("an_expired_value_is_not_returned").await
    else {
        return;
    };
    let ns = namespace("expiry");
    let key = ns.key("short-lived").unwrap();

    cache
        .set(&key, b"here", Duration::from_secs(30))
        .await
        .unwrap();
    assert_eq!(cache.get(&key).await.unwrap(), Some(b"here".to_vec()));

    clock.advance(chrono::Duration::seconds(29));
    assert_eq!(cache.get(&key).await.unwrap(), Some(b"here".to_vec()));

    clock.advance(chrono::Duration::seconds(2));
    assert_eq!(cache.get(&key).await.unwrap(), None, "past its TTL");

    // And through `get_many`, which has its own predicate.
    assert_eq!(
        cache.get_many(std::slice::from_ref(&key)).await.unwrap(),
        vec![None]
    );

    // The row is still there — expiry is lazy, enforced by the read — until
    // something sweeps it. That is the whole point of `sweep`: correctness
    // never depends on it running.
    let swept = sweep(&cache, 1000, 10).await.unwrap();
    assert!(swept >= 1, "the sweep should have taken the dead row");
    assert_eq!(cache.get(&key).await.unwrap(), None);

    clean_up(&cache, &ns).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_expired_row_is_a_free_slot_rather_than_an_obstacle() {
    let Some((cache, clock)) = cache_with_manual_clock("an_expired_row_is_a_free_slot").await
    else {
        return;
    };
    let ns = namespace("add");
    let key = ns.key("once").unwrap();

    assert!(cache
        .add(&key, b"first", Duration::from_secs(30))
        .await
        .unwrap());
    assert!(
        !cache
            .add(&key, b"second", Duration::from_secs(30))
            .await
            .unwrap(),
        "a live entry blocks `add`"
    );
    assert_eq!(cache.get(&key).await.unwrap(), Some(b"first".to_vec()));

    clock.advance(chrono::Duration::seconds(31));
    assert!(
        cache
            .add(&key, b"third", Duration::from_secs(30))
            .await
            .unwrap(),
        "an expired entry does not block `add`; without the `WHERE` on the \
         conflict arm this key would be unusable forever"
    );
    assert_eq!(cache.get(&key).await.unwrap(), Some(b"third".to_vec()));

    clean_up(&cache, &ns).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_zero_ttl_is_refused() {
    let Some(cache) = cache("a_zero_ttl_is_refused").await else {
        return;
    };
    let ns = namespace("zero-ttl");
    let key = ns.key("k").unwrap();

    assert!(matches!(
        cache.set(&key, b"v", Duration::ZERO).await,
        Err(CacheError::InvalidTtl(_))
    ));
    assert_eq!(cache.get(&key).await.unwrap(), None);

    clean_up(&cache, &ns).await;
}

// ---------------------------------------------------------------------------
// Namespaces
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_namespaces_cannot_see_each_other() {
    let Some(cache) = cache("two_namespaces_cannot_see_each_other").await else {
        return;
    };
    let views = namespace("iso-views");
    let sessions = namespace("iso-sessions");

    let a = views.key("shared-name").unwrap();
    let b = sessions.key("shared-name").unwrap();

    cache.set(&a, b"from views", MINUTE).await.unwrap();
    cache.set(&b, b"from sessions", MINUTE).await.unwrap();

    assert_eq!(cache.get(&a).await.unwrap(), Some(b"from views".to_vec()));
    assert_eq!(
        cache.get(&b).await.unwrap(),
        Some(b"from sessions".to_vec())
    );

    // Clearing one leaves the other alone, which is the operation a deploy
    // uses to invalidate a class of entries.
    let gone = cache.clear(&views).await.unwrap();
    assert_eq!(gone, 1);
    assert_eq!(cache.get(&a).await.unwrap(), None);
    assert_eq!(
        cache.get(&b).await.unwrap(),
        Some(b"from sessions".to_vec()),
        "clearing one namespace must not touch another"
    );

    // Deleting in one namespace does not reach into the other either.
    let a2 = views.key("only-here").unwrap();
    let b2 = sessions.key("only-here").unwrap();
    cache.set(&b2, b"stay", MINUTE).await.unwrap();
    assert!(!cache.delete(&a2).await.unwrap());
    assert_eq!(cache.get(&b2).await.unwrap(), Some(b"stay".to_vec()));

    clean_up(&cache, &views).await;
    clean_up(&cache, &sessions).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_version_in_the_name_invalidates_without_deleting_anything() {
    let Some(cache) = cache("a_version_in_the_name_invalidates").await else {
        return;
    };
    let base = namespace("versioned");
    let v1 = Namespace::versioned(base.as_str(), "v1").unwrap();
    let v2 = Namespace::versioned(base.as_str(), "v2").unwrap();

    cache
        .set(&v1.key("home").unwrap(), b"old render", MINUTE)
        .await
        .unwrap();

    // The deploy bumps the version. Nothing is deleted and nothing reads the
    // old entry again.
    assert_eq!(cache.get(&v2.key("home").unwrap()).await.unwrap(), None);
    assert_eq!(
        cache.get(&v1.key("home").unwrap()).await.unwrap(),
        Some(b"old render".to_vec()),
        "the old rows are still there; they are just unreachable"
    );

    clean_up(&cache, &v1).await;
    clean_up(&cache, &v2).await;
}

// ---------------------------------------------------------------------------
// The size cap
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_value_over_the_cap_is_rejected() {
    let Some(cache) = cache("a_value_over_the_cap_is_rejected").await else {
        return;
    };
    let cache = cache.with_max_value_bytes(1024);
    let ns = namespace("too-large");
    let key = ns.key("big").unwrap();

    let fits = vec![b'x'; 1024];
    cache.set(&key, &fits, MINUTE).await.unwrap();
    assert_eq!(cache.get(&key).await.unwrap(), Some(fits));

    let over = vec![b'x'; 1025];
    match cache.set(&key, &over, MINUTE).await {
        Err(CacheError::TooLarge { bytes, limit, .. }) => {
            assert_eq!((bytes, limit), (1025, 1024));
        }
        other => panic!("expected TooLarge, got {other:?}"),
    }
    // The failed write left the previous value alone rather than half-storing.
    assert_eq!(cache.get(&key).await.unwrap().map(|v| v.len()), Some(1024));

    // `add` checks it too, and so does `fetch`, which is where it matters most:
    // a `fetch` that silently skipped the store would recompute on every call
    // and never say so.
    assert!(matches!(
        cache.add(&ns.key("other").unwrap(), &over, MINUTE).await,
        Err(CacheError::TooLarge { .. })
    ));

    let result: Result<Vec<u8>, FetchError<std::io::Error>> = cache
        .fetch(&ns.key("computed").unwrap(), MINUTE, || async {
            Ok(over.clone())
        })
        .await;
    assert!(
        matches!(result, Err(FetchError::Cache(CacheError::TooLarge { .. }))),
        "fetch must report a value it could not store"
    );

    clean_up(&cache, &ns).await;
}

// ---------------------------------------------------------------------------
// The stampede
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fetch_computes_exactly_once_under_concurrency() {
    let Some(cache) = cache("fetch_computes_exactly_once").await else {
        return;
    };
    let cache = Arc::new(cache);
    let ns = namespace("stampede");
    let key = Arc::new(ns.key("expensive").unwrap());

    const CALLERS: u32 = 32;
    let runs = Arc::new(AtomicU32::new(0));

    let mut tasks = Vec::new();
    for _ in 0..CALLERS {
        let cache = cache.clone();
        let key = key.clone();
        let runs = runs.clone();
        tasks.push(tokio::spawn(async move {
            let policy = FetchPolicy::new(MINUTE)
                .lease_for(Duration::from_secs(30))
                .wait_for(Duration::from_secs(20));
            cache
                .fetch_with(&key, policy, || async move {
                    runs.fetch_add(1, Ordering::SeqCst);
                    // Long enough that every other caller is certainly waiting
                    // on the lease rather than arriving after the value landed.
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    Ok::<_, std::io::Error>(b"computed once".to_vec())
                })
                .await
                .expect("fetch")
        }));
    }

    for task in tasks {
        assert_eq!(task.await.unwrap(), b"computed once".to_vec());
    }

    assert_eq!(
        runs.load(Ordering::SeqCst),
        1,
        "the lease must collapse {CALLERS} concurrent misses into one computation"
    );
    assert_eq!(
        cache.get(&key).await.unwrap(),
        Some(b"computed once".to_vec())
    );

    clean_up(&cache, &ns).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_computation_gives_the_lease_back_at_once() {
    let Some(cache) = cache("a_failed_computation_gives_the_lease_back").await else {
        return;
    };
    let ns = namespace("fetch-failure");
    let key = ns.key("flaky").unwrap();

    let failed: Result<Vec<u8>, FetchError<std::io::Error>> = cache
        .fetch(&key, MINUTE, || async {
            Err(std::io::Error::other("the upstream was down"))
        })
        .await;
    assert!(matches!(failed, Err(FetchError::Compute(_))));

    // The lease is gone rather than sitting there for its thirty seconds, so
    // the next caller starts immediately instead of waiting out a computation
    // that is not happening.
    assert!(
        cache.lease(&key, Duration::from_secs(5)).await.unwrap(),
        "release_lease should have freed the key"
    );
    cache.release_lease(&key).await.unwrap();

    // And the second caller gets a value.
    let ok: Result<Vec<u8>, FetchError<std::io::Error>> = cache
        .fetch(&key, MINUTE, || async { Ok(b"worked".to_vec()) })
        .await;
    assert_eq!(ok.ok(), Some(b"worked".to_vec()));

    clean_up(&cache, &ns).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lease_is_never_mistaken_for_a_value() {
    let Some(cache) = cache("a_lease_is_never_mistaken_for_a_value").await else {
        return;
    };
    let ns = namespace("lease");
    let key = ns.key("held").unwrap();

    assert!(cache.lease(&key, MINUTE).await.unwrap());
    assert!(
        !cache.lease(&key, MINUTE).await.unwrap(),
        "one holder at a time"
    );
    assert_eq!(
        cache.get(&key).await.unwrap(),
        None,
        "a lease is a claim, not a value"
    );
    assert_eq!(
        cache.get_many(std::slice::from_ref(&key)).await.unwrap(),
        vec![None]
    );

    // Releasing a lease never removes somebody else's value.
    cache.set(&key, b"a real value", MINUTE).await.unwrap();
    cache.release_lease(&key).await.unwrap();
    assert_eq!(
        cache.get(&key).await.unwrap(),
        Some(b"a real value".to_vec()),
        "release_lease must not delete a stored value"
    );

    clean_up(&cache, &ns).await;
}

// ---------------------------------------------------------------------------
// The table itself
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_table_is_unlogged_and_migrate_is_idempotent() {
    let Some(cache) = cache("the_table_is_unlogged").await else {
        return;
    };
    // `cache()` already migrated; a second run must be a no-op rather than an
    // error, because it runs on every boot.
    cache.migrate().await.unwrap();

    assert_eq!(
        cache.is_unlogged().await.unwrap(),
        Some(true),
        "the whole design rests on this table being UNLOGGED; a logged one \
         works and costs what this crate exists to avoid"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn several_processes_can_migrate_at_the_same_instant() {
    // The regression this suite actually found. `CREATE TABLE IF NOT EXISTS`
    // is not atomic: without the advisory lock in `sql::MIGRATION_LOCK` the
    // losers fail with a duplicate key on `pg_type_typname_nsp_index`, which
    // is what a rolling deploy looks like from the inside.
    let Some(cache) = cache("several_processes_can_migrate_at_once").await else {
        return;
    };
    let cache = Arc::new(cache);

    let mut tasks = Vec::new();
    for _ in 0..8 {
        let cache = cache.clone();
        tasks.push(tokio::spawn(async move { cache.migrate().await }));
    }
    for task in tasks {
        task.await
            .unwrap()
            .expect("a concurrent migrate must not fail");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stats_answer_the_questions_an_operator_asks() {
    let Some(cache) = cache("stats_answer_the_questions").await else {
        return;
    };
    let ns = namespace("stats");
    cache
        .set(&ns.key("a").unwrap(), b"x", MINUTE)
        .await
        .unwrap();

    let stats = cache.stats().await.unwrap();
    assert!(stats.entries >= 1);
    assert!(stats.bytes > 0);
    // `expired` and `dead_tuples` are counts, not errors; they just have to
    // come back rather than blow up on an empty `pg_stat_user_tables` row.
    let _ = (stats.expired, stats.dead_tuples, stats.last_vacuum);

    clean_up(&cache, &ns).await;
}
