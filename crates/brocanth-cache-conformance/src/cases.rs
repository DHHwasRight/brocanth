//! The cases. Each one is a promise from `brocanth_cache::Cache` or
//! `CacheExt`, written as code.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use brocanth_cache::{CacheError, CacheExt, FetchError, FetchPolicy, Namespace};

use crate::Ctx;

type CaseFuture<'a> = Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

/// One case: a name and the code that checks it.
pub(crate) struct Case {
    pub(crate) name: &'static str,
    pub(crate) run: for<'a> fn(&'a Ctx) -> CaseFuture<'a>,
}

/// A TTL that runs out during a case.
const SHORT: Duration = Duration::from_millis(400);
/// Long enough for [`SHORT`] to be over on any machine.
const PAST_SHORT: Duration = Duration::from_millis(900);
/// A TTL that does not run out during a case.
const MINUTE: Duration = Duration::from_secs(60);

/// Fails the case with a message when the condition is false.
macro_rules! ensure {
    ($cond:expr, $($why:tt)+) => {
        if !$cond {
            return Err(format!($($why)+));
        }
    };
}

/// Fails the case when the two sides differ.
macro_rules! same {
    ($left:expr, $right:expr, $($why:tt)+) => {{
        let (left, right) = (&$left, &$right);
        if left != right {
            return Err(format!("{}: got {:?}, wanted {:?}", format!($($why)+), left, right));
        }
    }};
}

/// Turns a cache error into a case failure that names the step.
fn step<T>(what: &str, result: Result<T, CacheError>) -> Result<T, String> {
    result.map_err(|e| format!("{what}: {e}"))
}

macro_rules! case {
    ($name:ident) => {
        Case {
            name: stringify!($name),
            run: |cx| Box::pin($name(cx)),
        }
    };
}

pub(crate) fn all() -> Vec<Case> {
    vec![
        case!(migrate_is_idempotent),
        case!(a_value_round_trips_as_bytes),
        case!(get_many_keeps_the_order_of_the_keys),
        case!(an_expired_value_is_a_miss),
        case!(add_stores_only_when_nothing_live_is_there),
        case!(a_zero_ttl_is_refused),
        case!(namespaces_do_not_see_each_other),
        case!(a_value_over_the_cap_is_refused),
        case!(a_lease_is_exclusive_and_is_not_a_value),
        case!(a_lease_expires),
        case!(releasing_a_lease_keeps_a_stored_value),
        case!(fetch_computes_once_under_concurrency),
        case!(a_failed_fetch_gives_the_lease_back),
        case!(fetch_reports_a_value_it_cannot_store),
        case!(a_tag_removes_its_entries_in_every_namespace),
        case!(a_tagged_write_replaces_the_earlier_tags),
        case!(a_plain_set_keeps_the_earlier_tags),
        case!(several_tags_count_an_entry_once),
        case!(fetch_with_tags_stores_them),
        case!(a_fragment_renders_once_until_its_tag_goes),
        case!(a_failed_fragment_is_not_stored),
        case!(purge_expired_keeps_live_entries),
        case!(a_rewrite_sets_a_new_ttl),
        case!(a_ttl_under_a_millisecond_is_accepted_and_soon_gone),
        case!(a_ttl_past_the_end_of_time_is_refused),
        case!(deleting_what_is_gone_is_not_an_error),
        case!(an_empty_value_is_a_value),
        case!(a_large_value_round_trips_byte_for_byte),
        case!(concurrent_writers_leave_one_whole_value),
        case!(concurrent_writers_on_many_keys_all_land),
        case!(a_namespace_is_not_a_prefix_of_another),
        case!(keys_are_plain_text_whatever_they_hold),
    ]
}

async fn migrate_is_idempotent(cx: &Ctx) -> Result<(), String> {
    step("first migrate", cx.cache.migrate().await)?;
    step("second migrate", cx.cache.migrate().await)?;
    Ok(())
}

async fn a_value_round_trips_as_bytes(cx: &Ctx) -> Result<(), String> {
    let c = &cx.cache;
    let key = cx.key("round", "greeting");
    same!(step("get", c.get(&key).await)?, None, "nothing stored yet");

    // A NUL and bytes that are not UTF-8: the contract is bytes.
    let value: Vec<u8> = vec![0x00, 0xff, 0xfe, b'h', b'i', 0x80];
    step("set", c.set(&key, &value, MINUTE).await)?;
    same!(step("get", c.get(&key).await)?, Some(value), "the value");

    step("second set", c.set(&key, b"second", MINUTE).await)?;
    same!(
        step("get", c.get(&key).await)?,
        Some(b"second".to_vec()),
        "a second set replaces"
    );

    ensure!(
        step("delete", c.delete(&key).await)?,
        "delete answers true when a value was there"
    );
    ensure!(
        !step("delete", c.delete(&key).await)?,
        "delete answers false when nothing was there"
    );
    same!(step("get", c.get(&key).await)?, None, "deleted");
    Ok(())
}

