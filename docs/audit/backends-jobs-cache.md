# Backends audit: brocanth-cache, brocanth-jobs, brocanth-queue

Repo state: `main` at `25e908a`, 2026-11-09. Read-only audit. No build was run.
All paths are relative to `/Users/max/Documents/Dev/rust/brocanth`.

**Short answer.** Max is right, but the code is not half-built. Each crate's
documentation says that the missing backends are left out **on purpose**.
The real gaps are these:

1. Cache has one backend (Postgres). It has no memory, SQLite or Redis backend
   and no conformance suite that a second backend could use.
2. Two job systems exist side by side. They share only the read-side
   `Inspect` trait. No generated project uses `brocanth-queue`.
3. `brocanth-jobs` has a SQLite store, but no generated project can select it.
4. The scaffold modules do not connect what the crates already provide: the
   shared pool, the cache sweep and the admin panels.

---

## 1. Cache

### 1.1 Backends

| Backend | Exists? | Where |
|---|---|---|
| Postgres, `UNLOGGED` table | **yes, the only one** | `crates/brocanth-cache/src/postgres.rs:41` (`PgCache`), `:238` (`impl Cache`). DDL `src/postgres/sql.rs:111` (`CREATE UNLOGGED TABLE brocanth_cache`), `:134` (`brocanth_cache_tags`, also unlogged) |
| Memory | **no**, by decision | `tests/common/mod.rs:1-30` ("there is no in-memory backend, and there will not be one"). `README.md:176`, `docs/cache.md:121`. Test-only fakes exist but do not ship: `tests/route.rs:55` (`Map`), `tests/panel.rs:66,132`, `src/route.rs:644` |
| SQLite | **no**. The crate is ready for one in another crate | `Cargo.toml:106-122`: "SQLite is the 'smaller setups, different crate' case". `default-features=false, features=["runtime"]` gives you the trait without a driver. `PgCache::connect` refuses `sqlite://` with a pointer to "a separate crate" (`src/postgres.rs:80-98`, test `:425`) |
| Redis / Valkey | **no** | It is mentioned only as advice: "when Redis is right" (`src/lib.rs:107-138`, `README.md:9`). No crate and no code |

Only `PgCache` implements `Cache` in the workspace (grep `impl.*Cache for`).

### 1.2 The store trait

`crates/brocanth-cache/src/cache.rs:112-292`: `#[async_trait] pub trait Cache: Send + Sync + 'static`, held as `Arc<dyn Cache>`.

- Required methods: `max_value_bytes` (:118), `migrate` (:135), `get` (:146), `set` (:176), `set_tagged` (:188), `invalidate_tags` (:204), `add` (:219, set-if-absent), `delete` (:227), `clear(namespace)` (:240), `lease` (:257), `release_lease` (:273), `purge_expired(limit)` (:291).
- Default methods: `get_many` (:162, loops over `get`), `invalidate_tag` (:195).
- Values are bytes only. Namespaces are separate from keys (`src/key.rs:84,148`). Tags: `src/tag.rs:46`.
- **Not on the trait:** `stats()` and `is_unlogged()` exist only on `PgCache` (`src/postgres.rs:171,184`). Because of this, the admin panel shows no numbers (`src/admin.rs:8`).

The trait is backend-neutral. Nothing in it needs SQL. A Redis backend can map each method: `SET NX PX` for `add` and `lease`, a Lua compare-and-delete for `release_lease`, sets for tags, and a no-op for `purge_expired`. Only `clear(namespace)` would need `SCAN` or a namespace-version key.

### 1.3 Features × backend

Every feature above the store is written against `dyn Cache`, so a new backend would get all of them at no cost:

| Feature | Written against | Where |
|---|---|---|
| read-through `fetch` with single flight (lease) | `CacheExt` for any `C: Cache` | `src/fetch.rs:252,450` |
| fragment / `try_fragment` | `CacheExt` | `src/fetch.rs:394,422` |
| tags | trait methods | `src/cache.rs:188-204` |
| route cache (`RouteCache` plugin) | `Arc<dyn Cache>` | `src/route.rs:222,280` |
| `TagInvalidator` (`events` feature) | `Arc<dyn Cache>` | `src/invalidate.rs:36-47` |
| `sweep()` | `&dyn Cache` → `purge_expired` | `src/sweep.rs:124` |
| admin panel (`admin` feature) | `Arc<dyn Cache>` | `src/admin.rs:69-79` |
| stats / unlogged check | **`PgCache` only** | `src/postgres.rs:171,184` |

