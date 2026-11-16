//! The embedded server, started for real: under the feature `embedded`
//! only (`cargo test -p brocanth-db --features embedded --test embedded`).
//!
//! The directory is under cargo's temporary target directory, so the
//! binaries are extracted once per target directory, not once per run.
//!
//! Each test skips when the archive is not on the machine, unless
//! `BROCANTH_EMBEDDED_TEST=1` lets it download it (`mise run embedded`).
#![cfg(feature = "embedded")]

use brocanth_db::embedded::{archive_on_machine, EmbeddedConfig, EmbeddedPostgres, ARCHIVE_ENV};
use brocanth_db::{Db, DbConfig};

/// False, with a message, when the test cannot install the server offline
/// and may not download it.
fn can_run(test: &str) -> bool {
    if archive_on_machine() || std::env::var_os("BROCANTH_EMBEDDED_TEST").is_some() {
        return true;
    }
    eprintln!(
        "{test}: skipped, no Postgres archive on this machine\n\
         help: set BROCANTH_EMBEDDED_TEST=1 to download it once (`mise run embedded`), \
         or set {ARCHIVE_ENV} to the archive file"
    );
    false
}

#[test]
fn the_pool_opens_on_the_url_and_a_restart_keeps_the_rows() {
    if !can_run("the_pool_opens_on_the_url_and_a_restart_keeps_the_rows") {
        return;
    }
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("embedded-postgres");
    let config = EmbeddedConfig::new(&dir, "brocanth_test");
    let runtime = tokio::runtime::Runtime::new().expect("a runtime");

    // Started inside a runtime, as a plugin hook is.
    let server = runtime.block_on(async { EmbeddedPostgres::start(&config) });
    let server = server.expect("the server starts");
    assert!(!server.redacted_url().contains(&password(&dir)));
    runtime.block_on(async {
        let db = Db::connect(&DbConfig::new(server.url()))
            .await
            .expect("the pool connects");
        let mut tx = db.pool().begin().await.expect("a transaction");
        brocanth_db::sqlx::query("CREATE TABLE IF NOT EXISTS kept (n int)")
            .execute(&mut *tx)
            .await
            .expect("create");
        brocanth_db::sqlx::query("INSERT INTO kept VALUES (1)")
            .execute(&mut *tx)
            .await
            .expect("insert");
        tx.commit().await.expect("commit");
        db.close().await;
    });
    server.stop().expect("the server stops");
    assert!(!dir.join("data/postmaster.pid").exists());

    let server = EmbeddedPostgres::start(&config).expect("it starts again");
    let count: i64 = runtime.block_on(async {
        let db = Db::connect(&DbConfig::new(server.url()))
            .await
            .expect("the pool connects again");
        let (count,): (i64,) = brocanth_db::sqlx::query_as("SELECT count(*) FROM kept")
            .fetch_one(db.pool())
            .await
            .expect("count");
        db.close().await;
        count
    });
    assert!(count >= 1, "the row survived the restart");
    // A drop stops it too.
    drop(server);
    assert!(!dir.join("data/postmaster.pid").exists());
}

fn password(dir: &std::path::Path) -> String {
    std::fs::read_to_string(dir.join("password")).expect("the password file")
}

/// The port in a URL: `...@localhost:<port>/<database>`.
fn port(url: &str) -> u16 {
    let after = &url[url.rfind(':').expect("a port") + 1..];
    after[..after.find('/').expect("a database")]
        .parse()
        .expect("a number")
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name)
}

async fn select_one(url: &str) -> Db {
    let db = Db::connect(&DbConfig::new(url))
        .await
        .expect("the pool connects");
    let (one,): (i32,) = brocanth_db::sqlx::query_as("SELECT 1")
        .fetch_one(db.pool())
        .await
        .expect("a query");
    assert_eq!(one, 1);
    db
}

