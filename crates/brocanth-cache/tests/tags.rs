//! Tags, fragments and the route cache, against a real Postgres.
//!
//! Tags are global, and these tests share one database, so every tag below is
//! built from the test's own namespace. See `tests/common/mod.rs` for the
//! loud skip when there is no database.

#![cfg(feature = "postgres")]

mod common;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use brocanth_cache::{Cache, CacheExt, FetchError, FetchPolicy, Namespace, PgCache, Tag};
use common::{cache, cache_with_manual_clock, clean_up, namespace, MINUTE};
use sqlx::Row as _;

/// A tag no other test uses.
fn tag(ns: &Namespace, name: &str) -> Tag {
    Tag::new(format!("{}#{name}", ns.as_str())).unwrap()
}

/// How many tag rows point into a namespace.
async fn tag_rows(cache: &PgCache, ns: &Namespace) -> i64 {
    sqlx::query("SELECT count(*) FROM brocanth_cache_tags WHERE namespace = $1")
        .bind(ns.as_str())
        .fetch_one(cache.pool())
        .await
        .unwrap()
        .get::<i64, _>(0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_tag_removes_its_entries_in_every_namespace_and_nothing_else() {
    let Some(cache) = cache("one_tag_removes_its_entries").await else {
        return;
    };
    let views = namespace("tag-views");
    let parts = namespace("tag-parts");
    let posts = tag(&views, "posts");

    let page = views.key("post-1").unwrap();
    let sidebar = parts.key("sidebar").unwrap();
    let other = views.key("about").unwrap();
    cache
        .set_tagged(&page, b"page", MINUTE, std::slice::from_ref(&posts))
        .await
        .unwrap();
    cache
        .set_tagged(&sidebar, b"side", MINUTE, std::slice::from_ref(&posts))
        .await
        .unwrap();
    cache.set(&other, b"about", MINUTE).await.unwrap();

    assert_eq!(cache.invalidate_tag(&posts).await.unwrap(), 2);
    assert_eq!(cache.get(&page).await.unwrap(), None);
    assert_eq!(cache.get(&sidebar).await.unwrap(), None);
    assert_eq!(cache.get(&other).await.unwrap(), Some(b"about".to_vec()));
    assert_eq!(tag_rows(&cache, &views).await, 0);

    // An unknown tag removes nothing and is not an error.
    assert_eq!(cache.invalidate_tag(&posts).await.unwrap(), 0);

    clean_up(&cache, &views).await;
    clean_up(&cache, &parts).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tagged_write_replaces_the_tags_of_the_one_before() {
    let Some(cache) = cache("a_tagged_write_replaces_the_tags").await else {
        return;
    };
    let ns = namespace("retag");
    let (a, b) = (tag(&ns, "a"), tag(&ns, "b"));
    let key = ns.key("k").unwrap();

    cache
        .set_tagged(&key, b"one", MINUTE, &[a.clone(), b.clone()])
        .await
        .unwrap();
    cache
        .set_tagged(&key, b"two", MINUTE, std::slice::from_ref(&b))
        .await
        .unwrap();
    assert_eq!(tag_rows(&cache, &ns).await, 1);

    assert_eq!(cache.invalidate_tag(&a).await.unwrap(), 0);
    assert_eq!(cache.get(&key).await.unwrap(), Some(b"two".to_vec()));
    assert_eq!(cache.invalidate_tag(&b).await.unwrap(), 1);
    assert_eq!(cache.get(&key).await.unwrap(), None);

    clean_up(&cache, &ns).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plain_set_keeps_the_old_tags_so_invalidation_errs_toward_a_miss() {
    let Some(cache) = cache("a_plain_set_keeps_the_old_tags").await else {
        return;
    };
    let ns = namespace("plain-after-tagged");
    let old = tag(&ns, "old");
    let key = ns.key("k").unwrap();

    cache
        .set_tagged(&key, b"tagged", MINUTE, std::slice::from_ref(&old))
        .await
        .unwrap();
    cache.set(&key, b"plain", MINUTE).await.unwrap();

    // Documented in `crate::tag`: the newer value goes too. A miss, never a
    // stale hit.
    assert_eq!(cache.invalidate_tag(&old).await.unwrap(), 1);
    assert_eq!(cache.get(&key).await.unwrap(), None);

    clean_up(&cache, &ns).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn several_tags_go_in_one_call_and_an_entry_is_counted_once() {
    let Some(cache) = cache("several_tags_go_in_one_call").await else {
        return;
    };
    let ns = namespace("many-tags");
    let (a, b, c) = (tag(&ns, "a"), tag(&ns, "b"), tag(&ns, "c"));

    let both = ns.key("both").unwrap();
    let only_b = ns.key("only-b").unwrap();
    let only_c = ns.key("only-c").unwrap();
    cache
        .set_tagged(&both, b"1", MINUTE, &[a.clone(), b.clone()])
        .await
        .unwrap();
    cache
        .set_tagged(&only_b, b"2", MINUTE, std::slice::from_ref(&b))
        .await
        .unwrap();
    cache
        .set_tagged(&only_c, b"3", MINUTE, std::slice::from_ref(&c))
        .await
        .unwrap();

    assert_eq!(cache.invalidate_tags(&[a, b]).await.unwrap(), 2);
    assert_eq!(cache.get(&only_c).await.unwrap(), Some(b"3".to_vec()));
    assert_eq!(cache.invalidate_tags(&[]).await.unwrap(), 0);

    clean_up(&cache, &ns).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_sweep_removes_the_tags_of_entries_that_are_gone() {
    let Some((cache, clock)) = cache_with_manual_clock("the_sweep_removes_orphan_tags").await
    else {
        return;
    };
    let ns = namespace("orphan-tags");
    let t = tag(&ns, "t");

    let expiring = ns.key("expiring").unwrap();
    let deleted = ns.key("deleted").unwrap();
    let live = ns.key("live").unwrap();
    cache
        .set_tagged(
            &expiring,
            b"x",
            Duration::from_secs(1),
            std::slice::from_ref(&t),
        )
        .await
        .unwrap();
    cache
        .set_tagged(&deleted, b"x", MINUTE, std::slice::from_ref(&t))
        .await
        .unwrap();
    cache
        .set_tagged(&live, b"x", MINUTE * 10, std::slice::from_ref(&t))
        .await
        .unwrap();
    cache.delete(&deleted).await.unwrap();
    assert_eq!(tag_rows(&cache, &ns).await, 3);

    clock.advance(chrono::Duration::seconds(2));
    brocanth_cache::sweep(&cache, 10_000, 10).await.unwrap();

    assert_eq!(
        tag_rows(&cache, &ns).await,
        1,
        "only the live entry's tag stays"
    );
    assert_eq!(cache.invalidate_tag(&t).await.unwrap(), 1);

    clean_up(&cache, &ns).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fetch_with_tags_stores_them() {
    let Some(cache) = cache("fetch_with_tags_stores_them").await else {
        return;
    };
    let ns = namespace("fetch-tags");
    let t = tag(&ns, "t");
    let key = ns.key("k").unwrap();

    let policy = FetchPolicy::new(MINUTE).tags([t.clone()]);
    let value = cache
        .fetch_with(&key, policy, || async {
            Ok::<_, std::io::Error>(b"v".to_vec())
        })
        .await
        .unwrap();
    assert_eq!(value, b"v");
    assert_eq!(cache.invalidate_tag(&t).await.unwrap(), 1);

    clean_up(&cache, &ns).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fragment_renders_once_until_its_tag_is_invalidated() {
    let Some(cache) = cache("a_fragment_renders_once").await else {
        return;
    };
    let ns = namespace("fragments");
    let posts = [tag(&ns, "posts")];
    let key = ns.key("sidebar").unwrap();
    let renders = AtomicU32::new(0);
    let render = || {
        renders.fetch_add(1, Ordering::SeqCst);
        "<ul><li>post</li></ul>".to_owned()
    };

    let first = cache.fragment(&key, &posts, MINUTE, render).await.unwrap();
    let second = cache.fragment(&key, &posts, MINUTE, render).await.unwrap();
    assert_eq!(first, "<ul><li>post</li></ul>");
    assert_eq!(second, first);
    assert_eq!(renders.load(Ordering::SeqCst), 1);

    cache.invalidate_tag(&posts[0]).await.unwrap();
    cache.fragment(&key, &posts, MINUTE, render).await.unwrap();
    assert_eq!(renders.load(Ordering::SeqCst), 2);

    clean_up(&cache, &ns).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fragment_whose_render_fails_is_not_stored() {
    let Some(cache) = cache("a_fragment_whose_render_fails").await else {
        return;
    };
    let ns = namespace("fragment-failure");
    let key = ns.key("broken").unwrap();

    let failed = cache
        .try_fragment(&key, &[], MINUTE, || {
            Err::<String, _>(std::io::Error::other("template error"))
        })
        .await;
    assert!(matches!(failed, Err(FetchError::Compute(_))));
    assert_eq!(cache.get(&key).await.unwrap(), None);

    clean_up(&cache, &ns).await;
}

/// The route cache over Postgres: a stored page, and its row tag dropping it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_route_cache_stores_in_postgres_and_a_row_tag_drops_the_page() {
    use brocanth::plugin_api::Plugin;
    use brocanth::plugin_app::AppBuilder;
    use brocanth::plugin_body::Body;
    use brocanth::plugin_hooks::{Begin, RequestState, Response};
    use brocanth_cache::route::{RouteCache, RouteCacheConfig, RouteRule, STATUS_HEADER};

    let Some(cache) = cache("the_route_cache_stores_in_postgres").await else {
        return;
    };
    let ns = namespace("routes");
    let row = tag(&ns, "posts:1");
    let cache = Arc::new(cache);
    let app = AppBuilder::new()
        .plugin_hooking(RouteCache::new(
            RouteCacheConfig::new(cache.clone())
                .namespace(ns.clone())
                .rule(RouteRule::prefix("/posts/", MINUTE).tags([row.clone()])),
        ))
        .build()
        .unwrap();

    let runs = AtomicU32::new(0);
    let request = || async {
        let mut request = http::Request::builder()
            .uri("/posts/1")
            .body(Vec::new())
            .unwrap();
        let mut state = RequestState::new();
        let outcome = match app.begin_async(&mut request, &mut state).await {
            Begin::Dispatch => {
                runs.fetch_add(1, Ordering::SeqCst);
                Ok(Response::new(Body::from("post one")))
            }
            Begin::Answered(outcome) => outcome,
        };
        let response = app.finish(&mut request, &mut state, outcome);
        response.headers()[STATUS_HEADER]
            .to_str()
            .unwrap()
            .to_owned()
    };

    assert_eq!(request().await, "miss");
    // The store is spawned after the response; wait for the row.
    let key = ns.key("GET /posts/1").unwrap();
    for _ in 0..200 {
        if cache.get(&key).await.unwrap().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(request().await, "hit");
    assert_eq!(runs.load(Ordering::SeqCst), 1);

    let path = brocanth_cache::route::path_tag("/posts/1").unwrap();
    assert_eq!(tag_rows(&cache, &ns).await, 2, "the rule's tag and {path}");
    assert_eq!(cache.invalidate_tag(&row).await.unwrap(), 1);
    assert_eq!(request().await, "miss");
    assert_eq!(runs.load(Ordering::SeqCst), 2);

    clean_up(&cache, &ns).await;
}

/// A model change reaches the cache as two tag invalidations.
#[cfg(feature = "events")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_row_change_invalidates_its_table_and_row_tags() {
    use brocanth_cache::TagInvalidator;
    use brocanth_events::{ChangeSink, RowChange};

    let Some(cache) = cache("a_row_change_invalidates").await else {
        return;
    };
    let ns = namespace("model-changes");
    // The invalidator spells tags from the collection name, so the
    // collection is made unique to this run instead.
    let table = format!("posts_{}", ns.as_str().replace(['.', ':', '-'], "_"));
    let page = ns.key("post").unwrap();
    let index = ns.key("index").unwrap();
    let unrelated = ns.key("unrelated").unwrap();
    cache
        .set_tagged(&page, b"p", MINUTE, &[Tag::row(&table, 42).unwrap()])
        .await
        .unwrap();
    cache
        .set_tagged(&index, b"i", MINUTE, &[Tag::table(&table).unwrap()])
        .await
        .unwrap();
    cache
        .set_tagged(&unrelated, b"u", MINUTE, &[Tag::row(&table, 7).unwrap()])
        .await
        .unwrap();

    let cache = Arc::new(cache);
    let sink = TagInvalidator::new(cache.clone()).only([table.clone()]);
    assert!(!sink.wants("users"));
    sink.publish(&RowChange::updated(table.as_str(), 42_i64, &["title"][..]));

    for _ in 0..200 {
        if cache.get(&page).await.unwrap().is_none() && cache.get(&index).await.unwrap().is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(cache.get(&page).await.unwrap(), None);
    assert_eq!(cache.get(&index).await.unwrap(), None);
    assert_eq!(cache.get(&unrelated).await.unwrap(), Some(b"u".to_vec()));

    clean_up(&cache, &ns).await;
}