async fn get_many_keeps_the_order_of_the_keys(cx: &Ctx) -> Result<(), String> {
    let c = &cx.cache;
    let ns = cx.namespace("many");
    let keys: Vec<_> = (0..5).map(|i| ns.key(format!("k{i}")).unwrap()).collect();
    // Every other one, so a backend that loses the order is caught.
    for (i, key) in keys.iter().enumerate() {
        if i % 2 == 0 {
            step("set", c.set(key, format!("v{i}").as_bytes(), MINUTE).await)?;
        }
    }
    let got = step("get_many", c.get_many(&keys).await)?;
    same!(got.len(), keys.len(), "one answer per key");
    for (i, slot) in got.iter().enumerate() {
        let wanted = (i % 2 == 0).then(|| format!("v{i}").into_bytes());
        same!(*slot, wanted, "the answer at {i}");
    }
    ensure!(
        step("get_many of nothing", c.get_many(&[]).await)?.is_empty(),
        "no keys, no answers"
    );
    Ok(())
}

async fn an_expired_value_is_a_miss(cx: &Ctx) -> Result<(), String> {
    let c = &cx.cache;
    let key = cx.key("expiry", "short");
    let other = cx.key("expiry", "long");
    step("set", c.set(&key, b"here", SHORT).await)?;
    step("set", c.set(&other, b"stays", MINUTE).await)?;
    same!(
        step("get", c.get(&key).await)?,
        Some(b"here".to_vec()),
        "before the TTL"
    );
    tokio::time::sleep(PAST_SHORT).await;
    same!(step("get", c.get(&key).await)?, None, "after the TTL");
    same!(
        step("get_many", c.get_many(&[key.clone(), other.clone()]).await)?,
        vec![None, Some(b"stays".to_vec())],
        "get_many after the TTL"
    );
    Ok(())
}

async fn add_stores_only_when_nothing_live_is_there(cx: &Ctx) -> Result<(), String> {
    let c = &cx.cache;
    let key = cx.key("add", "once");
    ensure!(
        step("add", c.add(&key, b"first", SHORT).await)?,
        "add on an empty key stores"
    );
    ensure!(
        !step("add", c.add(&key, b"second", SHORT).await)?,
        "a live entry blocks add"
    );
    same!(
        step("get", c.get(&key).await)?,
        Some(b"first".to_vec()),
        "the first value stays"
    );
    tokio::time::sleep(PAST_SHORT).await;
    ensure!(
        step("add", c.add(&key, b"third", MINUTE).await)?,
        "an expired entry is a free slot for add"
    );
    same!(
        step("get", c.get(&key).await)?,
        Some(b"third".to_vec()),
        "add stored after expiry"
    );

    // Two callers racing: exactly one wins.
    let raced = cx.key("add", "race");
    let mut tasks = Vec::new();
    for i in 0..16u8 {
        let cache = Arc::clone(c);
        let raced = raced.clone();
        tasks.push(tokio::spawn(async move {
            cache.add(&raced, &[i], MINUTE).await
        }));
    }
    let mut winners = 0;
    for task in tasks {
        let added = task.await.map_err(|e| format!("an add panicked: {e}"))?;
        if step("add", added)? {
            winners += 1;
        }
    }
    same!(winners, 1, "concurrent adds on one key");
    Ok(())
}

async fn a_zero_ttl_is_refused(cx: &Ctx) -> Result<(), String> {
    let c = &cx.cache;
    let key = cx.key("zero", "k");
    ensure!(
        matches!(
            c.set(&key, b"v", Duration::ZERO).await,
            Err(CacheError::InvalidTtl(_))
        ),
        "set with a zero TTL answers InvalidTtl"
    );
    ensure!(
        matches!(
            c.add(&key, b"v", Duration::ZERO).await,
            Err(CacheError::InvalidTtl(_))
        ),
        "add with a zero TTL answers InvalidTtl"
    );
    ensure!(
        matches!(
            c.set_tagged(&key, b"v", Duration::ZERO, &[cx.tag("zero")])
                .await,
            Err(CacheError::InvalidTtl(_))
        ),
        "set_tagged with a zero TTL answers InvalidTtl"
    );
    ensure!(
        matches!(
            c.lease(&key, Duration::ZERO).await,
            Err(CacheError::InvalidTtl(_))
        ),
        "lease with a zero TTL answers InvalidTtl"
    );
    same!(step("get", c.get(&key).await)?, None, "nothing stored");
    Ok(())
}

async fn namespaces_do_not_see_each_other(cx: &Ctx) -> Result<(), String> {
    let c = &cx.cache;
    let views = cx.namespace("iso-a");
    let sessions = cx.namespace("iso-b");
    let a = views.key("shared").unwrap();
    let b = sessions.key("shared").unwrap();
    step("set", c.set(&a, b"from a", MINUTE).await)?;
    step("set", c.set(&b, b"from b", MINUTE).await)?;
    step("set", c.set(&views.key("two").unwrap(), b"2", MINUTE).await)?;
    same!(step("get", c.get(&a).await)?, Some(b"from a".to_vec()), "a");
    same!(step("get", c.get(&b).await)?, Some(b"from b".to_vec()), "b");

    same!(
        step("clear", c.clear(&views).await)?,
        2,
        "clear counts the entries it removed"
    );
    same!(step("get", c.get(&a).await)?, None, "cleared");
    same!(
        step("get", c.get(&b).await)?,
        Some(b"from b".to_vec()),
        "clearing one namespace leaves the other"
    );
    ensure!(
        !step("delete", c.delete(&views.key("only-b").unwrap()).await)?,
        "a key in another namespace is not this one"
    );
    same!(
        step("clear an empty namespace", c.clear(&views).await)?,
        0,
        "nothing left to clear"
    );

    // A version in the name: a new namespace, and the old entries stay.
    let base = cx.namespace("ver");
    let v1 = Namespace::versioned(base.as_str(), "v1").unwrap();
    let v2 = Namespace::versioned(base.as_str(), "v2").unwrap();
    step("set", c.set(&v1.key("home").unwrap(), b"old", MINUTE).await)?;
    same!(
        step("get", c.get(&v2.key("home").unwrap()).await)?,
        None,
        "the new version starts empty"
    );
    Ok(())
}