Gaps in practice:
- **Nothing schedules `sweep`.** The crate leaves that to `brocanth-jobs` (`src/sweep.rs:21-30`), but no code anywhere calls it: no scaffold, no jobs recurring job (grep `brocanth_cache::sweep`). A generated project with `cache` + `jobs` never reclaims expired rows.
- No eviction and no size bound beyond the TTL (`src/lib.rs:123-127`). Rails' Solid Cache has FIFO eviction.
- No conformance suite. `tests/postgres.rs` and `tests/tags.rs` are written against `PgCache` (`tests/tags.rs:15,25`). A second backend would have no shared tests to pass.

### 1.4 The generated `cache` module

`crates/brocanth-cache/src/scaffold.rs`:
- Manifest (:22-46): `adapts_to("config")` only. **It does not adapt to `db`**. There is no backend choice and no child module.
- Settings (:57-73): `url` (secret) and `migrate`. That is all.
- The generated plugin is hard-wired to `PgCache::connect_lazy(&self.url)` (:113, :179). It opens **its own pool**, but `PgCache::from_pool` exists for sharing the model's pool, and its doc comment recommends that (`src/postgres.rs:100-110`).
- The plugin does not register `CachePanel`, `RouteCache`, `TagInvalidator` or a sweep (grep: none of them appears outside the crate).
- `docs/modules/cache.md:41` lists only `url`. The `migrate` setting (added in `1642836`) is missing, so the page is stale.

**A project cannot choose anything about the cache backend.** It gets Postgres or no cache.

---

## 2. Jobs

### 2.1 Stores

`crates/brocanth-jobs` has **its own store**, not `brocanth-queue`'s `Backend`:

- `src/store.rs:76-83`: `pub enum Backend { Postgres(PgPool), Sqlite(SqlitePool) }`, which is a closed enum and not a trait. Each operation branches on the dialect inside `Store` (`src/store.rs:440-1243`), with SQL side by side in `src/store/sql.rs` (`*_PG` / `*_SQLITE`).
- Features (`Cargo.toml:148-150`): `default = ["runtime","postgres"]`, `postgres`, `sqlite` (`sqlx/sqlite-bundled`). Both use sqlx 0.9.
- If both features are off, the build fails with `compile_error!`, and the message refuses a memory backend on purpose (`src/lib.rs:158-167`, reasoning at `:28-32`).
- The URL scheme selects the dialect at run time (`src/queue.rs:810-850`). SQLite runs in WAL mode with `busy_timeout` 5 s and claims with `BEGIN IMMEDIATE` (`src/store.rs:25-48`).

| Store | Exists? |
|---|---|
| Postgres (sqlx) | yes, the default |
| SQLite (sqlx) | yes, behind the `sqlite` feature |
| Memory | no, by decision (`src/lib.rs:30-32,162-166`) |
| Redis / SQS / NATS | no |

### 2.2 Does jobs use `brocanth-queue`'s `Backend`? No. This is duplication.

`brocanth-jobs` depends on `brocanth-queue` **only for `Inspect`**, the read side (`Cargo.toml:21-26`, `src/inspect.rs:1-30,124`). The README says so too (`README.md:302-305`): "`brocanth-jobs` shares only the `Inspect` read side with `brocanth-queue`".

These pieces exist twice:

| Concept | brocanth-jobs | brocanth-queue |
|---|---|---|
| Job trait | `src/job.rs` (`Job`, `JobContext`, `JobId`) | `src/job.rs` (`Job`, `JobContext`, `JobId`, `Lane`) |
| Worker | `src/worker.rs` (892 lines) | `src/worker.rs` (834 lines) |
| Registry | `src/registry.rs` | `src/registry.rs` |
| Backoff | `src/backoff.rs:24` | `job::Backoff` |
| Events hook | `JobEvents` (`src/events.rs:256`) | `Observer` (`src/worker.rs:221`) |
| Postgres claim with `SKIP LOCKED` | `src/store/sql.rs` | `brocanth-queue-postgres/src/lib.rs` |
| Dead-letter table | `brocanth_jobs_dead` | `queue_dead` |

The data models also differ. Jobs keeps a `state` column. Queue derives the state from `available_at` + `lease_token` and fences each settle with a token (`brocanth-jobs/src/inspect.rs:8-13`).

### 2.3 Can `brocanth-queue-memory` or `-postgres` serve as a jobs backend?

**No.** `brocanth_jobs::Queue` accepts only `impl Into<brocanth_jobs::Backend>`, which means a sqlx pool (`src/queue.rs:147`). Jobs uses `Memory` and `PgQueue` **only in tests of its admin panel**, to prove that the panel renders over any `DynInspect` (`Cargo.toml:78-88`, `tests/panel.rs` `the_same_screens_render_over_another_queue_entirely`, `the_history_screen_plots_what_the_archive_kept`). Outside tests, nothing in the workspace depends on `brocanth-queue-postgres` or `-memory`, and no scaffold module installs them.

