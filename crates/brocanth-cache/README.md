# brocanth-cache

A cache kept in Postgres, so a deployment that already runs Postgres gets a
cache without Redis. It stores bytes under a namespaced key with a TTL, in an
`UNLOGGED` table: no write-ahead log, no replication, and an empty cache after
a crash. On top of that store it gives a read-through `fetch` with single
flight, tags, a fragment cache, a route cache for whole `GET` responses and a
backoffice panel. Use it when you run one Postgres and the cache holds
thousands to low millions of entries. Use Redis when the cache is a second
workload, when you send reads to replicas, when you need an eviction policy,
or when the cache must survive a crash. The crate documentation argues each
of these points.

## Getting started

```toml
[dependencies]
brocanth-cache = { git = "https://github.com/DHHwasRight/brocanth", branch = "main" }
```

| Feature | Default | Adds |
|---|---|---|
| `runtime` | on | the cache itself: everything but `scaffold`. Every other feature turns it on |
| `postgres` | on | `PgCache`, the one backend (`sqlx`) |
| `admin` | off | `brocanth_cache::admin`, the cache tab in `brocanth-admin` |
| `events` | off | `TagInvalidator`, a `ChangeSink` that invalidates tags on model writes |
| `scaffold` | off | the `cache` module for `brocanth create`; only the CLI turns it on |

With `default-features = false, features = ["runtime"]` you get the `Cache` trait, the key types, the
clock and `fetch`, and no SQL driver. That build is for a crate that writes a
second backend.

As a module:

```sh
brocanth create my-app --with config,cache   # a new project
brocanth add cache                           # an existing project
```

The module writes `src/plugins/cache/mod.rs`. That plugin opens a `PgCache`
from the URL at startup and puts a `CacheHandle` on the context. It does not
create the table: put `PgCache::schema_sql()` in a migration that you read.

Settings, section `[cache]`:

| Key | Kind | Default | Flag | Environment |
|---|---|---|---|---|
| `url` | text | `postgres://localhost/development` | none: secret | `$<APP>_CACHE__URL` |

`url` is a secret. A database URL can carry a password, and every user on the
host can read a process's arguments. So no `--cache.url` flag exists. Set it
in the environment or, with the `config` module, in `config/<env>.toml`. See
`docs/module-flags.md`.

## Examples

### Namespaces, keys and tags

These values check themselves when you make them, so a bad key fails before
it reaches SQL.

```rust
use brocanth_cache::{Namespace, Tag};

# fn main() -> Result<(), brocanth_cache::CacheError> {
// Change the version and deploy: nothing reads the old entries again.
let views = Namespace::versioned("views", "v2")?;
let key = views.key("home")?;
assert_eq!(key.namespace().as_str(), "views:v2");
assert_eq!(key.as_str(), "home");

assert_eq!(Tag::table("posts")?.as_str(), "posts");
assert_eq!(Tag::row("posts", 42)?.as_str(), "posts:42");

assert!(Namespace::new("has a space").is_err());
# Ok(())
# }
```

### Read through, with tags

`fetch` returns the stored value, or computes it. When many callers miss one
key at the same time, exactly one computes, under a lease, and the rest wait
for its value.

```rust,no_run
use std::time::Duration;
use brocanth_cache::{Cache, CacheExt, Namespace, PgCache, Tag};

# async fn render_home() -> Result<Vec<u8>, std::io::Error> { Ok(Vec::new()) }
# #[tokio::main]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
let cache = PgCache::connect("postgres://localhost/app").await?;
cache.migrate().await?; // or put PgCache::schema_sql() in a migration

let key = Namespace::new("views")?.key("home")?;
let page = cache
    .fetch(&key, Duration::from_secs(60), || async { render_home().await })
    .await?;

// Store with the tags of what the value read, then drop them all in one call.
let post = Tag::row("posts", 42)?;
cache.set_tagged(&key, &page, Duration::from_secs(60), &[post.clone()]).await?;
cache.invalidate_tag(&post).await?;
# Ok(())
# }
```

### Register the plugin

`CachePlugin` takes a cache that is already open, because a pool opens
asynchronously and `Plugin::new` is synchronous. A handler then reads the
`CacheHandle` from the context (`cx.get::<CacheHandle, _>()`). A handler in an
application without the plugin does not compile.

```rust,no_run
use std::sync::Arc;
use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth_cache::{CacheConfig, CachePlugin, PgCache};

# #[tokio::main]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
let cache = PgCache::connect("postgres://localhost/app").await?;
let app = AppBuilder::new()
    .plugin(CachePlugin::new(CacheConfig::new(Arc::new(cache))))
    .build()?;
# let _ = app;
# Ok(())
# }
```

### Cache whole pages

`RouteCache` answers a cached `GET` without running the handler. It stores a
response only when that is safe: no `Authorization`, no session cookie, status
200, a buffered body, no `Set-Cookie`, and no `Cache-Control: private` or
`no-store`.

```rust,no_run
use std::sync::Arc;
use std::time::Duration;
use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth_cache::route::{RouteCache, RouteCacheConfig, RouteRule};
use brocanth_cache::{PgCache, Tag};

# #[tokio::main]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
let cache = PgCache::connect("postgres://localhost/app").await?;
let routes = RouteCacheConfig::new(Arc::new(cache))
    .session_cookies(["sid"])
    .rule(RouteRule::path("/", Duration::from_secs(60)))
    .rule(
        RouteRule::prefix("/posts/", Duration::from_secs(300))
            .tags([Tag::table("posts")?])
            .vary([http::header::ACCEPT_LANGUAGE]),
    );
let app = AppBuilder::new()
    .plugin_hooking(RouteCache::new(routes))
    .build()?;
# let _ = app;
# Ok(())
# }
```

The three `no_run` blocks need a Postgres server. They compile in every
`cargo test`.

## Testing

```sh
cargo test -p brocanth-cache
```

The crate is Postgres only and has no in-memory backend, on purpose: a second
implementation would test SQL that nobody deploys. Without a database, every
Postgres test skips and prints a banner. Do not read a green run without the
variable as a pass. To run them:

```sh
BROCANTH_TEST_DATABASE_URL=postgres://localhost/brocanth_test cargo test -p brocanth-cache
```

`BROCANTH_CACHE_TEST_POSTGRES` overrides `BROCANTH_TEST_DATABASE_URL` for this
crate only. The route-cache hook tests and the panel tests run without a
server: they use a fake `Cache`. To test the panel, turn on `admin`:
`cargo test -p brocanth-cache --features admin`.

### Testing an application that uses this crate

Point a test at a real database with the same variable, call `migrate()` once,
and use a namespace per test, so tests do not see each other's entries.
`PgCache::with_clock(ManualClock::now_frozen())` makes expiry testable without
a sleep: `advance` the clock past the TTL. For a cached page, drive the
application with `brocanth-test`'s `TestClient` and read the
`brocanth-cache: hit` or `miss` response header.

## More

- The crate documentation (`cargo doc -p brocanth-cache --open`): `UNLOGGED`,
  the costs, when Redis is right, the stampede lease and the sweep.
- `docs/cache.md`: tags, the route cache, fragments and invalidation from
  model changes.
- `docs/module-flags.md`: how `[cache] url` is set, and why it is a secret.