async fn a_value_over_the_cap_is_refused(cx: &Ctx) -> Result<(), String> {
    let c = &cx.cache;
    let cap = c.max_value_bytes();
    let key = cx.key("cap", "big");
    let fits = vec![b'x'; cap];
    step("set at the cap", c.set(&key, &fits, MINUTE).await)?;
    same!(
        step("get", c.get(&key).await)?.map(|v| v.len()),
        Some(cap),
        "a value at the cap is stored"
    );
    let over = vec![b'x'; cap + 1];
    match c.set(&key, &over, MINUTE).await {
        Err(CacheError::TooLarge { bytes, limit, .. }) => {
            same!((bytes, limit), (cap + 1, cap), "TooLarge carries the sizes");
        }
        other => return Err(format!("set over the cap: wanted TooLarge, got {other:?}")),
    }
    same!(
        step("get", c.get(&key).await)?.map(|v| v.len()),
        Some(cap),
        "a refused write leaves the old value"
    );
    ensure!(
        matches!(
            c.add(&cx.key("cap", "add"), &over, MINUTE).await,
            Err(CacheError::TooLarge { .. })
        ),
        "add over the cap answers TooLarge"
    );
    ensure!(
        matches!(
            c.set_tagged(&cx.key("cap", "tagged"), &over, MINUTE, &[cx.tag("cap")])
                .await,
            Err(CacheError::TooLarge { .. })
        ),
        "set_tagged over the cap answers TooLarge"
    );
    Ok(())
}

async fn a_lease_is_exclusive_and_is_not_a_value(cx: &Ctx) -> Result<(), String> {
    let c = &cx.cache;
    let key = cx.key("lease", "held");
    ensure!(
        step("lease", c.lease(&key, MINUTE).await)?,
        "the first lease is granted"
    );
    ensure!(
        !step("lease", c.lease(&key, MINUTE).await)?,
        "one holder at a time"
    );
    same!(
        step("get", c.get(&key).await)?,
        None,
        "a lease is not a value"
    );
    same!(
        step("get_many", c.get_many(std::slice::from_ref(&key)).await)?,
        vec![None],
        "get_many does not see a lease"
    );
    ensure!(
        !step("add", c.add(&key, b"v", MINUTE).await)?,
        "a live lease blocks add"
    );
    step("release", c.release_lease(&key).await)?;
    ensure!(
        step("lease", c.lease(&key, MINUTE).await)?,
        "a released lease can be taken again"
    );
    step("set", c.set(&key, b"value", MINUTE).await)?;
    same!(
        step("get", c.get(&key).await)?,
        Some(b"value".to_vec()),
        "set replaces a lease with a value"
    );
    ensure!(
        !step("lease", c.lease(&key, MINUTE).await)?,
        "a live value blocks a lease"
    );
    Ok(())
}

async fn a_lease_expires(cx: &Ctx) -> Result<(), String> {
    let c = &cx.cache;
    let key = cx.key("lease-ttl", "k");
    ensure!(
        step("lease", c.lease(&key, SHORT).await)?,
        "the first lease is granted"
    );
    tokio::time::sleep(PAST_SHORT).await;
    ensure!(
        step("lease", c.lease(&key, MINUTE).await)?,
        "an expired lease frees the key"
    );
    Ok(())
}

async fn releasing_a_lease_keeps_a_stored_value(cx: &Ctx) -> Result<(), String> {
    let c = &cx.cache;
    let key = cx.key("release", "k");
    ensure!(
        step("lease", c.lease(&key, MINUTE).await)?,
        "the lease is granted"
    );
    step("set", c.set(&key, b"a real value", MINUTE).await)?;
    step("release", c.release_lease(&key).await)?;
    same!(
        step("get", c.get(&key).await)?,
        Some(b"a real value".to_vec()),
        "release_lease removes a lease only"
    );
    // Releasing what was never leased is not an error.
    step(
        "release of nothing",
        c.release_lease(&cx.key("release", "none")).await,
    )?;
    Ok(())
}

async fn fetch_computes_once_under_concurrency(cx: &Ctx) -> Result<(), String> {
    const CALLERS: u32 = 24;
    let key = Arc::new(cx.key("stampede", "expensive"));
    let runs = Arc::new(AtomicU32::new(0));
    let mut tasks = Vec::new();
    for _ in 0..CALLERS {
        let cache = Arc::clone(&cx.cache);
        let key = Arc::clone(&key);
        let runs = Arc::clone(&runs);
        tasks.push(tokio::spawn(async move {
            let policy = FetchPolicy::new(MINUTE)
                .lease_for(Duration::from_secs(30))
                .wait_for(Duration::from_secs(20));
            cache
                .fetch_with(&key, policy, || async move {
                    runs.fetch_add(1, Ordering::SeqCst);
                    // Long enough that every other caller waits on the
                    // lease rather than arriving after the value landed.
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    Ok::<_, std::io::Error>(b"computed once".to_vec())
                })
                .await
        }));
    }
    for task in tasks {
        match task.await {
            Ok(Ok(value)) => same!(value, b"computed once".to_vec(), "every caller's value"),
            Ok(Err(e)) => return Err(format!("fetch: {e}")),
            Err(e) => return Err(format!("a caller panicked: {e}")),
        }
    }
    same!(
        runs.load(Ordering::SeqCst),
        1,
        "{CALLERS} concurrent misses compute once"
    );
    same!(
        step("get", cx.cache.get(&key).await)?,
        Some(b"computed once".to_vec()),
        "fetch stored the value"
    );
    Ok(())
}

