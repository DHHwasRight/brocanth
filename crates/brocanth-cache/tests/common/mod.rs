//! The test story for a Postgres-only crate, and the one rule it follows:
//! **a test that did not run must say so.**
//!
//! This crate's SQL is Postgres only, so its SQL tests need a server.
//! `MemoryCache` exists for tests and development, and it passes the
//! conformance suite, but it runs none of the SQL these tests are about, so
//! it does not replace them.
//!
//! So there are three ways this could have gone, and here is the one chosen and
//! why the other two were not.
//!
//! **A testcontainer** (`testcontainers-rs`) starts a Postgres in Docker for
//! the suite. Rejected: it puts a container runtime in the development
//! dependencies of a framework crate, it fails on a machine with no Docker —
//! which is most of them, on this workspace's own platform, as the one that
//! wrote this found out — and the failure looks like a broken test rather than
//! a missing tool. It also makes `cargo test` download an image the first time,
//! which is a surprise nobody consents to.
//!
//! **A mock, or `MemoryCache`, for these tests to run against.** Rejected
//! for this file. It would test the trait against an implementation nobody deploys
//! and prove nothing about the SQL, which is where every interesting decision
//! in this crate lives — the `ON CONFLICT … WHERE` that makes the lease atomic,
//! the `WITH ORDINALITY` join, `ctid`, `UNLOGGED`. A green suite that never
//! touched Postgres would be worse than no suite, because it would be
//! believed.
//!
//! **An environment variable naming a database, and a loud skip otherwise.**
//! Chosen, and it is what `brocanth-jobs/tests/postgres.rs` already does in
//! this workspace.
//!
//! ```sh
//! BROCANTH_CACHE_TEST_POSTGRES=postgres://localhost/brocanth_cache_test \
//!   cargo test -p brocanth-cache
//! ```
//!
//! `BROCANTH_TEST_DATABASE_URL` is read as a fallback, so one variable can
//! serve every crate in the workspace that needs a server.
//!
//! The loudness is the part that matters. A suite that passes by running
//! nothing is a lie that survives CI, so: every skipped test prints a banner
//! naming itself, and [`announce`] prints the reason once with the command to
//! fix it. A CI job for this crate should set the variable and should fail if
//! it is not set.

#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use brocanth_cache::{Cache, ManualClock, Namespace, PgCache};
use chrono::{TimeZone, Utc};

/// The URL of a database to test against, from either variable. An empty
/// variable counts as unset.
pub fn test_database_url() -> Option<String> {
    ["BROCANTH_CACHE_TEST_POSTGRES", "BROCANTH_TEST_DATABASE_URL"]
        .iter()
        .find_map(|name| {
            std::env::var(name)
                .ok()
                .filter(|url| !url.trim().is_empty())
        })
}

/// Writes straight to the real standard error, bypassing libtest's capture.
///
/// This is the part that makes the skip actually loud. `eprintln!` goes
/// through `std::io::_eprint`, which libtest redirects into a per-test buffer
/// that it prints **only when the test fails** — so a skip announced with
/// `eprintln!` is invisible in exactly the run where it matters, and the suite
/// reports fourteen passes for fourteen tests that did nothing. A write to the
/// `Stderr` handle is not captured and reaches the terminal.
fn shout(message: &str) {
    use std::io::Write as _;
    let _ = std::io::stderr().write_all(message.as_bytes());
    let _ = std::io::stderr().flush();
}

/// Prints the banner, once per test binary, when there is no database.
pub fn announce() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        if test_database_url().is_none() {
            shout(
                "\n\
                 ============================================================\n\
                 brocanth-cache: NO DATABASE. EVERY POSTGRES TEST IS SKIPPED.\n\
                 ============================================================\n\
                 These tests are about the Postgres SQL, and MemoryCache runs\n\
                 none of it, so they prove nothing until you point them at a\n\
                 server:\n\n\
                 \x20 BROCANTH_CACHE_TEST_POSTGRES=postgres://localhost/brocanth_cache_test \\\n\
                 \x20   cargo test -p brocanth-cache\n\n\
                 A green run without that variable means the suite did not\n\
                 run. Do not read it as a pass.\n\
                 ============================================================\n",
            );
        }
    });
}

