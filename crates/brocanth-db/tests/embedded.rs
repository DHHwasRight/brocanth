//! The embedded server, started for real: under the feature `embedded`
//! only (`cargo test -p brocanth-db --features embedded --test embedded`).
//!
//! The directory is under cargo's temporary target directory, so the
//! binaries are extracted once per target directory, not once per run.
//!
//! Each test skips when the archive is not on the machine, unless
//! `BROCANTH_EMBEDDED_TEST=1` lets it download it (`mise run embedded`).
#![cfg(feature = "embedded")]

use brocanth_db::embedded::{
    archive_on_machine, backup, bundled, fetch_archive, restore, EmbeddedConfig, EmbeddedError,
    EmbeddedPostgres, Mode, ARCHIVE_ENV, POSTGRES_VERSION,
};
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
    let mode = std::env::var(CHILD_MODE).unwrap_or_default();
    if mode.starts_with("prod") {
        return production_child(std::path::Path::new(&dir), &mode);
    }
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

// Production mode. Each test runs in a directory of its own. "Fresh"
// removes everything but `server/`, so the binaries are extracted once per
// directory; the offline test removes `server/` too.

/// A production config on `dir`, installed from the archive file, connected
/// as the application role `app`.
fn production(dir: &std::path::Path, archive: Option<std::path::PathBuf>) -> EmbeddedConfig {
    let mut config = EmbeddedConfig::new(dir, "app");
    config.mode = Mode::Production;
    config.archive = archive;
    config.app_role = Some("app".to_string());
    config
}

/// The archive file, downloaded once when the machine has none.
fn archive() -> std::path::PathBuf {
    fetch_archive().unwrap_or_else(|e| panic!("{e}"))
}

/// `dir` with everything but `server/` removed.
fn fresh(name: &str) -> std::path::PathBuf {
    let dir = temp_dir(name);
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if entry.file_name() != "server" {
                let path = entry.path();
                let _ = std::fs::remove_dir_all(&path).or_else(|_| std::fs::remove_file(&path));
            }
        }
    }
    std::fs::create_dir_all(&dir).expect("the directory");
    dir
}

async fn count(db: &Db) -> i64 {
    let (n,): (i64,) = brocanth_db::sqlx::query_as("SELECT count(*) FROM kept")
        .fetch_one(db.pool())
        .await
        .expect("count");
    n
}

async fn insert_rows(url: &str, n: i32) -> Db {
    let db = select_one(url).await;
    brocanth_db::sqlx::query("CREATE TABLE IF NOT EXISTS kept (n int)")
        .execute(db.pool())
        .await
        .expect("create");
    brocanth_db::sqlx::query("INSERT INTO kept SELECT generate_series(1, $1)")
        .bind(n)
        .execute(db.pool())
        .await
        .expect("insert");
    db
}

/// A fresh directory, no `server/`, the archive file only: the start needs
/// no network and no cache. The pool connects as a role that is not the
/// superuser and owns the database; the server listens on localhost; the
/// secrets are private files.
#[test]
fn production_starts_offline_from_an_archive_file_as_the_application_role() {
    if !can_run("production_starts_offline_from_an_archive_file_as_the_application_role") {
        return;
    }
    let dir = temp_dir("prod-offline");
    let _ = std::fs::remove_dir_all(&dir);
    let server = EmbeddedPostgres::start(&production(&dir, Some(archive())))
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        server.url().starts_with("postgresql://app:"),
        "{}",
        server.redacted_url()
    );
    let runtime = tokio::runtime::Runtime::new().expect("a runtime");
    runtime.block_on(async {
        let db = insert_rows(server.url(), 3).await;
        let (superuser, bypass, owner, listen): (bool, bool, String, String) =
            brocanth_db::sqlx::query_as(
                "SELECT r.rolsuper, r.rolbypassrls, \
                 pg_get_userbyid(d.datdba)::text, current_setting('listen_addresses') \
                 FROM pg_roles r, pg_database d \
                 WHERE r.rolname = current_user AND d.datname = current_database()",
            )
            .fetch_one(db.pool())
            .await
            .expect("the role");
        assert!(
            !superuser && !bypass,
            "the application role is not privileged"
        );
        assert_eq!(owner, "app", "the role owns the database");
        assert_eq!(listen, "localhost");
        db.close().await;
    });
    #[cfg(unix)]
    for file in ["password", "app_password"] {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join(file))
            .expect(file)
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "{file}: {mode:o}");
    }
    let hba = std::fs::read_to_string(dir.join("data/pg_hba.conf")).expect("pg_hba.conf");
    assert!(
        hba.contains("scram-sha-256")
            && !hba
                .lines()
                .any(|l| !l.starts_with('#') && l.trim_end().ends_with(" password")),
        "{hba}"
    );
    server.stop().expect("it stops");

    // Installed now: a start with no archive at all reads none.
    let server = EmbeddedPostgres::start(&production(&dir, None)).unwrap_or_else(|e| panic!("{e}"));
    server.stop().expect("it stops");
}