async fn a_failed_fetch_gives_the_lease_back(cx: &Ctx) -> Result<(), String> {
    let c = &cx.cache;
    let key = cx.key("fetch-fail", "flaky");
    let failed: Result<Vec<u8>, FetchError<std::io::Error>> = c
        .fetch(&key, MINUTE, || async {
            Err(std::io::Error::other("the upstream was down"))
        })
        .await;
    ensure!(
        matches!(failed, Err(FetchError::Compute(_))),
        "a failed computation answers Compute, got {failed:?}"
    );
    ensure!(
        step("lease", c.lease(&key, Duration::from_secs(5)).await)?,
        "the failed fetch released its lease"
    );
    step("release", c.release_lease(&key).await)?;
    let ok: Result<Vec<u8>, FetchError<std::io::Error>> = c
        .fetch(&key, MINUTE, || async { Ok(b"worked".to_vec()) })
        .await;
    same!(ok.ok(), Some(b"worked".to_vec()), "the next fetch computes");
    Ok(())
}

async fn fetch_reports_a_value_it_cannot_store(cx: &Ctx) -> Result<(), String> {
    let c = &cx.cache;
    let over = vec![b'x'; c.max_value_bytes() + 1];
    let key = cx.key("fetch-cap", "k");
    let result: Result<Vec<u8>, FetchError<std::io::Error>> =
        c.fetch(&key, MINUTE, || async move { Ok(over) }).await;
    ensure!(
        matches!(result, Err(FetchError::Cache(CacheError::TooLarge { .. }))),
        "fetch of a value over the cap answers TooLarge"
    );
    ensure!(
        step("lease", c.lease(&key, Duration::from_secs(5)).await)?,
        "the refused fetch released its lease"
    );
    Ok(())
}

async fn a_tag_removes_its_entries_in_every_namespace(cx: &Ctx) -> Result<(), String> {
    let c = &cx.cache;
    let posts = cx.tag("posts");
    let page = cx.key("tag-views", "post-1");
    let sidebar = cx.key("tag-parts", "sidebar");
    let other = cx.key("tag-views", "about");
    step(
        "set_tagged",
        c.set_tagged(&page, b"page", MINUTE, std::slice::from_ref(&posts))
            .await,
    )?;
    step(
        "set_tagged",
        c.set_tagged(&sidebar, b"side", MINUTE, std::slice::from_ref(&posts))
            .await,
    )?;
    step("set", c.set(&other, b"about", MINUTE).await)?;

    same!(
        step("invalidate_tag", c.invalidate_tag(&posts).await)?,
        2,
        "invalidate_tag counts the entries it removed"
    );
    same!(step("get", c.get(&page).await)?, None, "the page went");
    same!(
        step("get", c.get(&sidebar).await)?,
        None,
        "the sidebar went"
    );
    same!(
        step("get", c.get(&other).await)?,
        Some(b"about".to_vec()),
        "an untagged entry stays"
    );
    same!(
        step("invalidate_tag", c.invalidate_tag(&posts).await)?,
        0,
        "a spent tag removes nothing"
    );

    // A deleted entry's tag invalidates nothing.
    let gone = cx.key("tag-views", "gone");
    step(
        "set_tagged",
        c.set_tagged(&gone, b"x", MINUTE, std::slice::from_ref(&posts))
            .await,
    )?;
    step("delete", c.delete(&gone).await)?;
    same!(
        step("invalidate_tag", c.invalidate_tag(&posts).await)?,
        0,
        "a deleted entry is not counted"
    );
    Ok(())
}

async fn a_tagged_write_replaces_the_earlier_tags(cx: &Ctx) -> Result<(), String> {
    let c = &cx.cache;
    let (a, b) = (cx.tag("retag-a"), cx.tag("retag-b"));
    let key = cx.key("retag", "k");
    step(
        "set_tagged",
        c.set_tagged(&key, b"one", MINUTE, &[a.clone(), b.clone()])
            .await,
    )?;
    step(
        "set_tagged",
        c.set_tagged(&key, b"two", MINUTE, std::slice::from_ref(&b))
            .await,
    )?;
    same!(
        step("invalidate_tag", c.invalidate_tag(&a).await)?,
        0,
        "the dropped tag no longer reaches the entry"
    );
    same!(
        step("get", c.get(&key).await)?,
        Some(b"two".to_vec()),
        "the entry stays"
    );
    same!(
        step("invalidate_tag", c.invalidate_tag(&b).await)?,
        1,
        "the kept tag reaches it"
    );
    same!(step("get", c.get(&key).await)?, None, "and removes it");

    // An empty tag set clears the tags.
    let c2 = cx.key("retag", "cleared");
    step(
        "set_tagged",
        c.set_tagged(&c2, b"x", MINUTE, std::slice::from_ref(&a))
            .await,
    )?;
    step("set_tagged", c.set_tagged(&c2, b"y", MINUTE, &[]).await)?;
    same!(
        step("invalidate_tag", c.invalidate_tag(&a).await)?,
        0,
        "an empty tag set drops the tags"
    );
    same!(
        step("get", c.get(&c2).await)?,
        Some(b"y".to_vec()),
        "the entry stays"
    );
    Ok(())
}

