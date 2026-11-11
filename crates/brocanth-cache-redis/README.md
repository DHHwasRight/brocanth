# brocanth-cache-redis

A `brocanth-cache` backend on Redis or Valkey. It implements the same
`Cache` trait as `PgCache`, and it passes the same conformance suite
(`brocanth-cache-conformance`). So `fetch`, fragments, tags, the route
cache, `TagInvalidator` and the admin panel work on it with no change.

Use it when the cache outgrew a Postgres table: a cache hot enough to be a
second workload, reads that go to replicas, or a need for an eviction
policy (`maxmemory`). The `brocanth-cache` documentation lists the cases.
For everything else, the Postgres cache is one moving part fewer.

## Getting started

```toml
[dependencies]
brocanth-cache = { git = "https://github.com/DHHwasRight/brocanth.git" }
brocanth-cache-redis = { git = "https://github.com/DHHwasRight/brocanth.git" }
```

In a generated project, `brocanth add cache-redis` writes the plugin and
the `[cache] url` setting.

```rust,no_run
use std::sync::Arc;
use std::time::Duration;

use brocanth_cache::{Cache, CacheExt, Namespace};
use brocanth_cache_redis::RedisCache;

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
// One PING before it answers. `RedisCache::open` connects on the first
// command instead, for a startup stage that must not wait.
let cache = RedisCache::connect("redis://localhost:6379").await?;
let cache: Arc<dyn Cache> = Arc::new(cache.with_prefix("my-app:cache"));

let views = Namespace::versioned("views", "v1")?;
let page = cache
    .fetch(&views.key("home")?, Duration::from_secs(60), || async {
        Ok::<_, std::io::Error>(b"<h1>Home</h1>".to_vec())
    })
    .await?;
# let _ = page;
# Ok(())
# }
```

## What differs from Postgres

- **Expiry is the server's.** Redis deletes an expired key by itself, so
  `purge_expired` answers 0 and a sweep is not needed.
- **No migration.** `migrate` does nothing.
- **Tags are sets.** A tagged write runs one Lua script that stores the
  value and its tags together. A tag set lives as long as its longest
  entry, so it goes away by itself.
- **`clear` scans.** It walks the namespace with `SCAN MATCH` and unlinks
  what it finds. Prefer `Namespace::versioned` for an invalidation at
  deploy time, as with Postgres.

## Limits

- **Not Redis Cluster.** A script touches an entry and its tag sets, and a
  cluster refuses a script whose keys sit in two slots. One server, a
  primary with replicas, or Sentinel all work.
- **No TLS yet.** The client is built without a TLS stack, so a
  `rediss://` URL fails to connect. Reach a managed server over a private
  network or a TLS proxy until a `tls` feature lands.
- **The cap is 1 MiB by default**, as for Postgres. A large value blocks
  the server while it copies it. Change it with `with_max_value_bytes`.

## Tests

The conformance suite runs when `BROCANTH_CACHE_TEST_REDIS` names a
server, and the tests say they skipped otherwise:

```sh
docker run -d --rm --name brocanth-redis -p 6390:6379 redis:7-alpine
BROCANTH_CACHE_TEST_REDIS=redis://localhost:6390 cargo test -p brocanth-cache-redis
docker stop brocanth-redis
```

`valkey/valkey` works the same way.

## Why `redis` and not `fred`

`redis` (redis-rs) is the client the Redis project maintains, and the one
with the most users. Its `ConnectionManager` reconnects by itself, and
`new_lazy_with_config` opens nothing until the first command, which is what
a synchronous startup stage needs. `Script` runs Lua by hash and loads it
again after a server restart. `fred` adds cluster routing, pools and a mock
that this crate does not use.