/// With no archive bundled, named or installed, production refuses to
/// start rather than download, and says how to give it one. With the
/// archive bundled (`BROCANTH_POSTGRES_BUNDLE` at build time), the same
/// start succeeds from the bytes in the binary.
#[test]
fn production_never_downloads() {
    if std::env::var_os(ARCHIVE_ENV).is_some() {
        eprintln!("production_never_downloads: skipped, {ARCHIVE_ENV} is set");
        return;
    }
    let dir = temp_dir("prod-no-archive");
    let _ = std::fs::remove_dir_all(&dir);
    let started = EmbeddedPostgres::start(&production(&dir, None));
    if bundled() {
        let server = started.unwrap_or_else(|e| panic!("bundled: {e}"));
        server.stop().expect("it stops");
        return;
    }
    let text = started.expect_err("no source").to_string();
    assert!(text.contains("never downloads"), "{text}");
    assert!(
        text.contains("help:") && text.contains("--db_embedded.archive="),
        "{text}"
    );
    assert!(
        !dir.join("server").join(POSTGRES_VERSION).exists(),
        "nothing installed"
    );
}

/// An archive that does not match the pin is refused before extraction.
#[test]
fn production_refuses_a_tampered_archive() {
    let dir = temp_dir("prod-tampered");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the directory");
    let tampered = dir.join("tampered.tar.gz");
    let mut bytes = match brocanth_db::embedded::local_archive() {
        Some(file) => std::fs::read(file).expect("the archive"),
        None => b"not the release archive".to_vec(),
    };
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    std::fs::write(&tampered, &bytes).expect("written");
    let refused = EmbeddedPostgres::start(&production(&dir, Some(tampered)));
    if bundled() {
        // The bundled archive comes first, and it matches.
        refused.expect("bundled").stop().expect("it stops");
        return;
    }
    let text = refused.expect_err("tampered").to_string();
    assert!(
        text.contains("SHA-256") && text.contains("nothing was extracted"),
        "{text}"
    );
    assert!(
        !dir.join("server").join(POSTGRES_VERSION).exists(),
        "nothing installed"
    );
}

/// A data directory of another major version is refused at start, with the
/// steps, before anything is installed or started.
#[test]
fn production_refuses_a_cluster_of_another_major_version() {
    let dir = fresh("prod-major");
    std::fs::create_dir_all(dir.join("data")).expect("data");
    std::fs::write(dir.join("data/PG_VERSION"), "17\n").expect("written");
    std::fs::write(dir.join("data/postgresql.conf"), "").expect("written");
    let refused = EmbeddedPostgres::start(&production(&dir, None)).expect_err("17");
    assert!(matches!(refused, EmbeddedError::Config { .. }), "{refused}");
    let text = refused.to_string();
    assert!(
        text.contains("Postgres 17 cluster") && text.contains("help:"),
        "{text}"
    );
}

/// One process owns a production server: a second start in the same
/// process, and a second process, are refused with a help line.
#[test]
fn production_refuses_a_second_owner() {
    if !can_run("production_refuses_a_second_owner") {
        return;
    }
    let dir = fresh("prod-owner");
    let config = production(&dir, Some(archive()));
    let server = EmbeddedPostgres::start(&config).unwrap_or_else(|e| panic!("{e}"));
    let again = EmbeddedPostgres::start(&config).expect_err("a second start");
    assert!(again.to_string().contains("one process owns it"), "{again}");
    let out = child_output(&dir, "prod-second");
    assert!(out.contains("refused=") && out.contains("help:"), "{out}");
    let runtime = tokio::runtime::Runtime::new().expect("a runtime");
    runtime.block_on(async { select_one(server.url()).await.close().await });
    server.stop().expect("it stops");
}

/// What a production child does: `prod-second` tries to start and prints
/// the refusal; `prod-kill` starts, writes rows and waits to be killed.
fn production_child(dir: &std::path::Path, mode: &str) {
    let archive = std::env::var_os("BROCANTH_EMBEDDED_CHILD_ARCHIVE").map(Into::into);
    let config = production(dir, archive);
    match mode {
        "prod-second" => match EmbeddedPostgres::start(&config) {
            Ok(_) => println!("started"),
            Err(e) => println!("refused={}", e.to_string().replace('\n', " ")),
        },
        "prod-kill" => {
            let server = EmbeddedPostgres::start(&config).expect("the child starts");
            let runtime = tokio::runtime::Runtime::new().expect("a runtime");
            runtime.block_on(async {
                let db = insert_rows(server.url(), 5).await;
                db.close().await;
            });
            println!("ready");
            use std::io::Write;
            std::io::stdout().flush().expect("flushed");
            loop {
                std::thread::sleep(std::time::Duration::from_secs(60));
            }
        }
        _ => panic!("unknown mode {mode}"),
    }
}

