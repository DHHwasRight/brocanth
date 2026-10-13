//! The test story for a Postgres-only crate, and the one rule it follows:
//! **a test that did not run must say so.**
//!
//! `brocanth-jobs` can run its whole suite on SQLite, on a laptop, with no
//! server. This crate cannot: it is Postgres only on purpose, there is no
//! in-memory backend, and there will not be one — an in-memory cache is a
//! second implementation, and a second implementation is a thing that behaves
//! differently from the one you deploy, which is the failure mode the whole
//! workspace is built to avoid.
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
//! **A mock, or an in-memory `Cache` for the tests to run against.** Rejected
//! outright. It would test the trait against an implementation nobody deploys
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

/// The URL of a database to test against, from either variable.
pub fn test_database_url() -> Option<String> {
    std::env::var("BROCANTH_CACHE_TEST_POSTGRES")
        .or_else(|_| std::env::var("BROCANTH_TEST_DATABASE_URL"))
        .ok()
        .filter(|url| !url.trim().is_empty())
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
                 This crate is Postgres only and has no in-memory backend, so\n\
                 the tests below prove nothing until you point them at a\n\
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
