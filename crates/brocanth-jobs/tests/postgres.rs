//! What only Postgres has: the migration race, the move from the first
//! queue's tables, the transactional enqueue and `LISTEN/NOTIFY`.
//!
//! The behaviour suite (`queue.rs`, `recurring.rs`, `panel.rs`) already runs
//! on Postgres through `common::on_every_backend!`. The tests here have no
//! memory twin.
//!
//! Skipped at run time unless `BROCANTH_JOBS_TEST_POSTGRES` or
//! `BROCANTH_TEST_DATABASE_URL` names a database:
//!
//! ```sh
//! BROCANTH_JOBS_TEST_POSTGRES=postgres://localhost/brocanth_jobs_test \
//!   cargo test -p brocanth-jobs
//! ```
//!
//! Every skip prints a line naming the test, through a write that `cargo
//! test`'s output capture does not swallow. Each test works in a schema of
//! its own.

#![cfg(feature = "postgres")]

#[macro_use]
mod common;

use std::time::Duration;

use brocanth_jobs::{Queue, Worker};
use common::{log, token, wait_for, Echo};

macro_rules! queue_or_skip {
    () => {
        match common::postgres_queue().await {
            Some(queue) => queue,
            None => {
                common::skipped(common::NO_POSTGRES);
                return;
            }
        }
    };
}

macro_rules! pool_or_skip {
    () => {
        match common::postgres_pool().await {
            Some(pool) => pool,
            None => {
                common::skipped(common::NO_POSTGRES);
                return;
            }
        }
    };
}

// ---------------------------------------------------------------------------
// The schema
// ---------------------------------------------------------------------------

/// `CREATE TABLE IF NOT EXISTS` racing itself.
///
/// On Postgres the catalogue check and the create are separate steps, so
/// two sessions both decide to create and the loser fails with a
/// `pg_class_relname_nsp_index` violation. It has to race against an
/// **empty** schema, because once the tables are there `IF NOT EXISTS`
/// short-circuits and there is nothing left to race.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn migrate_is_safe_from_several_connections_at_once() {
    let pool = pool_or_skip!();

    // Eight at once, on one pool of six connections, so several really are
    // in flight together.
    let mut racers = Vec::new();
    for _ in 0..8 {
        let queue = Queue::from_pool(pool.clone());
        racers.push(tokio::spawn(async move { queue.migrate().await }));
    }
    let mut failures = Vec::new();
    for racer in racers {
        if let Err(e) = racer.await.expect("the task did not panic") {
            failures.push(e.to_string());
        }
    }
    assert!(
        failures.is_empty(),
        "concurrent migrate() must not fail: {failures:?}"
    );

    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT tablename::text FROM pg_tables \
         WHERE schemaname = current_schema() ORDER BY tablename",
    )
    .fetch_all(&pool)
    .await
    .expect("listing the tables");
    for table in [
        "brocanth_job_schedules",
        "queue_bulk",
        "queue_dead",
        "queue_outbox",
        "queue_scheduled",
    ] {
        assert!(
            tables.iter().any(|t| t == table),
            "{table} is missing from {tables:?}"
        );
    }
}

/// An application that ran the first queue has rows in `brocanth_jobs`
/// and `brocanth_dead_jobs`. `migrate` moves them into the engine's tables,
/// so nothing waiting or dead is lost in the upgrade.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn migrate_moves_the_first_queues_rows_into_the_engine() {
    let pool = pool_or_skip!();
    let t = token("legacy");

    create_old_tables(&pool).await;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let payload = format!(r#"{{"token":"{t}","n":1}}"#);
    // One waiting job, and one a worker of the old version was running.
    for state in ["pending", "running"] {
        sqlx::query(
            "INSERT INTO brocanth_jobs (queue, job_name, payload, state, attempts, \
             max_attempts, run_at, created_at) VALUES ('default', 'echo', $1, $2, $3, 5, $4, $4)",
        )
        .bind(&payload)
        .bind(state)
        .bind(i64::from(state == "running"))
        .bind(now_ms)
        .execute(&pool)
        .await
        .expect("an old job");
    }
    sqlx::query(
        "INSERT INTO brocanth_dead_jobs (job_id, queue, job_name, payload, priority, attempts, \
         max_attempts, error, failure_kind, backtrace, enqueued_at, failed_at) \
         VALUES (9, 'default', 'echo', $1, 0, 5, 5, 'smtp refused', 'panicked', 'frame 1', $2, $2)",
    )
    .bind(&payload)
    .bind(now_ms)
    .execute(&pool)
    .await
    .expect("an old dead job");

    let queue = Queue::from_pool(pool.clone());
    queue.migrate().await.expect("migrate");

    let stats = queue.stats().await.expect("stats");
    assert_eq!(stats.ready, 2, "both jobs are waiting again: {stats:?}");
    assert_eq!(stats.dead, 1);
    let dead = queue.dead_jobs(10, 0).await.expect("dead");
    assert_eq!(dead[0].kind, "echo");
    assert!(
        dead[0].failure.contains("smtp refused"),
        "{}",
        dead[0].failure
    );
    assert!(dead[0].failure.contains("frame 1"), "the backtrace travels");

    // The old tables are left, empty, and a second migrate moves nothing twice.
    queue.migrate().await.expect("migrate again");
    assert_eq!(queue.stats().await.expect("stats").ready, 2);
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM brocanth_jobs")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(left, 0);

    // And the moved jobs run.
    let worker = Worker::builder(&queue)
        .poll_interval(Duration::from_millis(10))
        .register::<Echo>()
        .start()
        .expect("start");
    wait_for(
        "both moved jobs to run",
        Duration::from_secs(10),
        || async { log(&t).len() == 2 },
    )
    .await;
    worker.shutdown(Duration::from_secs(5)).await.expect("stop");
}