/// Tests that build an application each, in parallel, on one `dir`: one
/// server, started once, stopped by the last handle and not before.
#[test]
fn concurrent_starts_in_one_process_share_one_server() {
    if !can_run("concurrent_starts_in_one_process_share_one_server") {
        return;
    }
    const N: usize = 8;
    let dir = temp_dir("embedded-concurrent");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(N));
    let starts: Vec<_> = (0..N)
        .map(|i| {
            let (dir, barrier) = (dir.clone(), barrier.clone());
            std::thread::spawn(move || {
                // Two databases on one server: each start creates its own.
                let config = EmbeddedConfig::new(&dir, format!("app_{}", i % 2));
                barrier.wait();
                EmbeddedPostgres::start(&config).expect("each start succeeds")
            })
        })
        .collect();
    let mut servers: Vec<EmbeddedPostgres> = starts
        .into_iter()
        .map(|start| start.join().expect("no panic"))
        .collect();
    let first = port(servers[0].url());
    assert!(servers.iter().all(|s| port(s.url()) == first), "one port");

    let runtime = tokio::runtime::Runtime::new().expect("a runtime");
    let mut pools: Vec<Db> = runtime.block_on(async {
        let mut pools = Vec::new();
        for server in &servers {
            pools.push(select_one(server.url()).await);
        }
        pools
    });

    // Shutdown order, as the plugins run it: the pool closes, then the
    // handle is released. Every handle but the last leaves the server up.
    let last_pool = pools.pop().expect("a pool");
    let last_server = servers.pop().expect("a server");
    for (pool, server) in pools.into_iter().zip(servers) {
        runtime.block_on(pool.close());
        server.stop().expect("a release");
    }
    assert!(dir.join("data/postmaster.pid").exists(), "still running");
    runtime.block_on(async {
        brocanth_db::sqlx::query("SELECT 1")
            .execute(last_pool.pool())
            .await
            .expect("the last pool still queries");
        last_pool.close().await;
    });
    last_server.stop().expect("the last release stops it");
    assert!(!dir.join("data/postmaster.pid").exists(), "stopped");
}

/// What the child process does: `join` releases its handle as a test
/// would; `orphan` exits with the server running, as `kill -9` leaves it.
const CHILD_DIR: &str = "BROCANTH_EMBEDDED_CHILD_DIR";
const CHILD_MODE: &str = "BROCANTH_EMBEDDED_CHILD_MODE";

/// The child of the cross-process tests. It does nothing unless the parent
/// set [`CHILD_DIR`].
#[test]
fn child_process_helper() {
    let Some(dir) = std::env::var_os(CHILD_DIR) else {
        return;
    };
    let server = EmbeddedPostgres::start(&EmbeddedConfig::new(dir, "child"))
        .expect("the child starts or joins");
    let runtime = tokio::runtime::Runtime::new().expect("a runtime");
    runtime.block_on(async { select_one(server.url()).await.close().await });
    println!("embedded-port={}", port(server.url()));
    if std::env::var(CHILD_MODE).as_deref() == Ok("orphan") {
        // No destructor runs: the server stays up and the lock goes away.
        std::process::exit(0);
    }
    server.stop().expect("the child releases");
}

/// Runs [`child_process_helper`] in a new process; returns its port.
fn child(dir: &std::path::Path, mode: &str) -> u16 {
    let out = std::process::Command::new(std::env::current_exe().expect("this binary"))
        .args(["--exact", "child_process_helper", "--nocapture"])
        .env(CHILD_DIR, dir)
        .env(CHILD_MODE, mode)
        .output()
        .expect("the child runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "the child failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("embedded-port="))
        .expect("the child prints its port")
        .parse()
        .expect("a port")
}

/// A second process (a second terminal, a second test binary) joins the
/// running server and leaves it running when it releases.
#[test]
fn a_second_process_joins_the_running_server() {
    if !can_run("a_second_process_joins_the_running_server") {
        return;
    }
    let dir = temp_dir("embedded-two-processes");
    let server =
        EmbeddedPostgres::start(&EmbeddedConfig::new(&dir, "parent")).expect("the parent starts");
    assert_eq!(child(&dir, "join"), port(server.url()), "same server");
    assert!(dir.join("data/postmaster.pid").exists(), "still running");
    let runtime = tokio::runtime::Runtime::new().expect("a runtime");
    runtime.block_on(async { select_one(server.url()).await.close().await });
    server.stop().expect("the parent stops it");
    assert!(!dir.join("data/postmaster.pid").exists(), "stopped");
}

/// A process that died with the server running (`kill -9`) leaves an
/// orphan and no lock: the next start stops it and starts again.
#[test]
fn a_start_after_a_killed_process_recovers() {
    if !can_run("a_start_after_a_killed_process_recovers") {
        return;
    }
    let dir = temp_dir("embedded-orphan");
    child(&dir, "orphan");
    assert!(dir.join("data/postmaster.pid").exists(), "the orphan runs");
    let server =
        EmbeddedPostgres::start(&EmbeddedConfig::new(&dir, "parent")).expect("the start recovers");
    let runtime = tokio::runtime::Runtime::new().expect("a runtime");
    runtime.block_on(async { select_one(server.url()).await.close().await });
    server.stop().expect("it stops");
    assert!(!dir.join("data/postmaster.pid").exists(), "stopped");
}