async fn a_plain_set_keeps_the_earlier_tags(cx: &Ctx) -> Result<(), String> {
    let c = &cx.cache;
    let old = cx.tag("plain-old");
    let key = cx.key("plain", "k");
    step(
        "set_tagged",
        c.set_tagged(&key, b"tagged", MINUTE, std::slice::from_ref(&old))
            .await,
    )?;
    step("set", c.set(&key, b"plain", MINUTE).await)?;
    // The trait documents it: a miss, never a stale hit.
    same!(
        step("invalidate_tag", c.invalidate_tag(&old).await)?,
        1,
        "the old tag still reaches the entry"
    );
    same!(step("get", c.get(&key).await)?, None, "and removes it");
    Ok(())
}

async fn several_tags_count_an_entry_once(cx: &Ctx) -> Result<(), String> {
    let c = &cx.cache;
    let (a, b, t) = (cx.tag("many-a"), cx.tag("many-b"), cx.tag("many-c"));
    let both = cx.key("many-tags", "both");
    let only_b = cx.key("many-tags", "only-b");
    let only_c = cx.key("many-tags", "only-c");
    step(
        "set_tagged",
        c.set_tagged(&both, b"1", MINUTE, &[a.clone(), b.clone()])
            .await,
    )?;
    step(
        "set_tagged",
        c.set_tagged(&only_b, b"2", MINUTE, std::slice::from_ref(&b))
            .await,
    )?;
    step(
        "set_tagged",
        c.set_tagged(&only_c, b"3", MINUTE, std::slice::from_ref(&t))
            .await,
    )?;
    same!(
        step("invalidate_tags", c.invalidate_tags(&[a, b]).await)?,
        2,
        "an entry with two of the tags counts once"
    );
    same!(
        step("get", c.get(&only_c).await)?,
        Some(b"3".to_vec()),
        "an entry with another tag stays"
    );
    same!(
        step("invalidate_tags of none", c.invalidate_tags(&[]).await)?,
        0,
        "no tags remove nothing"
    );
    Ok(())
}

async fn fetch_with_tags_stores_them(cx: &Ctx) -> Result<(), String> {
    let c = &cx.cache;
    let t = cx.tag("fetch-tags");
    let key = cx.key("fetch-tags", "k");
    let policy = FetchPolicy::new(MINUTE).tags([t.clone()]);
    let value = c
        .fetch_with(&key, policy, || async {
            Ok::<_, std::io::Error>(b"v".to_vec())
        })
        .await
        .map_err(|e| format!("fetch_with: {e}"))?;
    same!(value, b"v".to_vec(), "the computed value");
    same!(
        step("invalidate_tag", c.invalidate_tag(&t).await)?,
        1,
        "the tag reaches the fetched entry"
    );
    Ok(())
}

async fn a_fragment_renders_once_until_its_tag_goes(cx: &Ctx) -> Result<(), String> {
    let c = &cx.cache;
    let posts = [cx.tag("fragment-posts")];
    let key = cx.key("fragments", "sidebar");
    let renders = AtomicU32::new(0);
    let render = || {
        renders.fetch_add(1, Ordering::SeqCst);
        "<ul><li>post</li></ul>".to_owned()
    };
    let first = step("fragment", c.fragment(&key, &posts, MINUTE, render).await)?;
    let second = step("fragment", c.fragment(&key, &posts, MINUTE, render).await)?;
    same!(
        first.as_str(),
        "<ul><li>post</li></ul>",
        "the rendered text"
    );
    same!(second, first, "the stored text");
    same!(
        renders.load(Ordering::SeqCst),
        1,
        "one render for two calls"
    );
    step("invalidate_tag", c.invalidate_tag(&posts[0]).await)?;
    step("fragment", c.fragment(&key, &posts, MINUTE, render).await)?;
    same!(
        renders.load(Ordering::SeqCst),
        2,
        "the tag sent the next call back to the template"
    );
    Ok(())
}

async fn a_failed_fragment_is_not_stored(cx: &Ctx) -> Result<(), String> {
    let c = &cx.cache;
    let key = cx.key("fragment-fail", "broken");
    let failed = c
        .try_fragment(&key, &[], MINUTE, || {
            Err::<String, _>(std::io::Error::other("template error"))
        })
        .await;
    ensure!(
        matches!(failed, Err(FetchError::Compute(_))),
        "a failed render answers Compute"
    );
    same!(step("get", c.get(&key).await)?, None, "nothing stored");
    Ok(())
}

async fn purge_expired_keeps_live_entries(cx: &Ctx) -> Result<(), String> {
    let c = &cx.cache;
    let live = cx.key("purge", "live");
    let dead = cx.key("purge", "dead");
    let t = cx.tag("purge");
    step(
        "set_tagged",
        c.set_tagged(&live, b"live", MINUTE, std::slice::from_ref(&t))
            .await,
    )?;
    step("set", c.set(&dead, b"dead", SHORT).await)?;
    tokio::time::sleep(PAST_SHORT).await;
    // A backend may have nothing to purge; the count is its own business.
    // What it must not do is remove what is live, or take more than asked.
    let removed = step("purge_expired", c.purge_expired(1).await)?;
    ensure!(removed <= 1, "purge_expired(1) removed {removed} entries");
    step("sweep", brocanth_cache::sweep(&**c, 1_000, 100).await)?;
    same!(
        step("get", c.get(&live).await)?,
        Some(b"live".to_vec()),
        "a live entry survives the sweep"
    );
    same!(
        step("get", c.get(&dead).await)?,
        None,
        "an expired one is a miss"
    );
    same!(
        step("invalidate_tag", c.invalidate_tag(&t).await)?,
        1,
        "a live entry keeps its tags through the sweep"
    );
    Ok(())
}

