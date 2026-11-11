#![cfg_attr(feature = "runtime", doc = include_str!("../README.md"))]
//!
//! # How the trait maps onto Redis
//!
//! | `Cache` | Redis |
//! |---|---|
//! | `get`, `get_many` | `GET`, `MGET` |
//! | `set` | a script: `SET … PX`, and the old tags live as long as the value |
//! | `set_tagged` | a script: `SET … PX`, drop the old tags, add the new ones |
//! | `add`, `lease` | `SET … NX PX` |
//! | `release_lease` | a script: delete only if the value is still a lease |
//! | `invalidate_tags` | a script: every member of every tag set, deleted once |
//! | `clear` | `SCAN MATCH` on the namespace, then `UNLINK` |
//! | `purge_expired` | nothing: Redis expires keys by itself, and answers 0 |
//!
//! A stored value is one byte of kind, then the bytes: `v` for a value, `l`
//! for a lease. So a lease is a key like any other, `SET NX` sees it, and
//! `GET` tells it from a value.
//!
//! Each script is atomic on the server, so a tagged write never leaves a
//! value without its tags, and an invalidation never counts an entry twice.
//!
//! # Not Redis Cluster
//!
//! The scripts touch an entry and its tag sets in one call, and a cluster
//! refuses a script whose keys sit in two slots. Use one server, a primary
//! with replicas, or Sentinel. Valkey works the same way.

#![forbid(unsafe_code)]

#[cfg(feature = "runtime")]
mod cache;

#[cfg(feature = "runtime")]
pub use cache::{RedisCache, DEFAULT_PREFIX};

/// The `cache-redis` module that `brocanth create` offers.
///
/// Behind the `scaffold` feature, which is off.
#[cfg(feature = "scaffold")]
pub mod scaffold;