### 2.4 Features × store

| Feature | jobs/Postgres | jobs/SQLite | queue-memory | queue-postgres | Where |
|---|---|---|---|---|---|
| enqueue, delay, priority | yes | yes | yes | yes | jobs `src/queue.rs:239,622-660`; queue `enqueue.rs` |
| unique key | yes | yes | yes (Unbounded) | yes (Unbounded) | jobs `store.rs:549-600`; queue-pg `lib.rs:291-300` |
| claim without double run | yes, `SKIP LOCKED` | yes, serialised `BEGIN IMMEDIATE` | yes | yes, `SKIP LOCKED` | jobs `store.rs:4-48,620-667` |
| retries + backoff | yes | yes | yes (core worker) | yes (core worker) | jobs `backoff.rs`; queue `job.rs` |
| dead letter + requeue | yes | yes | yes | yes | jobs `queue.rs:276,375-385`; memory `lib.rs:165` |
| orphan reclaim / heartbeat | yes | yes | yes (lease expiry) | yes (lease + fence) | jobs `queue.rs:393`, `store.rs:757,949` |
| cron / recurring + misfire | **yes** | **yes** | **no** | **no** | jobs `cron.rs:50`, `queue.rs:440-575`; queue: no cron anywhere |
| LISTEN/NOTIFY wake-up | yes | no (polls) | n/a | yes | jobs `listen.rs:52,103`, `store.rs:828-835`; queue-pg `lib.rs:151,349,764` |
| enqueue inside the caller's tx | **no** | **no** | no | **yes** (`Tx`, `queue_tx` lane) | queue-pg `tx.rs:90`, `lib.rs:103-112` |
| outbox (bulk lane, atomic) | no | no | yes (`MemoryOutbox`) | yes (`PgOutbox` + relay) | memory `outbox.rs:123`; queue-pg `outbox.rs:45` |
| events / observer | yes (`JobEvents`) | yes | yes (`Observer`) | yes | |
| `Inspect` (operator reads) | yes | yes | yes | yes | jobs `inspect.rs:124`; memory `inspect.rs:89`; pg `inspect.rs:156` |
| history / archive chart | no (deletes on success) | no | no | yes (7-day partitions) | jobs `inspect.rs:25-30`; pg `inspect.rs:157,564`, `partitions.rs` |
| cancel / purge | discard only | discard only | yes | yes | queue `backend.rs:394-400` |
| admin panel (`JobsPanel`) | yes | yes | yes | yes, also history | jobs `admin.rs:109-124` (over `DynInspect`) |
| outbox dead-letter screen | n/a | n/a | not mounted | not mounted | `DECISIONS.md:392-393` |
| Capabilities flags | n/a | n/a | yes (unbounded / SQS-shaped) | yes | queue `backend.rs:51-140` |

### 2.5 Specific gaps

- **SQLite parity in features is good. Test parity is inverted.** The full suite (`tests/queue.rs` 17 tests, `tests/recurring.rs` 10, `tests/panel.rs`) runs **on SQLite** (`tests/common/mod.rs:35`). The default backend, Postgres, gets 9 targeted tests (`tests/postgres.rs`), and those are skipped when no database is set. `every_statement_is_valid_postgres` (:83) prepares each statement, but behaviour such as unique keys, shutdown, `run_now`/`discard` and stop-when is not proven on Postgres.
- **SQLite cannot be reached from a generated project.** The `jobs` scaffold adds `Dep::brocanth("brocanth-jobs")` with default features, so it gets Postgres only (`src/scaffold.rs:56`). The `db` module is Postgres only (`crates/brocanth-db/src/scaffold.rs:35,110-113`). A `sqlite://` URL in `[jobs] url` fails at startup with "no SQLite driver" (`src/queue.rs:840-845`).
- **No memory store** for dev and tests. This conflicts with the rest of the workspace: `brocanth-auth` (`src/memory.rs:85`), the rate limit (`brocanth-middleware/src/rate_limit/memory.rs:99`), `brocanth-analytics` (`src/store.rs:234`) and `brocanth-queue-memory` all ship one.
- **The conformance suite does not cover jobs.** `brocanth-queue-conformance` runs `Backend` cases (19 plus 2 gated by capabilities) and outbox cases (5) against memory, the adversary and Postgres (`brocanth-queue-conformance/src/lib.rs:163,206`, `tests/memory.rs:17-30`, `brocanth-queue-postgres/tests/postgres.rs:70,80,252`). It has **no `Inspect` cases and no `TxEnqueue` cases**. `brocanth-jobs` cannot run it, because jobs does not implement `Backend`.
- The scaffold opens its own pool from `[jobs] url` (`src/scaffold.rs:91-97`). It does not adapt to `db`. It wires no `JobsPanel`, and it schedules no cache sweep when `cache` is present.

