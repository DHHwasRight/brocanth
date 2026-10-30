# Cache: tags, routes and fragments

`brocanth-cache` stores bytes under a key in an unlogged Postgres table. The
crate documentation argues that choice. This page covers the three layers
above the key/value store: tags, the route cache and the fragment cache.
They close gap G8 in `docs/framework-gaps.md` (A15, D19).

## 1. Tags

A tag names a group of entries. Store an entry with the tags of what it
read, then invalidate a tag to remove every entry that carries it, in every
namespace.

```rust
let posts = Tag::table("posts")?;       // "posts"
let post = Tag::row("posts", 42)?;      // "posts:42"

cache.set_tagged(&key, &html, ttl, &[posts.clone(), post.clone()]).await?;
cache.invalidate_tag(&post).await?;     // the entry is gone
cache.invalidate_tags(&[a, b]).await?;  // one round trip
```

What it promises:

* **A miss, never a stale hit.** An invalidation can remove more than it
  had to. A plain `set` over a tagged key keeps the old tags until the
  entry expires, so invalidating one of them removes the newer value too.
* **A write that races an invalidation can survive it.** A value computed
  from the old row and stored after the invalidation ran stays. The TTL is
  the bound.
* A tagged write replaces the key's tag set in the same statement as the
  value.

Storage: a second unlogged table, `brocanth_cache_tags (tag, namespace, k)`.
Not a `TEXT[]` column on `brocanth_cache`, because an index on a column that
changes ends HOT updates for every `set`. `purge_expired` (and so `sweep`)
also removes tag rows whose entry is gone. `migrate` and `schema_sql`
create the table; an application that pastes `schema_sql` into its own
migration must paste the two new statements too.

`FetchPolicy::tags` stores a `fetch_with` result with tags.

## 2. The route cache

`brocanth_cache::route::RouteCache` is a plugin with two hooks. The request
hook (`AsyncRequestHook`) answers a hit without running the handler. The
response hook stores the handler's response when it is safe.

```rust
let routes = RouteCacheConfig::new(cache.clone())
    .session_cookies(["sid"])
    .rule(RouteRule::path("/", Duration::from_secs(60)))
    .rule(
        RouteRule::prefix("/posts/", Duration::from_secs(300))
            .tags([Tag::table("posts")?])
            .vary([http::header::ACCEPT_LANGUAGE]),
    );
let app = AppBuilder::new().plugin_hooking(RouteCache::new(routes)).build()?;
```

A response is stored only when every one of these is true:

| Condition | Why |
|---|---|
| `GET`, and a rule matches the path (first rule wins) | nothing else is safe to replay |
| no `Authorization`, no session cookie, unless `allow_authenticated()` | one user's page must not go to another |
| status `200` | errors and redirects are not pages |
| buffered body | a stream has no body to store |
| no `Set-Cookie` | a cookie is for one client |
| `Cache-Control` has no `no-store` or `private` | the handler's opt-out |
| fits under `max_value_bytes` | the cap is an error elsewhere, a skip here |

* **Key:** `GET`, path, query (unless `ignore_query()`), and each `vary`
  header's value, escaped so one part cannot impersonate another. A key over
  1024 bytes is not cached.
* **Tags:** the rule's tags, `route:{path}` (`route::path_tag`), and any
  the handler sets in the `brocanth-cache-tags` response header. The hook
  removes that header from every response.
* **`brocanth-cache: hit | miss`** on each response the cache looked at.
* **Order:** declare the plugin so its response hook runs before any hook
  that rewrites the body (compression, `ETag`). On a hit those hooks run
  again over the stored body.
* **Failure:** a lookup error fails the request, as `fetch` does. The store
  is spawned on the Tokio runtime after the response is final; a failed
  store is a later miss.
* **No single flight.** Concurrent misses on one path all run the handler.

## 3. Fragments

```rust
let sidebar: String = cache
    .fragment(&fragments.key("sidebar")?, &[Tag::table("posts")?], ttl, || {
        render_sidebar()
    })
    .await?;
```

`render` is a plain closure, because templates render synchronously. It runs
only on a miss, under the same single-flight lease as `fetch`.
`try_fragment` takes a render that can fail; a failed render is not stored.

## 4. Invalidation from model changes

With the `events` feature, `TagInvalidator` is a `brocanth_events::ChangeSink`.
Each `RowChange` invalidates `table` and `table:id`. `publish` must not block,
so the invalidation is spawned on the Tokio runtime and lands just after the
write. `.only([...])` limits it to some collections, and `wants` tells the
model layer to skip the rest.

`brocanth-model` publishes no changes yet (G4), so today an application
calls `publish` itself or invalidates tags directly.

## 5. Not done

* No in-memory backend. `brocanth-cache` is Postgres only on purpose
  (`tests/common/mod.rs`). The route-cache hook tests use a fake, as the
  panel tests do; the tag SQL is tested against Postgres.
* No stale-while-revalidate, no CDN purge, no `HEAD` from a stored `GET`.
* No scaffold change: the `cache` module of `brocanth create` writes the
  plugin, and an application adds `RouteCache` by hand.