/// The first queue's two tables, as its `migrate` created them.
async fn create_old_tables(pool: &sqlx::PgPool) {
    for sql in [OLD_JOBS, OLD_DEAD] {
        sqlx::query(sql).execute(pool).await.expect("an old table");
    }
}

const OLD_JOBS: &str = "CREATE TABLE brocanth_jobs (
    id BIGSERIAL PRIMARY KEY, queue TEXT NOT NULL, job_name TEXT NOT NULL,
    payload TEXT NOT NULL, priority BIGINT NOT NULL DEFAULT 0,
    state TEXT NOT NULL DEFAULT 'pending', attempts BIGINT NOT NULL DEFAULT 0,
    max_attempts BIGINT NOT NULL, run_at BIGINT NOT NULL, created_at BIGINT NOT NULL,
    unique_key TEXT, locked_by TEXT, locked_at BIGINT, heartbeat_at BIGINT,
    last_error TEXT)";

const OLD_DEAD: &str = "CREATE TABLE brocanth_dead_jobs (
    id BIGSERIAL PRIMARY KEY, job_id BIGINT NOT NULL, queue TEXT NOT NULL,
    job_name TEXT NOT NULL, payload TEXT NOT NULL, priority BIGINT NOT NULL,
    attempts BIGINT NOT NULL, max_attempts BIGINT NOT NULL, unique_key TEXT,
    error TEXT NOT NULL, failure_kind TEXT NOT NULL, backtrace TEXT,
    enqueued_at BIGINT NOT NULL, failed_at BIGINT NOT NULL)";

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// One old job row.
async fn old_job(
    pool: &sqlx::PgPool,
    queue: &str,
    payload: &str,
    run_at: i64,
    unique: Option<&str>,
) {
    sqlx::query(
        "INSERT INTO brocanth_jobs (queue, job_name, payload, attempts, max_attempts, \
         run_at, created_at, unique_key, last_error) \
         VALUES ($1, 'echo', $2, 2, 7, $3, $4, $5, 'flaky smtp')",
    )
    .bind(queue)
    .bind(payload)
    .bind(run_at)
    .bind(now_ms())
    .bind(unique)
    .execute(pool)
    .await
    .expect("an old job");
}

/// `migrate` on a schema that is already current changes nothing: the
/// jobs, the dead letter and the schedules are all still there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn migrate_twice_on_a_current_schema_is_a_no_op() {
    let queue = queue_or_skip!();
    let t = token("migrate-twice");

    let id = queue
        .enqueue(Echo {
            token: t.clone(),
            n: 1,
        })
        .await
        .expect("enqueue");
    queue
        .recurring("kept", "0 * * * *", common::Nightly { token: t.clone() })
        .install()
        .await
        .expect("install");
    let before = queue.stats().await.expect("stats");

    queue.migrate().await.expect("migrate again");
    queue.migrate().await.expect("and again");

    let after = queue.stats().await.expect("stats");
    assert_eq!(
        (after.ready, after.scheduled, after.running, after.dead),
        (before.ready, before.scheduled, before.running, before.dead),
    );
    assert_eq!(after.ready, 1);
    assert!(queue
        .inspect(id.id().expect("an id"))
        .await
        .expect("inspect")
        .is_some());
    // The schedule survived: removing it finds it.
    assert!(queue.remove_recurring("kept").await.expect("remove"));
}