async fn a_rewrite_sets_a_new_ttl(cx: &Ctx) -> Result<(), String> {
    let c = &cx.cache;
    let longer = cx.key("rewrite", "longer");
    let shorter = cx.key("rewrite", "shorter");
    let added = cx.key("rewrite", "added");
    step("set", c.set(&longer, b"short", SHORT).await)?;
    step("set", c.set(&longer, b"long", MINUTE).await)?;
    step("set", c.set(&shorter, b"long", MINUTE).await)?;
    step("set", c.set(&shorter, b"short", SHORT).await)?;
    // A failed add must not touch the TTL of the live entry.
    step("set", c.set(&added, b"first", SHORT).await)?;
    ensure!(
        !step("add", c.add(&added, b"second", MINUTE).await)?,
        "a live entry blocks add"
    );
    tokio::time::sleep(PAST_SHORT).await;
    same!(
        step("get", c.get(&longer).await)?,
        Some(b"long".to_vec()),
        "a rewrite with a longer TTL outlives the first TTL"
    );
    same!(
        step("get", c.get(&shorter).await)?,
        None,
        "a rewrite with a shorter TTL ends sooner"
    );
    same!(
        step("get", c.get(&added).await)?,
        None,
        "a refused add leaves the old TTL"
    );
    Ok(())
}

async fn a_ttl_under_a_millisecond_is_accepted_and_soon_gone(cx: &Ctx) -> Result<(), String> {
    let c = &cx.cache;
    let tiny = Duration::from_micros(50);
    let key = cx.key("tiny", "set");
    let added = cx.key("tiny", "add");
    let leased = cx.key("tiny", "lease");
    step("set with a tiny TTL", c.set(&key, b"v", tiny).await)?;
    step("add with a tiny TTL", c.add(&added, b"v", tiny).await)?;
    step("lease with a tiny TTL", c.lease(&leased, tiny).await)?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    same!(
        step("get", c.get(&key).await)?,
        None,
        "a set with a tiny TTL"
    );
    same!(
        step("get", c.get(&added).await)?,
        None,
        "an add with a tiny TTL"
    );
    ensure!(
        step("lease", c.lease(&leased, MINUTE).await)?,
        "a lease with a tiny TTL frees the key"
    );
    Ok(())
}

async fn a_ttl_past_the_end_of_time_is_refused(cx: &Ctx) -> Result<(), String> {
    let c = &cx.cache;
    let key = cx.key("forever", "k");
    let forever = Duration::MAX;
    ensure!(
        matches!(
            c.set(&key, b"v", forever).await,
            Err(CacheError::InvalidTtl(_))
        ),
        "set with Duration::MAX answers InvalidTtl"
    );
    ensure!(
        matches!(
            c.add(&key, b"v", forever).await,
            Err(CacheError::InvalidTtl(_))
        ),
        "add with Duration::MAX answers InvalidTtl"
    );
    ensure!(
        matches!(
            c.set_tagged(&key, b"v", forever, &[cx.tag("forever")])
                .await,
            Err(CacheError::InvalidTtl(_))
        ),
        "set_tagged with Duration::MAX answers InvalidTtl"
    );
    ensure!(
        matches!(c.lease(&key, forever).await, Err(CacheError::InvalidTtl(_))),
        "lease with Duration::MAX answers InvalidTtl"
    );
    same!(step("get", c.get(&key).await)?, None, "nothing stored");

    // Long, but representable: ten years is a TTL, not an error.
    let decade = Duration::from_secs(10 * 365 * 24 * 60 * 60);
    step("set for ten years", c.set(&key, b"kept", decade).await)?;
    same!(
        step("get", c.get(&key).await)?,
        Some(b"kept".to_vec()),
        "a ten-year TTL"
    );
    Ok(())
}

async fn deleting_what_is_gone_is_not_an_error(cx: &Ctx) -> Result<(), String> {
    let c = &cx.cache;
    ensure!(
        !step("delete", c.delete(&cx.key("gone", "never")).await)?,
        "delete of a key never written answers false"
    );
    // Whether an expired entry counts as "there" is the backend's own
    // business (Redis has already dropped it). It must not fail, and the
    // key must stay a miss.
    let expired = cx.key("gone", "expired");
    step("set", c.set(&expired, b"v", SHORT).await)?;
    tokio::time::sleep(PAST_SHORT).await;
    step("delete of an expired entry", c.delete(&expired).await)?;
    same!(step("get", c.get(&expired).await)?, None, "still a miss");
    ensure!(
        !step("delete again", c.delete(&expired).await)?,
        "a second delete answers false"
    );
    ensure!(
        step("add", c.add(&expired, b"new", MINUTE).await)?,
        "a deleted key is free for add"
    );
    same!(
        step(
            "clear an empty namespace",
            c.clear(&cx.namespace("gone-none")).await
        )?,
        0,
        "clear of a namespace never written"
    );
    Ok(())
}

