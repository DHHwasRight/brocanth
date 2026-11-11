# brocanth-cache-conformance

The `brocanth-cache` contract as an executable suite. Every `Cache`
backend runs it: `PgCache`, `MemoryCache` and `RedisCache` today. A
backend that passes keeps every promise the trait documents, so code
written against one backend behaves the same on another.

The cases cover values as bytes, `get_many` order, expiry, `add`, the zero
TTL, namespaces, the size cap, leases, `fetch` and its single flight,
tags, fragments and the sweep. Nothing in a case names a table, a
connection or a command.

Use it in the test suite of a backend. Do not use it to test an
application: it tests storage, not caching decisions.

## Getting started

```toml
[dev-dependencies]
brocanth-cache-conformance = { git = "https://github.com/DHHwasRight/brocanth.git" }
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

Inside this workspace: a path dependency on `../brocanth-cache-conformance`.

```rust
use std::sync::Arc;

use brocanth_cache::MemoryCache;

# #[tokio::main(flavor = "multi_thread", worker_threads = 4)]
# async fn main() {
let report = brocanth_cache_conformance::run("memory", Arc::new(MemoryCache::new())).await;
report.assert_ok();
println!("{report}");
# }
```

`assert_conformant(name, cache)` does the same in one call.

## Rules for a run

- The cache must be migrated, or have nothing to migrate.
- Every name a run writes starts with a prefix unique to the run, so two
  runs against one live server do not meet. A run clears nothing it did
  not write.
- The expiry cases sleep past a TTL of 400 ms, because a server-side
  backend keeps its own clock. A run takes a few seconds.
- `purge_expired` may answer 0: a backend whose server expires keys by
  itself has nothing to purge. The suite checks only that a purge keeps
  live entries and their tags.
