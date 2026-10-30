//! A cache kept in Postgres, so a deployment that already runs Postgres gets a
//! cache without running Redis.
//!
//! This is a plugin. The core is a router handing a request and a context to a
//! controller, it has exactly three dependencies, and a test in it fails the
//! build if a fourth kind arrives — no SQL driver, none of what is in here. The
//! arrow points from this crate to the core and never back, so an application
//! that does not name [`CachePlugin`] in its `main` links no `sqlx` and no
//! cache.
//!
//! ```no_run
//! use std::time::Duration;
//! use brocanth_cache::{Cache, CacheExt, Namespace, PgCache};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let cache = PgCache::connect("postgres://localhost/app").await?;
//!     cache.migrate().await?;
//!
//!     let views = Namespace::versioned("views", "v1")?;
//!     let key = views.key("home")?;
//!
//!     // Read through: exactly one caller computes, however many miss at once.
//!     let page = cache
//!         .fetch(&key, Duration::from_secs(60), || async { render_home().await })
//!         .await?;
//!
//!     let _ = page;
//!     Ok(())
//! }
//! # async fn render_home() -> Result<Vec<u8>, std::io::Error> { Ok(Vec::new()) }
//! ```
//!
//! # The claim, and the honest version of it
//!
//! A cache in a relational database is an unusual choice, and the usual
//! objection is the right one: a cache is the most write-heavy thing an
//! application has, and a relational database makes writes expensive on
//! purpose. Every row written to an ordinary Postgres table is written twice —
//! once to the write-ahead log, once to the heap — then shipped to every
//! replica and every archive. Paying that to store something you are
//! deliberately willing to lose is absurd.
//!
//! So this crate does not pay it. **The cache table is `UNLOGGED`**, and that
//! single decision is what makes the rest defensible.
//!
//! ## `UNLOGGED`, and why a cache is the one thing it is right for
//!
//! An unlogged table writes no WAL at all. The write path is several times
//! cheaper, nothing is shipped to a replica, and nothing is archived. In
//! exchange, Postgres **truncates every unlogged table on crash recovery**: it
//! has no log, so it cannot prove the pages are consistent, and it throws them
//! away rather than serve something it cannot vouch for.
//!
//! For almost any table that is a catastrophe. For a cache it is the correct
//! behaviour, and more than that it is the behaviour you want. **A cache that
//! survives a crash is a cache you have to invalidate.** Entries written by the
//! process that died, about state that may have been mid-transaction, are
//! exactly the entries you do not want served to the next request. An empty
//! cache after an unclean shutdown is not data loss; it is the only state you
//! can reason about.
//!
//! ## The costs, stated plainly
//!
//! Three of them are real, and you should decide about each one before you use
//! this.
//!
//! **1. A replica does not have this table.** Unlogged tables are not
//! replicated — not by streaming replication, not by logical replication. A
//! read replica cannot serve the cache, so a cache read is always a read
//! against the primary, on a connection the primary could have used for
//! something else. If your architecture is "send reads to the replicas", this
//! crate quietly sends a class of reads back to the primary. It also means a
//! failover starts with a cold cache, on a database that is already having a
//! bad day.
//!
//! **2. A restart is a miss storm.** The truncation is silent and total: one
//! unclean shutdown and every entry is gone at once. Everything the cache was
//! protecting — the expensive query, the upstream API, the rendering — gets the
//! full load, at the moment the system is least able to take it. The
//! read-through path in [`fetch`] is the mitigation and it is a real one,
//! because it collapses a thousand concurrent misses on one key into one
//! computation. It does not help with a thousand misses on a thousand
//! *different* keys. Stagger your TTLs, and know that your origin must survive
//! a cold start.
//!
//! **3. Dead tuples, and the vacuum race.** Postgres never updates a row in
//! place: an overwrite writes a new version and leaves the old one dead for
//! autovacuum to reclaim. A cache overwrites the same hot keys constantly, so
//! this table makes garbage faster than anything else in your database. Left
//! alone it bloats, and a bloated table takes more of `shared_buffers`, which
//! is memory your real queries wanted.
//!
//! What this crate does about it: the DDL sets `fillfactor = 80` and lowers
//! this table's autovacuum thresholds to 2%, and there is **no index on
//! `expires_at`**, so an overwrite changes no indexed column and can be a HOT
//! update — new version on the same page, no index entries written, no index
//! bloat. The argument is spelled out at [`PgCache::schema_sql`].
//!
//! What the operator should watch: `dead_tuples` and `last_vacuum` from
//! [`PgCache::stats`]. Dead tuples climbing while `last_vacuum` stands still is
//! autovacuum losing the race, and it is visible there long before anything
//! looks broken.
//!
//! # When this is the wrong answer and Redis is right
//!
//! Use Redis, and do not argue about it, when:
//!
//! * **The cache is hot enough to be a second workload.** Tens of thousands of
//!   cache operations per second against the same primary that is serving your
//!   application's real queries is two systems sharing one connection pool, one
//!   buffer cache and one set of autovacuum workers. They will contend, and the
//!   thing that degrades is the database the application cannot run without.
//! * **You are already sending reads to replicas.** Cost 1 above is not a
//!   detail in that architecture; it is the architecture.
//! * **You want what Redis is actually for.** Pub/sub, sorted sets, streams,
//!   `INCR` as a rate limiter, blocking pops, per-field hash expiry. This crate
//!   stores bytes under a key with an expiry. Everything else on that list is a
//!   reason Redis exists, and reimplementing one of them on top of a table is
//!   how you end up with a worse Redis.
//! * **You need an eviction policy.** Redis has `maxmemory` and LRU. This has a
//!   TTL and a sweep, and if you write more than fits, Postgres fills the disk
//!   that your durable data is also on. A cache with no bound is only safe when
//!   you can bound it yourself, with TTLs and with
//!   [`Cache::max_value_bytes`].
//! * **The cache must outlive a crash.** If the answer to "what happens on an
//!   unclean restart" is anything other than "it refills", the `UNLOGGED`
//!   decision is wrong for you, and without it this crate's main argument is
//!   gone.
//!
//! Use this, and stop thinking about it, when: you run one Postgres, your cache
//! is thousands to low millions of entries, your hit path is a primary-key
//! lookup, and adding a second stateful service to the deployment would cost
//! more in operations than the cache saves. That is most small and medium
//! applications, and for them a second service to run, monitor, back up, patch
//! and fail over is a worse trade than a table.
//!
//! # Where the arguments are
//!
//! Each decision is argued where it lives, because an argument far from the
//! code it justifies rots:
//!
//! * [`PgCache::schema_sql`] — `UNLOGGED`, the `fillfactor`, and why the sweep
//!   is a sequential scan on purpose.
//! * [`fetch`] — **the stampede**: why a lease rather than an advisory lock,
//!   and the specific way advisory locks fail here.
//! * [`sweep`] — why a TTL must not depend on a sweeper, and why the thing that
//!   schedules the sweep belongs in `brocanth-jobs` rather than here.
//! * [`key`] — why a namespace is a column rather than a prefix, and the
//!   version trick that invalidates a whole class of entries without deleting a
//!   row.
//! * [`cache`] — why values are bytes and this crate will never see your
//!   structs.
//! * [`clock`] — why time is a `BIGINT` and a parameter.
//! * [`plugin`] — the lifecycle, and why this plugin starts nothing.
//! * [`tag`] — what a tag invalidation promises, and what it does not.
//! * [`route`] — the route cache, and every reason it refuses to store.
//! * [`CacheExt::fragment`] — a rendered fragment, under the same lease as
//!   `fetch`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// The cache's tab in `brocanth-admin`.
///
/// Behind an off-by-default feature, so a process that only reads the
/// cache links neither the admin nor a renderer.
#[cfg(feature = "admin")]
pub mod admin;
pub mod cache;
pub mod clock;
pub mod error;
pub mod fetch;
pub mod key;
pub mod plugin;
pub mod route;
pub mod sweep;
pub mod tag;