/// An old row keeps its queue, its time, its attempts, its error and its
/// unique key. A future `run_at` stays in the future.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn migrate_keeps_what_each_old_row_said() {
    let pool = pool_or_skip!();
    let t = token("legacy-fields");
    create_old_tables(&pool).await;

    let payload = format!(r#"{{"token":"{t}","n":1}}"#);
    let in_an_hour = now_ms() + 3_600_000;
    old_job(&pool, "mail", &payload, in_an_hour, Some("invoice:7")).await;

    let queue = Queue::from_pool(pool.clone());
    queue.migrate().await.expect("migrate");

    let stats = queue.stats().await.expect("stats");
    assert_eq!(stats.ready, 0, "a future job is not ready: {stats:?}");
    // It ran twice before: it lists as a retry, with its old error.
    let waiting = queue.retries(10, 0).await.expect("retries");
    assert_eq!(waiting.len(), 1, "{waiting:?}");
    let row = &waiting[0];
    assert_eq!(row.kind, "echo");
    assert_eq!(row.queue, "mail");
    assert_eq!(row.attempt, 2);
    assert_eq!(row.max_attempts, 7);
    assert_eq!(row.last_error.as_deref(), Some("flaky smtp"));
    let due = row
        .available_at
        .duration_since(std::time::SystemTime::now())
        .expect("still in the future");
    assert!(due > Duration::from_secs(3_000), "{due:?}");

    // The unique key came along: the same key folds into the moved row.
    let again = queue
        .enqueue(Echo {
            token: t.clone(),
            n: 2,
        })
        .queue("mail")
        .unique("invoice:7")
        .await
        .expect("enqueue");
    assert!(again.is_duplicate(), "the old row still holds its key");
}

/// During a rolling deploy an old worker keeps writing to the old table.
/// The next boot's `migrate` moves those rows too, and a row whose unique
/// key the engine already holds is folded, not doubled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_later_migrate_moves_what_an_old_worker_wrote_and_folds_a_held_key() {
    let queue = queue_or_skip!();
    let pool = queue_pool(&queue).await;
    let t = token("legacy-later");

    // The new version already runs, and holds `report:1`.
    queue
        .enqueue(Echo {
            token: t.clone(),
            n: 1,
        })
        .unique("report:1")
        .await
        .expect("enqueue");

    // An old worker, still up, writes two jobs to its own table.
    create_old_tables(&pool).await;
    let payload = format!(r#"{{"token":"{t}","n":9}}"#);
    old_job(&pool, "default", &payload, now_ms(), Some("report:1")).await;
    old_job(&pool, "default", &payload, now_ms(), None).await;

    queue.migrate().await.expect("migrate");

    assert_eq!(
        queue.stats().await.expect("stats").ready,
        2,
        "the job with the held key folded into the one already queued"
    );
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM brocanth_jobs")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(left, 0, "the old table is empty");
}

/// Only the old dead letter is left (somebody dropped `brocanth_jobs`).
/// It still moves, and a dead row with no backtrace says so by saying
/// nothing about one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn migrate_moves_an_old_dead_letter_on_its_own() {
    let pool = pool_or_skip!();
    let t = token("legacy-dead");
    sqlx::query(OLD_DEAD)
        .execute(&pool)
        .await
        .expect("the old dead table");
    sqlx::query(
        "INSERT INTO brocanth_dead_jobs (job_id, queue, job_name, payload, priority, attempts, \
         max_attempts, error, failure_kind, enqueued_at, failed_at) \
         VALUES (3, 'mail', 'echo', $1, 0, 1, 1, 'still running after 30s', 'timed out', $2, $2)",
    )
    .bind(format!(r#"{{"token":"{t}","n":1}}"#))
    .bind(now_ms())
    .execute(&pool)
    .await
    .expect("an old dead job");

    let queue = Queue::from_pool(pool.clone());
    queue.migrate().await.expect("migrate");
    queue
        .migrate()
        .await
        .expect("migrate again moves nothing twice");

    let dead = queue.dead_jobs(10, 0).await.expect("dead");
    assert_eq!(dead.len(), 1);
    assert_eq!(dead[0].queue, "mail");
    assert_eq!(dead[0].failure, "timed out: still running after 30s");

    // And it can be put back, on its own queue, and run.
    let requeued = queue
        .requeue_dead(&dead[0].id)
        .await
        .expect("requeue")
        .expect("a new row");
    let row = queue.job(&requeued).await.expect("job").expect("the row");
    assert_eq!(row.queue, "mail");
    let worker = Worker::builder(&queue)
        .queues(["mail"])
        .poll_interval(Duration::from_millis(10))
        .register::<Echo>()
        .start()
        .expect("start");
    wait_for(
        "the requeued job to run",
        Duration::from_secs(10),
        || async { log(&t) == vec!["ran 1"] },
    )
    .await;
    worker.shutdown(Duration::from_secs(5)).await.expect("stop");
}

// ---------------------------------------------------------------------------
// The transactional enqueue
// ---------------------------------------------------------------------------

/// A job enqueued in a transaction commits with it, and a worker's relay
/// delivers it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_job_enqueued_in_a_committed_transaction_runs() {
    let queue = queue_or_skip!();
    let pool = queue_pool(&queue).await;
    let t = token("in-tx");

    let mut tx = pool.begin().await.expect("begin");
    queue
        .enqueue(Echo {
            token: t.clone(),
            n: 1,
        })
        .in_tx(&mut tx)
        .await
        .expect("enqueue in the transaction");
    assert_eq!(
        queue.stats().await.expect("stats").ready,
        0,
        "nothing is visible before the commit"
    );
    tx.commit().await.expect("commit");

    let worker = Worker::builder(&queue)
        .poll_interval(Duration::from_millis(10))
        .register::<Echo>()
        .start()
        .expect("start");
    wait_for(
        "the relayed job to run",
        Duration::from_secs(10),
        || async { log(&t) == ["ran 1"] },
    )
    .await;
    worker.shutdown(Duration::from_secs(5)).await.expect("stop");
}