---

## 3. Gap table

`yes` / `partial` / `no`. "n/a" means the feature does not apply to that crate.

### Cache

| Feature | Postgres | Memory | SQLite | Redis/Valkey |
|---|---|---|---|---|
| backend exists | yes | no (test fakes only) | no | no |
| get/set/add/delete/clear | yes | no | no | no |
| tags + invalidate | yes | no | no | no |
| lease / `fetch` single flight | yes | no | no | no |
| fragment | yes | no | no | no |
| route cache | yes | no | no | no |
| `TagInvalidator` | yes | no | no | no |
| `purge_expired` / sweep fn | yes | no | no | no |
| sweep actually scheduled | **no** | no | no | no |
| admin panel | yes (no stats) | no | no | no |
| stats | partial (inherent method, not on the trait) | no | no | no |
| eviction / size bound | no (TTL only) | no | no | no |
| conformance suite | no | no | no | no |
| selectable in scaffold | yes (only choice) | no | no | no |
| shares the `db` pool in scaffold | no | n/a | n/a | n/a |

### Jobs / queue

| Feature | jobs-Postgres | jobs-SQLite | jobs-Memory | queue-Memory | queue-Postgres | queue-SQLite | queue-Redis/SQS |
|---|---|---|---|---|---|---|---|
| backend exists | yes | yes | no | yes | yes | no | no |
| retries/backoff | yes | yes | no | yes | yes | no | no |
| dead letter | yes | yes | no | yes | yes | no | no |
| unique jobs | yes | yes | no | yes | yes | no | no |
| cron / recurring | yes | yes | no | **no** | **no** | no | no |
| LISTEN/NOTIFY | yes | no (polls) | n/a | n/a | yes | n/a | n/a |
| tx enqueue | no | no | no | no | yes | no | no |
| outbox | no | no | no | yes | yes | no | no |
| events | yes | yes | no | yes | yes | no | no |
| `Inspect` | yes | yes | no | yes | yes | no | no |
| history | no | no | no | no | yes | no | no |
| admin panel | yes | yes | no | yes | yes | no | no |
| conformance suite | **no** | **no** | no | yes | yes | no | no |
| full test suite runs on it | partial (9 tests) | yes | no | yes | yes | no | no |
| selectable in scaffold | yes (only choice) | **no** | no | no | no | no | no |
| shares the `db` pool in scaffold | no | n/a | n/a | n/a | n/a | n/a | n/a |

---

## 4. Recommendations, ranked