fn child_command(dir: &std::path::Path, mode: &str) -> std::process::Command {
    let mut command = std::process::Command::new(std::env::current_exe().expect("this binary"));
    command
        .args(["--exact", "child_process_helper", "--nocapture"])
        .env(CHILD_DIR, dir)
        .env(CHILD_MODE, mode)
        .env("BROCANTH_EMBEDDED_CHILD_ARCHIVE", archive());
    command
}

fn child_output(dir: &std::path::Path, mode: &str) -> String {
    let out = child_command(dir, mode).output().expect("the child runs");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// SIGKILL of the owning process, then of the postmaster itself: the next
/// start recovers (orphan logic, then Postgres's WAL replay) and the
/// committed rows are there.
#[cfg(unix)]
#[test]
fn production_survives_sigkill_with_rows_intact() {
    use std::io::BufRead;
    if !can_run("production_survives_sigkill_with_rows_intact") {
        return;
    }
    let dir = fresh("prod-kill");
    let mut child = child_command(&dir, "prod-kill")
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("the child runs");
    let stdout = child.stdout.take().expect("stdout");
    let ready = std::io::BufReader::new(stdout)
        .lines()
        .map_while(Result::ok)
        .any(|line| line == "ready");
    assert!(ready, "the child wrote its rows");
    child.kill().expect("SIGKILL");
    child.wait().expect("reaped");
    let pid = std::fs::read_to_string(dir.join("data/postmaster.pid")).expect("the orphan runs");
    let pid = pid.lines().next().expect("a pid").trim().to_string();
    let killed = std::process::Command::new("kill")
        .args(["-KILL", &pid])
        .status()
        .expect("kill");
    assert!(killed.success(), "the postmaster is killed");
    std::thread::sleep(std::time::Duration::from_millis(500));

    let server = EmbeddedPostgres::start(&production(&dir, Some(archive())))
        .unwrap_or_else(|e| panic!("the start recovers: {e}"));
    let runtime = tokio::runtime::Runtime::new().expect("a runtime");
    let n = runtime.block_on(async {
        let db = select_one(server.url()).await;
        let n = count(&db).await;
        db.close().await;
        n
    });
    assert_eq!(n, 5, "the committed rows survived");
    server.stop().expect("it stops");
}

/// A backup taken while the server runs, rows deleted, the server stopped,
/// the restore run: the rows are back, owned by the application role.
#[test]
fn production_backup_and_restore_round_trip() {
    if !can_run("production_backup_and_restore_round_trip") {
        return;
    }
    let dir = fresh("prod-backup");
    let config = production(&dir, Some(archive()));
    let file = dir.join("app.dump");
    let server = EmbeddedPostgres::start(&config).unwrap_or_else(|e| panic!("{e}"));
    let runtime = tokio::runtime::Runtime::new().expect("a runtime");
    let db = runtime.block_on(insert_rows(server.url(), 4));
    backup(&config, &file).unwrap_or_else(|e| panic!("the backup: {e}"));
    assert!(file.is_file() && !file.with_extension("partial").exists());
    runtime.block_on(async {
        brocanth_db::sqlx::query("DELETE FROM kept")
            .execute(db.pool())
            .await
            .expect("delete");
        assert_eq!(count(&db).await, 0);
    });
    let refused = restore(&config, &file).expect_err("the server runs");
    assert!(
        refused.to_string().contains("stop the application"),
        "{refused}"
    );
    runtime.block_on(db.close());
    server.stop().expect("it stops");

    restore(&config, &file).unwrap_or_else(|e| panic!("the restore: {e}"));
    let server = EmbeddedPostgres::start(&config).unwrap_or_else(|e| panic!("{e}"));
    runtime.block_on(async {
        let db = select_one(server.url()).await;
        assert_eq!(count(&db).await, 4, "the rows are back");
        let (owner,): (String,) = brocanth_db::sqlx::query_as(
            "SELECT tableowner::text FROM pg_tables WHERE tablename = 'kept'",
        )
        .fetch_one(db.pool())
        .await
        .expect("the owner");
        assert_eq!(owner, "app");
        db.close().await;
    });
    server.stop().expect("it stops");
    // With the server stopped, the backup starts it for the dump.
    backup(&config, &dir.join("again.dump")).unwrap_or_else(|e| panic!("{e}"));
    assert!(!dir.join("data/postmaster.pid").exists(), "stopped again");
}