/// A rollback takes the job with it: no relay ever sees it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rolled_back_transaction_takes_its_job_with_it() {
    let queue = queue_or_skip!();
    let pool = queue_pool(&queue).await;
    let t = token("in-tx-rollback");

    let mut tx = pool.begin().await.expect("begin");
    queue
        .enqueue(Echo {
            token: t.clone(),
            n: 1,
        })
        .in_tx(&mut tx)
        .await
        .expect("enqueue in the transaction");
    tx.rollback().await.expect("rollback");

    let worker = Worker::builder(&queue)
        .poll_interval(Duration::from_millis(10))
        .register::<Echo>()
        .start()
        .expect("start");
    tokio::time::sleep(Duration::from_millis(800)).await;
    worker.shutdown(Duration::from_secs(5)).await.expect("stop");

    assert!(log(&t).is_empty(), "the job ran: {:?}", log(&t));
    let outbox: i64 = sqlx::query_scalar("SELECT count(*) FROM queue_outbox")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(outbox, 0);
    assert_eq!(queue.stats().await.expect("stats").total(), 0);
}

/// The pool a test queue runs on, for a transaction in the same schema.
async fn queue_pool(queue: &Queue) -> sqlx::PgPool {
    queue.pg_pool().expect("a Postgres queue").clone()
}

// ---------------------------------------------------------------------------
// Being told, rather than asking
// ---------------------------------------------------------------------------

/// The claim about `LISTEN`/`NOTIFY`, stated so it cannot pass by accident.
///
/// Both polls are set to a minute. A worker that polls cannot start this job
/// inside the assertion window, so the only way the test passes is if the
/// enqueue woke it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_enqueue_wakes_a_worker_that_would_otherwise_be_asleep() {
    let queue = queue_or_skip!();
    let token = token("notify-wakes");

    let worker = Worker::builder(&queue)
        .concurrency(1)
        // A minute each. Neither is allowed to be what makes this work.
        .poll_interval(Duration::from_secs(60))
        .safety_poll(Duration::from_secs(60))
        .register::<Echo>()
        .start()
        .expect("the worker started");

    // Let it reach the idle branch and the listener attach before the row
    // lands, which is the case the notification exists for.
    tokio::time::sleep(Duration::from_millis(400)).await;

    let started = std::time::Instant::now();
    queue
        .enqueue(Echo {
            token: token.clone(),
            n: 1,
        })
        .await
        .expect("enqueued");

    wait_for("the job to run", Duration::from_secs(5), || async {
        !log(&token).is_empty()
    })
    .await;

    let waited = started.elapsed();
    assert_eq!(log(&token), ["ran 1"]);
    assert!(
        waited < Duration::from_secs(5),
        "woken by the enqueue, not by a poll: waited {waited:?}"
    );

    worker
        .shutdown(Duration::from_secs(5))
        .await
        .expect("the worker stopped");
}

/// With the listener off, the poll is the mechanism again.
///
/// The fallback has to work, because it is what a Postgres deployment runs
/// when its connection limit cannot spare a session per worker.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_worker_told_not_to_listen_still_finds_the_work() {
    let queue = queue_or_skip!();
    let token = token("no-listen");

    let worker = Worker::builder(&queue)
        .concurrency(1)
        .listen(false)
        .poll_interval(Duration::from_millis(100))
        .register::<Echo>()
        .start()
        .expect("the worker started");

    queue
        .enqueue(Echo {
            token: token.clone(),
            n: 7,
        })
        .await
        .expect("enqueued");

    wait_for("the polled job to run", Duration::from_secs(10), || async {
        !log(&token).is_empty()
    })
    .await;
    assert_eq!(log(&token), ["ran 7"]);

    worker
        .shutdown(Duration::from_secs(5))
        .await
        .expect("the worker stopped");
}