#[cfg(feature = "events")]
pub mod invalidate;

#[cfg(feature = "postgres")]
pub mod postgres;

/// The `brocanth create` module that writes this crate into a new project.
///
/// Behind the `scaffold` feature, which is off: the generator is the CLI's
/// business and not an application's.
#[cfg(feature = "scaffold")]
pub mod scaffold;

/// The panel's screen, compiled in by rsx at macro expansion: nothing is
/// read from disk at run time.
///
/// At the crate root and not inside [`admin`], because a template's own
/// `use` line names this path. Private, like the admin's own template
/// module, which is also what keeps `missing_docs` off generated items.
#[cfg(feature = "admin")]
mod admin_templates {
    rsx::load_dir!("src/admin_templates");
}

pub use cache::{expires_at, Cache, DEFAULT_MAX_VALUE_BYTES};
pub use clock::{from_millis, to_millis, Clock, ManualClock, SystemClock};
pub use error::{CacheError, Result};
pub use fetch::{CacheExt, FetchError, FetchPolicy};
pub use key::{Key, Namespace, MAX_KEY_BYTES, MAX_NAMESPACE_BYTES};
pub use plugin::{CacheConfig, CacheHandle, CachePlugin};
pub use sweep::sweep;
pub use tag::{Tag, MAX_TAG_BYTES};

#[cfg(feature = "events")]
pub use invalidate::TagInvalidator;

#[cfg(feature = "postgres")]
pub use postgres::{CacheStats, PgCache};

/// Everything an application that caches needs, in one `use`.
pub mod prelude {
    pub use crate::{Cache, CacheConfig, CacheExt, CacheHandle, CachePlugin, Key, Namespace, Tag};

    #[cfg(feature = "postgres")]
    pub use crate::PgCache;
}