/// A cache against the test database, migrated, or a loud skip and `None`.
///
/// Every test starts with
/// `let Some(cache) = common::cache("its_own_name").await else { return };`
/// so a skipped test names itself on the way out.
pub async fn cache(test: &str) -> Option<PgCache> {
    announce();
    let Some(url) = test_database_url() else {
        shout(&format!("*** SKIPPED (no database): {test}\n"));
        return None;
    };
    let cache = PgCache::connect(&url)
        .await
        .expect("connecting to Postgres");
    cache.migrate().await.expect("creating the cache table");
    // One database serves every test in the file and they run in parallel, so
    // nothing here truncates anything: each test works inside its own
    // namespace, which is also what two features sharing a cache would do.
    Some(cache)
}

/// A cache on a table of its own, in a new schema, or a loud skip and `None`.
///
/// For a test that sweeps. A sweep deletes every expired row in the table,
/// in every namespace, so a namespace does not isolate it. A sweep at the
/// real clock deletes the rows of a test on a [`ManualClock`], whose
/// instants are years in the past. A sweep at a moved clock deletes the
/// live rows of other tests. Drop the schema with [`drop_schema`].
pub async fn cache_in_own_schema(test: &str) -> Option<(PgCache, String)> {
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    use std::str::FromStr as _;

    announce();
    let Some(url) = test_database_url() else {
        shout(&format!("*** SKIPPED (no database): {test}\n"));
        return None;
    };
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let schema = format!(
        "t_cache_{}_{}",
        std::process::id(),
        COUNT.fetch_add(1, Ordering::Relaxed)
    );
    let options = PgConnectOptions::from_str(&url).expect("a Postgres URL");
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options.clone())
        .await
        .expect("connecting to Postgres");
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP SCHEMA IF EXISTS {schema} CASCADE"
    )))
    .execute(&admin)
    .await
    .expect("dropping a stale schema");
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
        .execute(&admin)
        .await
        .expect("creating the schema");
    admin.close().await;
    let pool = PgPoolOptions::new()
        .connect_with(options.options([("search_path", schema.as_str())]))
        .await
        .expect("connecting to Postgres");
    let cache = PgCache::from_pool(pool);
    cache.migrate().await.expect("creating the cache table");
    Some((cache, schema))
}

/// Drops a schema that [`cache_in_own_schema`] made.
pub async fn drop_schema(cache: &PgCache, schema: &str) {
    let _ = sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP SCHEMA IF EXISTS {schema} CASCADE"
    )))
    .execute(cache.pool())
    .await;
}

/// A namespace no other test is using.
///
/// Tests share one database, so isolation is by name rather than by truncation.
pub fn namespace(label: &str) -> Namespace {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let n = COUNT.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    Namespace::new(format!("t.{label}.{pid}.{n}")).expect("a usable namespace")
}

/// A cache whose clock a test moves by hand, so expiry is exact rather than
/// slept for.
pub async fn cache_with_manual_clock(test: &str) -> Option<(PgCache, ManualClock)> {
    let clock = ManualClock::new(Utc.timestamp_opt(1_700_000_000, 0).unwrap());
    let cache = cache(test).await?.with_clock(clock.clone());
    Some((cache, clock))
}

/// Removes everything a test made. Called at the end of each test so a shared
/// database does not fill up with namespaces.
pub async fn clean_up(cache: &PgCache, namespace: &Namespace) {
    let _ = cache.clear(namespace).await;
}

/// A duration spelled out, for readability at a call site.
pub const MINUTE: Duration = Duration::from_secs(60);
