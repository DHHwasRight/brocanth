//! The conformance suite, against a real Redis or Valkey.
//!
//! Skipped, and loudly, when `BROCANTH_CACHE_TEST_REDIS` names no server.
//! A temporary one:
//!
//! ```sh
//! docker run -d --rm --name brocanth-redis -p 6390:6379 redis:7-alpine
//! BROCANTH_CACHE_TEST_REDIS=redis://localhost:6390 cargo test -p brocanth-cache-redis
//! docker stop brocanth-redis
//! ```
//!
//! `valkey/valkey` works the same way. Each run writes under a prefix of
//! its own and the suite's names are unique per run, so the tests share one
//! server and clean nothing.

use std::sync::Arc;

use brocanth_cache::{Cache, Namespace};
use brocanth_cache_redis::RedisCache;

fn url() -> Option<String> {
    std::env::var("BROCANTH_CACHE_TEST_REDIS")
        .ok()
        .filter(|url| !url.trim().is_empty())
}

/// Writes past libtest's capture, so a skip shows in a passing run.
fn shout(message: &str) {
    use std::io::Write as _;
    let _ = std::io::stderr().write_all(message.as_bytes());
}

async fn cache(test: &str) -> Option<RedisCache> {
    let Some(url) = url() else {
        shout(&format!(
            "*** SKIPPED (no Redis): {test}\n\
             help: set BROCANTH_CACHE_TEST_REDIS=redis://localhost:6390 with a \
             `redis:7-alpine` container on that port\n"
        ));
        return None;
    };
    let cache = RedisCache::connect(&url)
        .await
        .expect("connecting to Redis");
    Some(cache.with_prefix(format!("test:{}:{test}", std::process::id())))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_redis_cache_keeps_every_promise_of_the_trait() {
    let Some(cache) = cache("conformance").await else {
        return;
    };
    brocanth_cache_conformance::assert_conformant("redis", Arc::new(cache)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_prefixes_do_not_meet_on_one_server() {
    let (Some(a), Some(b)) = (cache("prefix-a").await, cache("prefix-b").await) else {
        return;
    };
    let ns = Namespace::new("shared").unwrap();
    let key = ns.key("k").unwrap();
    let minute = std::time::Duration::from_secs(60);
    a.set(&key, b"from a", minute).await.unwrap();
    assert_eq!(b.get(&key).await.unwrap(), None);
    assert_eq!(b.clear(&ns).await.unwrap(), 0);
    assert_eq!(a.get(&key).await.unwrap(), Some(b"from a".to_vec()));
    assert_eq!(a.clear(&ns).await.unwrap(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_url_that_is_not_redis_says_what_to_write() {
    let Err(e) = RedisCache::open("postgres://localhost/app") else {
        panic!("a Postgres URL is not a Redis URL");
    };
    let said = e.to_string();
    assert!(said.contains("help:"), "{said}");
}