async fn an_empty_value_is_a_value(cx: &Ctx) -> Result<(), String> {
    let c = &cx.cache;
    let key = cx.key("empty", "set");
    step("set", c.set(&key, b"", MINUTE).await)?;
    same!(
        step("get", c.get(&key).await)?,
        Some(Vec::new()),
        "an empty value is a hit, not a miss"
    );
    same!(
        step("get_many", c.get_many(std::slice::from_ref(&key)).await)?,
        vec![Some(Vec::new())],
        "get_many sees the empty value"
    );
    ensure!(
        !step("lease", c.lease(&key, MINUTE).await)?,
        "an empty value blocks a lease like any value"
    );
    ensure!(
        !step("add", c.add(&key, b"other", MINUTE).await)?,
        "an empty value blocks add"
    );
    step("release", c.release_lease(&key).await)?;
    same!(
        step("get", c.get(&key).await)?,
        Some(Vec::new()),
        "release_lease does not take an empty value for a lease"
    );
    let added = cx.key("empty", "add");
    ensure!(
        step("add", c.add(&added, b"", MINUTE).await)?,
        "add stores an empty value"
    );
    same!(
        step("get", c.get(&added).await)?,
        Some(Vec::new()),
        "the added empty value"
    );
    ensure!(
        step("delete", c.delete(&key).await)?,
        "delete answers true for an empty value"
    );
    Ok(())
}

async fn a_large_value_round_trips_byte_for_byte(cx: &Ctx) -> Result<(), String> {
    let c = &cx.cache;
    let cap = c.max_value_bytes();
    // Every byte value, in an order that does not repeat on a short
    // period, so a truncation, an encoding or a shifted chunk shows.
    let pattern = |len: usize, seed: usize| -> Vec<u8> {
        (0..len)
            .map(|i| ((i.wrapping_mul(31) + i / 251 + seed) % 256) as u8)
            .collect()
    };
    let ns = cx.namespace("large");
    let sizes = [cap.min(64 * 1024), cap / 2 + 3, cap];
    let mut keys = Vec::new();
    for (n, len) in sizes.iter().enumerate() {
        let key = ns.key(format!("k{n}")).unwrap();
        let value = pattern(*len, n);
        step("set", c.set(&key, &value, MINUTE).await)?;
        let got = step("get", c.get(&key).await)?;
        ensure!(
            got.as_deref() == Some(&value[..]),
            "a value of {len} bytes came back changed ({:?} bytes)",
            got.map(|v| v.len())
        );
        keys.push(key);
    }
    let many = step("get_many", c.get_many(&keys).await)?;
    for (n, (len, got)) in sizes.iter().zip(&many).enumerate() {
        ensure!(
            got.as_deref() == Some(&pattern(*len, n)[..]),
            "get_many: the value of {len} bytes came back changed"
        );
    }
    // A large value replaced by a small one leaves nothing of the old.
    step("set small", c.set(&keys[2], b"small", MINUTE).await)?;
    same!(
        step("get", c.get(&keys[2]).await)?,
        Some(b"small".to_vec()),
        "a small value over a large one"
    );
    Ok(())
}

async fn concurrent_writers_leave_one_whole_value(cx: &Ctx) -> Result<(), String> {
    const WRITERS: u8 = 16;
    const LEN: usize = 32 * 1024;
    let key = cx.key("writers", "one");
    let whole = |v: &[u8]| v.len() == LEN && v.iter().all(|b| *b == v[0]) && v[0] < WRITERS;
    let mut tasks = Vec::new();
    for i in 0..WRITERS {
        let cache = Arc::clone(&cx.cache);
        let key = key.clone();
        tasks.push(tokio::spawn(async move {
            for _ in 0..4 {
                cache.set(&key, &vec![i; LEN], MINUTE).await?;
                // A reader between writes sees one writer's value, whole.
                if let Some(v) = cache.get(&key).await? {
                    if !(v.len() == LEN && v.iter().all(|b| *b == v[0])) {
                        return Ok(false);
                    }
                }
            }
            Ok::<_, CacheError>(true)
        }));
    }
    for task in tasks {
        let clean = task.await.map_err(|e| format!("a writer panicked: {e}"))?;
        ensure!(step("write", clean)?, "a reader saw a torn value");
    }
    let last = step("get", cx.cache.get(&key).await)?;
    ensure!(
        last.as_deref().is_some_and(whole),
        "after the race the value is one writer's, whole: got {:?} bytes",
        last.map(|v| v.len())
    );
    Ok(())
}

async fn concurrent_writers_on_many_keys_all_land(cx: &Ctx) -> Result<(), String> {
    const KEYS: usize = 64;
    let ns = cx.namespace("fanout");
    let t = cx.tag("fanout");
    let mut tasks = Vec::new();
    for i in 0..KEYS {
        let cache = Arc::clone(&cx.cache);
        let key = ns.key(format!("k{i}")).unwrap();
        let t = t.clone();
        tasks.push(tokio::spawn(async move {
            let value = format!("v{i}");
            if i % 2 == 0 {
                cache.set(&key, value.as_bytes(), MINUTE).await
            } else {
                cache
                    .set_tagged(&key, value.as_bytes(), MINUTE, std::slice::from_ref(&t))
                    .await
            }
        }));
    }
    for task in tasks {
        let done = task.await.map_err(|e| format!("a writer panicked: {e}"))?;
        step("write", done)?;
    }
    let keys: Vec<_> = (0..KEYS)
        .map(|i| ns.key(format!("k{i}")).unwrap())
        .collect();
    let got = step("get_many", cx.cache.get_many(&keys).await)?;
    for (i, slot) in got.iter().enumerate() {
        same!(
            *slot,
            Some(format!("v{i}").into_bytes()),
            "the value at {i}"
        );
    }
    same!(
        step("invalidate_tag", cx.cache.invalidate_tag(&t).await)?,
        (KEYS / 2) as u64,
        "every concurrent tagged write recorded its tag"
    );
    same!(
        step("clear", cx.cache.clear(&ns).await)?,
        (KEYS / 2) as u64,
        "the untagged half is left for clear"
    );
    Ok(())
}

