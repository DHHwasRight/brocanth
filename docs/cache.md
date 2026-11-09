# Cache: tags, routes and fragments

`brocanth-cache` stores bytes under a key in an unlogged Postgres table. The
crate documentation argues that choice. This page covers the three layers
above the key/value store (tags, the route cache and the fragment cache),
and the sweep that deletes expired rows.
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

`brocanth-model` publishes its writes (G4, done). `brocanth_model::changes`
has `insert_published`, `update_published`, `delete_published` (the
`Publish` trait) and `Tracked::save_published`. Each takes a
`ChangeSink`, so pass a `TagInvalidator`, or a `brocanth_events::FanOut`
that holds it beside the realtime broker. A write inside a transaction
goes through `Writing`, and the sink hears it only when the transaction
commits. A write that does not go through these methods publishes
nothing: call `publish` yourself or invalidate the tags directly.

## 5. The sweep

Every read checks `expires_at`, so an expired entry is a miss whether its
row is there or not. The sweep only frees the space. Without it, the
`UNLOGGED` table keeps every expired row until its key is written again.

`CachePlugin` and the generated `cache` plugin run the sweep themselves.
`brocanth_cache::Sweeper` is one Tokio task per process. It starts at
`start` and stops at `shutdown` (dropping it stops it too). Each run calls
`sweep(cache, 10_000, 50)`.

| | `CachePlugin` | Generated plugin |
|---|---|---|
| Interval | `CacheConfig::sweep_every(Duration)`, default 5 minutes | `[cache] sweep_every`, seconds, default `300` |
| Off | `Duration::ZERO` | `0` |
| A failed sweep | `CacheConfig::on_sweep_error(f)`, default: nothing | a `warn` line through `say!` |

* **Jitter.** Each wait is the interval with up to a quarter added or
  taken away, at random. Instances that start in the same deploy do not
  sweep at the same moment.
* **Several instances.** Each one sweeps. Two deletes at once remove each
  row once, so no lock or leader is needed.
* **First run.** After the first wait, not at `start`: with `migrate`, the
  readiness check can still be creating the table.
* **Cost.** `expires_at` has no index, on purpose (`PgCache::schema_sql`
  says why), so a sweep is a sequential scan. It runs every few minutes,
  off the request path.

**Why the plugin and not a `jobs` cron.** An earlier version left the
schedule to `brocanth-jobs` and wired nothing, so no project deleted an
expired row. A cron job would work only in projects that also have `jobs`,
and most projects that cache have no queue. The plugin's task works in
every project with `cache`, links nothing new (the crate already links
Tokio's timer for `fetch`), and is cheap when there is nothing to delete.
A project that schedules the sweep another way (a recurring job,
`pg_cron`) sets the interval to zero.

## 6. Not done

* The in-memory backend, `MemoryCache`, is for tests and development only
  (decision 1 in `docs/audit/backends-jobs-cache.md`). It passes the
  conformance suite, and `brocanth check --deploy` refuses the
  `cache-memory` module in production. The tag SQL is still tested
  against Postgres.
* No stale-while-revalidate, no CDN purge, no `HEAD` from a stored `GET`.
* No scaffold change for the route cache: the `cache` module of
  `brocanth create` writes the plugin, and an application adds
  `RouteCache` by hand.