| # | What | Why | Effort |
|---|---|---|---|
| 1 | **Fix the scaffold wiring that already has crate support.** (a) `cache` and `jobs`: `adapts_to("db")`. With `db` present, use `PgCache::from_pool` / `Queue::from_pool` on the `db` pool and drop their own `url` setting. Today a project with `db`+`jobs`+`cache` opens 3 pools to one database. Precedent: `analytics` reads the `db` pool (`crates/brocanth-analytics/src/scaffold.rs:47-59`). (b) With `cache`+`jobs`, install a recurring `CacheSweep` job. (c) `adapts_to("admin")`: add `CachePanel` / `JobsPanel` through `Slot::AdminPanels` (module-config §7.3 pattern). (d) Regenerate `docs/modules/cache.md`. | Small. It uses only code that exists. It closes the "sweep never runs" defect. | **S** |
| 2 | **A cache conformance suite**: `brocanth_cache::testing::conformance(Arc<dyn Cache>)` (or a `brocanth-cache-conformance` crate like the queue one). Move the trait-level cases from `tests/postgres.rs` and `tests/tags.rs` into it. Keep the SQL-specific checks (`UNLOGGED`, HOT updates) in the Postgres tests. | This must come before any second cache backend. It answers the crate's own objection that "a second implementation behaves differently" by proving the opposite. | **S–M** |
| 3 | **`MemoryCache`** behind a `memory` feature, run through suite #2. Use: tests, `brocanth dev` without Postgres, single-process apps. | Aligns cache with auth, rate limit, analytics and queue, which all ship a memory store. Needs decision D1. | **S** |
| 4 | **Decide one job system** (D2), then do one of: **(a)** make `brocanth-queue` the engine and turn `brocanth-jobs` into a facade: port cron/misfire, `JobEvents` and unique keys onto `brocanth-queue`, and add a `brocanth-queue-sqlite` adapter if SQLite stays (**L**); **(b)** keep `brocanth-jobs` as the product, and merge `brocanth-queue`'s tx lane and outbox into it (**M–L**); **(c)** keep both, and add a jobs `Backend` impl so that the conformance suite runs on it (**M**, schema change: lease token). | Two Workers, two Job traits and two dead-letter tables in one framework cost the most maintenance of anything here, and a model writing brocanth code will be confused by them. | **L** (a) / **M–L** (b) / **M** (c) |
| 5 | **Run the jobs suite on both dialects.** Use a macro or `rstest` that runs `tests/queue.rs` and `tests/recurring.rs` on SQLite and also, when the database is set, on Postgres. | The default backend is the one tested less today. | **S** |
| 6 | **`brocanth-cache-redis`** (Valkey-compatible, `redis` or `fred` crate), as its own crate per `Cargo.toml:107-113`, run through suite #2. Scaffold: child module `cache-redis`. | The crate's own docs tell users to "use Redis when…" but offer no route. Needs D3. | **M** |
| 7 | **`inspect` and `tx` conformance cases** in `brocanth-queue-conformance`. | The panel depends on `Inspect` semantics that no shared test checks. | **S** |
| 8 | **SQLite cache / SQLite generated projects.** Only after `db` gets a dialect choice. Then: a `sqlite` feature on the `jobs` dep, and a `brocanth-cache-sqlite` crate (a normal table with WAL, no `UNLOGGED`). | SQLite jobs exists now but no project can reach it. A SQLite cache is useless without a SQLite project. Needs D4. | **M** (db dialect) + **M** (cache-sqlite) |
| 9 | Mount `DynOutboxInspect` in `JobsPanel` (`DECISIONS.md:392`). | Open item in DECISIONS Part 5. | **S** |

### How a project should pick a backend

- **Not a config setting.** A backend changes `Cargo.toml` (a crate or a feature), and a value in `config/*.toml` cannot do that.
- **Not a `src/modules/` static option.** `docs/module-config.md:577-589` (§2.4, §2.5) says that enum options are deferred and that "a constant cannot remove a crate from `Cargo.toml`".
- **Use child modules** for each backend other than the default: `cache-redis`, `cache-memory`, `jobs-sqlite`, each `part_of_but_off("cache")` / `("jobs")`, with `.conflicts()` between siblings. The parent keeps Postgres as the default. Precedent: `realtime-sse`/`realtime-websocket` (`crates/brocanth-realtime/src/scaffold.rs:114,577`) and `docker-kubernetes`. A child module carries its own `Dep` (crate + feature) and its own `[cache]` settings, and `brocanth add`/`remove` already handle it.
- **Use `adapts_to("db")` for pool sharing**, not for choosing the backend. For SQLite, once `db` has a dialect, `jobs` can follow it with `adapts_to("db")` + `Dep::feature_with`. Precedent: `auth` switches between `MemoryStore` and `PgStore` from `project.has(...)` (`crates/brocanth-auth/src/scaffold.rs:166-173`).
- A memory backend for tests is not a module. Expose it as a dev-dependency or a `testing` helper.

---

## 5. Decisions for Max

- **D1. Keep "no in-memory backend"?** Cache and jobs both refuse one on purpose (`brocanth-cache/tests/common/mod.rs:1-30`, `brocanth-jobs/src/lib.rs:30-32,158-167`). Auth, rate limit, analytics and queue ship one. Options: keep the refusal everywhere / allow a memory backend for tests only, gated by a conformance suite / allow a full dev backend.
- **D2. One job system or two?** Choose 4a (queue becomes the engine), 4b (jobs absorbs tx and outbox) or 4c (keep both, add conformance). This decision blocks recommendations 4, 7 and 9, and also any SQS or Redis queue work.
- **D3. Is Redis/Valkey in scope** as a first-party `brocanth-cache-redis` (and later a queue `Backend`)? Or is "the way out" left to users through the trait?
- **D4. Is SQLite a target for generated projects?** If yes, `db` needs a dialect choice first. If no, think about whether to drop the `sqlite` feature in jobs, or at least move it out of the scaffold story, because nothing selects it today.
- **D5. Pool sharing as the default:** with `db` present, should `cache`/`jobs` reuse its pool and drop their own `url`? Or keep separate URLs so that a project can move the cache to another database?
- **D6. The selection mechanism:** child modules per backend (recommended), or wait for enum options in `src/modules/` (which cannot change Cargo deps without generator support).