async fn a_namespace_is_not_a_prefix_of_another(cx: &Ctx) -> Result<(), String> {
    let c = &cx.cache;
    let base = cx.namespace("pfx");
    let name = base.as_str();
    let others: Vec<Namespace> = [":sub", ".sub", "-sub", "_sub", "x"]
        .iter()
        .map(|tail| Namespace::new(format!("{name}{tail}")).unwrap())
        .collect();
    // The joined strings collide: `pfx` + `sub:k` and `pfx:sub` + `k`.
    let in_base = base.key("sub:k").unwrap();
    let in_sub = others[0].key("k").unwrap();
    step("set", c.set(&in_base, b"base", MINUTE).await)?;
    step("set", c.set(&in_sub, b"sub", MINUTE).await)?;
    for ns in &others[1..] {
        step("set", c.set(&ns.key("k").unwrap(), b"other", MINUTE).await)?;
    }
    same!(
        step("get", c.get(&in_base).await)?,
        Some(b"base".to_vec()),
        "a key with the separator in it"
    );
    same!(
        step("get", c.get(&in_sub).await)?,
        Some(b"sub".to_vec()),
        "a namespace that extends another"
    );
    same!(
        step("get", c.get(&base.key("k").unwrap()).await)?,
        None,
        "the base namespace does not see a longer one"
    );
    same!(
        step("clear", c.clear(&base).await)?,
        1,
        "clear takes its own namespace only"
    );
    same!(
        step("get", c.get(&in_sub).await)?,
        Some(b"sub".to_vec()),
        "clearing a namespace leaves the one that extends it"
    );
    for ns in &others[1..] {
        same!(
            step("get", c.get(&ns.key("k").unwrap()).await)?,
            Some(b"other".to_vec()),
            "clearing `{name}` leaves `{ns}`"
        );
    }
    same!(
        step("clear", c.clear(&others[0]).await)?,
        1,
        "the longer namespace clears by itself"
    );
    Ok(())
}

async fn keys_are_plain_text_whatever_they_hold(cx: &Ctx) -> Result<(), String> {
    let c = &cx.cache;
    let ns = cx.namespace("text");
    // Unicode, and every character a glob, a LIKE or a quote reads.
    let long = format!("{}x", "日".repeat(341));
    let names: Vec<String> = [
        "café",
        "日本語",
        "🦀 crab",
        "*",
        "a*",
        "?",
        "[ab]",
        "a\\b",
        "^",
        "%",
        "_",
        "100%_off",
        "a b",
        "it's",
        "\"q\"",
        "k:with:colons",
        " ",
        "{x}",
        "$1",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .chain(std::iter::once(long))
    .collect();
    let keys: Vec<_> = names
        .iter()
        .map(|n| ns.key(n.clone()).map_err(|e| format!("key {n:?}: {e}")))
        .collect::<Result<_, _>>()?;
    let t = cx.tag("étiquette 🏷 *");
    for (i, key) in keys.iter().enumerate() {
        let value = format!("{i}:{}", names[i]);
        if i % 3 == 0 {
            step(
                "set_tagged",
                c.set_tagged(key, value.as_bytes(), MINUTE, std::slice::from_ref(&t))
                    .await,
            )?;
        } else {
            step("set", c.set(key, value.as_bytes(), MINUTE).await)?;
        }
    }
    for (i, key) in keys.iter().enumerate() {
        same!(
            step("get", c.get(key).await)?,
            Some(format!("{i}:{}", names[i]).into_bytes()),
            "the value at {:?}",
            names[i]
        );
    }
    let got = step("get_many", c.get_many(&keys).await)?;
    for (i, slot) in got.iter().enumerate() {
        same!(
            *slot,
            Some(format!("{i}:{}", names[i]).into_bytes()),
            "get_many at {:?}",
            names[i]
        );
    }
    // `*` is a key, not a pattern: deleting it takes it alone.
    ensure!(
        step("delete", c.delete(&keys[3]).await)?,
        "delete of the key `*`"
    );
    same!(
        step("get", c.get(&keys[4]).await)?,
        Some(format!("4:{}", names[4]).into_bytes()),
        "deleting `*` leaves `a*`"
    );
    let tagged = keys.len().div_ceil(3) as u64 - 1; // `*` (index 3) was tagged and is gone
    same!(
        step("invalidate_tag", c.invalidate_tag(&t).await)?,
        tagged,
        "a tag with Unicode and a `*` in it"
    );
    let left = keys.len() as u64 - 1 - tagged;
    same!(
        step("clear", c.clear(&ns).await)?,
        left,
        "clear counts every key left"
    );
    Ok(())
}
