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
