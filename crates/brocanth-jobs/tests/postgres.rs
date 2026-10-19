//! The same guarantees, on Postgres, where the claim really does use
//! `FOR UPDATE SKIP LOCKED`.
//!
//! Compiled with the `postgres` feature, which is the default, and skipped at
//! run time unless `BROCANTH_JOBS_TEST_POSTGRES` or
//! `BROCANTH_TEST_DATABASE_URL` names a database:
//!
//! ```sh
//! BROCANTH_JOBS_TEST_POSTGRES=postgres://localhost/brocanth_jobs_test \
//!   cargo test -p brocanth-jobs --test postgres
//! ```
//!
//! Skipping rather than failing, because `cargo test -p brocanth-jobs` has to
//! pass on a laptop with no server running — and because the suite in
//! `queue.rs` already proves the behaviour on a real database. What these add
//! is that the Postgres *dialect* of every statement is valid and that the
//! locking claim behaves the same as the serialised one.
//!
//! Every skip prints a line naming the test, through a write that `cargo
//! test`'s output capture does not swallow. Postgres is the default backend
//! now, so a run where these quietly did nothing would be a run that tested
//! the default path not at all, and it has to be visible that that is what
//! happened.

#![cfg(feature = "postgres")]

mod common;

use std::time::Duration;

use brocanth_jobs::{Enqueued, Queue, Worker};
use common::{log, token, wait_for, AlwaysFails, Echo, Nightly};

/// The queue under test, or `None` when no database was named.
///
/// `migrate` runs once for the whole process, not once per test. Running it
/// per test is what the suite used to do, and it deadlocks once there are
/// enough tests: `CREATE INDEX IF NOT EXISTS` takes a `ShareLock` on the
/// table, a concurrent `reclaim_orphans` holds `RowExclusiveLock` on it and
/// wants the dead-letter table, and the two cross. The schema is the same
/// for every test here, so creating it once is also what an application
/// does.
async fn postgres_queue() -> Option<Queue> {
    use tokio::sync::OnceCell;
    static MIGRATED: OnceCell<()> = OnceCell::const_new();

    let url = common::postgres_url()?;
    let queue = Queue::connect(&url).await.expect("connecting to Postgres");
    MIGRATED
        .get_or_init(|| async {
            queue.migrate().await.expect("creating the tables");
        })
        .await;
    // One database serves every test in this file, and they run in parallel,
    // so nothing here truncates anything. Each test works inside its own queue
    // name and its own token instead, which is what two applications sharing a
    // database would do.
    //
    // One `cargo test` process at a time, though. `tick_schedules`,
    // `reclaim_orphans` and `stats` are queue-wide by design — they are the
    // operations an operator runs over the whole queue — so a second process
    // on the same database fires the first one's schedules and reclaims its
    // jobs. Two concurrent suites need two databases.
    Some(queue)
}

macro_rules! queue_or_skip {
    () => {
        match postgres_queue().await {
            Some(queue) => queue,
            None => {
                common::skipped(
                    "no Postgres server; set BROCANTH_JOBS_TEST_POSTGRES or \
                     BROCANTH_TEST_DATABASE_URL to run this",
                );
                return;
            }
        }
    };
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_statement_is_valid_postgres() {
    let queue = queue_or_skip!();
    let t = token("pg-smoke");

    let id = queue
        .enqueue(Echo {
            token: t.clone(),
            n: 1,
        })
        .unique(format!("pg-smoke:{t}"))
        .await
        .expect("enqueue");
    assert!(matches!(id, Enqueued::Inserted(_)));

    queue.stats().await.expect("stats");
    queue.inspect(id.id()).await.expect("inspect");
    queue.dead_jobs(10, 0).await.expect("dead jobs");
    // The three operator reads. Here and not only in `queue.rs`, because
    // that suite runs on SQLite and this test's whole job is the dialect.
    queue.retries(10, 0).await.expect("retries");
    queue.scheduled(10, 0).await.expect("scheduled");
    queue.busy(10, 0).await.expect("busy");
    queue
        .reclaim_orphans(Duration::from_secs(60))
        .await
        .expect("reclaim");
    queue.tick_schedules().await.expect("tick");

    let worker = Worker::builder(&queue)
        .queues(["default"])
        .poll_interval(Duration::from_millis(10))
        .register::<Echo>()
        .start()
        .expect("start");
    wait_for("the job to run", Duration::from_secs(10), || async {
        log(&t) == vec!["ran 1"]
    })
    .await;
    worker
        .shutdown(Duration::from_secs(5))
        .await
        .expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn skip_locked_keeps_two_workers_off_the_same_row() {
    let queue = queue_or_skip!();
    let t = token("pg-skip-locked");
    const JOBS: u32 = 60;

    for n in 0..JOBS {
        queue
            .enqueue(Echo {
                token: t.clone(),
                n,
            })
            .queue(&t)
            .await
            .expect("enqueue");
    }

    let mut workers = Vec::new();
    for i in 0..4 {
        workers.push(
            Worker::builder(&queue)
                .id(format!("pg-{i}"))
                .queues([t.clone()])
                .concurrency(4)
                .poll_interval(Duration::from_millis(5))
                .register::<Echo>()
                .start()
                .expect("start"),
        );
    }

    wait_for("every job to run", Duration::from_secs(30), || async {
        log(&t).len() >= JOBS as usize
    })
    .await;
    for worker in workers {
        worker
            .shutdown(Duration::from_secs(5))
            .await
            .expect("shutdown");
    }

    let ran = log(&t);
    let unique: std::collections::HashSet<&String> = ran.iter().collect();
    assert_eq!(unique.len(), JOBS as usize);
    assert_eq!(ran.len(), JOBS as usize, "a row was claimed twice");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_dead_letter_works_on_postgres_too() {
    let queue = queue_or_skip!();
    let t = token("pg-dead");

    queue
        .enqueue(AlwaysFails { token: t.clone() })
        .queue(&t)
        .await
        .expect("enqueue");

    let worker = Worker::builder(&queue)
        .queues([t.clone()])
        .poll_interval(Duration::from_millis(10))
        .register::<AlwaysFails>()
        .start()
        .expect("start");
    wait_for("three attempts", Duration::from_secs(20), || async {
        log(&t).len() >= 3
    })
    .await;
    wait_for("the dead letter", Duration::from_secs(10), || async {
        queue
            .dead_jobs(50, 0)
            .await
            .expect("dead")
            .iter()
            .any(|d| d.payload.contains(&t))
    })
    .await;
    worker
        .shutdown(Duration::from_secs(5))
        .await
        .expect("shutdown");

    let dead = queue.dead_jobs(50, 0).await.expect("dead");
    let mine = dead.iter().find(|d| d.payload.contains(&t)).expect("mine");
    assert_eq!(mine.attempts, 3);
    assert_eq!(mine.failure_kind, "returned");
    queue.discard_dead(mine.id).await.expect("discard");
}

/// Recurring schedules on Postgres.
///
/// `tests/recurring.rs` runs this path on SQLite only, and it is a different
/// set of statements from the queue itself — an upsert on a unique name, a due
/// lookup and a fired-marker write. A `ManualClock` moves the window, so the
/// test takes no wall-clock time.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_recurring_schedule_fires_once_per_window_on_postgres() {
    use brocanth_jobs::ManualClock;
    use chrono::{TimeZone, Utc};

    let queue = queue_or_skip!();
    let t = token("pg-recurring");
    let noon = Utc.with_ymd_and_hms(2026, 5, 6, 12, 0, 0).unwrap();
    let clock = ManualClock::new(noon);
    let queue = queue.with_clock(clock.clone());

    queue
        .recurring(&t, "0 * * * *", Nightly { token: t.clone() })
        .queue(&t)
        .install()
        .await
        .expect("install");

    // The counts are read per queue, not from `tick_schedules`'s return value.
    // That return value is queue-wide, and this test shares a database with
    // the others and with whatever a previous run left behind.
    async fn ready(queue: &Queue, name: &str) -> u64 {
        queue
            .stats()
            .await
            .expect("stats")
            .queues
            .iter()
            .find(|q| q.queue == name)
            .map_or(0, |q| q.ready)
    }

    // Nothing is due at noon: the next occurrence is 13:00.
    queue.tick_schedules().await.expect("tick");
    assert_eq!(ready(&queue, &t).await, 0);

    // 13:00:10, ten seconds late and well inside the grace period.
    clock.set(noon + chrono::Duration::seconds(3_610));
    queue.tick_schedules().await.expect("tick");
    assert_eq!(ready(&queue, &t).await, 1, "one firing");
    queue.tick_schedules().await.expect("tick");
    assert_eq!(
        ready(&queue, &t).await,
        1,
        "the fired marker stuck, so the same window does not fire twice"
    );

    let worker = Worker::builder(&queue)
        .queues([t.clone()])
        .poll_interval(Duration::from_millis(10))
        .register::<Nightly>()
        .start()
        .expect("start");
    wait_for(
        "the scheduled job to run",
        Duration::from_secs(10),
        || async { log(&t) == vec!["nightly ran"] },
    )
    .await;
    worker
        .shutdown(Duration::from_secs(5))
        .await
        .expect("shutdown");

    // Take the schedule back out. It is the one row in this file that outlives
    // its test, and a later run's `tick_schedules` would otherwise fire it.
    assert!(queue.remove_recurring(&t).await.expect("remove"));
}

/// The regression test for the one bug in this crate that a laptop cannot see.
///
/// `CREATE TABLE IF NOT EXISTS` is not atomic on Postgres: the catalogue check
/// and the create are separate steps. Eight sessions calling [`Queue::migrate`]
/// at the same moment against a schema that does not yet exist is a rolling
/// deploy in miniature, and before the advisory lock went in it failed with
/// `duplicate key value violates unique constraint "pg_class_relname_nsp_index"`.
///
/// It has to race against an **empty** schema, because once the tables are
/// there `IF NOT EXISTS` short-circuits and there is nothing left to race. So
/// the test makes a throwaway Postgres schema, points the queue at it through
/// the `options=-c search_path=...` parameter, and drops it afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn migrate_is_safe_from_several_connections_at_once() {
    let Some(url) = common::postgres_url() else {
        common::skipped(
            "no Postgres server; set BROCANTH_JOBS_TEST_POSTGRES or \
             BROCANTH_TEST_DATABASE_URL to run this",
        );
        return;
    };

    let schema = format!("brocanth_jobs_race_{}", std::process::id());
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("connecting to Postgres");
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP SCHEMA IF EXISTS {schema} CASCADE"
    )))
    .execute(&admin)
    .await
    .expect("dropping any leftover schema");
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
        .execute(&admin)
        .await
        .expect("creating the throwaway schema");

    let separator = if url.contains('?') { '&' } else { '?' };
    let scoped = format!("{url}{separator}options=-c%20search_path%3D{schema}");

    // Eight at once. Each opens its own pool, exactly as eight instances of an
    // application booting together would.
    let mut racers = Vec::new();
    for _ in 0..8 {
        let scoped = scoped.clone();
        racers.push(tokio::spawn(async move {
            let queue = Queue::connect(&scoped).await?;
            queue.migrate().await
        }));
    }
    let mut failures = Vec::new();
    for racer in racers {
        if let Err(e) = racer.await.expect("the task did not panic") {
            failures.push(e.to_string());
        }
    }

    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT tablename FROM pg_tables WHERE schemaname = $1 ORDER BY tablename",
    )
    .bind(&schema)
    .fetch_all(&admin)
    .await
    .expect("listing the tables");

    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP SCHEMA IF EXISTS {schema} CASCADE"
    )))
    .execute(&admin)
    .await
    .expect("dropping the throwaway schema");

    assert!(
        failures.is_empty(),
        "concurrent migrate() must not fail: {failures:?}"
    );
    assert_eq!(
        tables,
        [
            "brocanth_dead_jobs",
            "brocanth_job_schedules",
            "brocanth_jobs"
        ],
        "all three tables, created once"
    );
}

// ---------------------------------------------------------------------------
// Being told, rather than asking
// ---------------------------------------------------------------------------

/// The claim about `LISTEN`/`NOTIFY`, stated so it cannot pass by accident.
///
/// Both polls are set to a minute. A worker that polls cannot start this job
/// inside the assertion window, so the only way the test passes is if the
/// enqueue woke it. That is the point: a shorter poll here would make the
/// test pass on either mechanism and prove nothing.
#[tokio::test]
async fn an_enqueue_wakes_a_worker_that_would_otherwise_be_asleep() {
    let queue = queue_or_skip!();
    let token = token("notify-wakes");
    let name = format!("q-{token}");

    let worker = Worker::builder(&queue)
        .queues([name.clone()])
        .concurrency(1)
        // A minute each. Neither is allowed to be what makes this work.
        .poll_interval(Duration::from_secs(60))
        .safety_poll(Duration::from_secs(60))
        .register::<Echo>()
        .start()
        .expect("the worker started");

    // Let it reach the idle branch and the listener attach before the row
    // lands, which is the case the notification exists for. A job enqueued
    // before the listener attaches is covered by the ring on connect.
    tokio::time::sleep(Duration::from_millis(400)).await;

    let started = std::time::Instant::now();
    queue
        .enqueue(Echo {
            token: token.clone(),
            n: 1,
        })
        .queue(&name)
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

/// A scheduled job starts when it is due, not at the next tick after it.
///
/// The safety poll is a minute, so if the worker slept its whole cap this
/// would take a minute. It does not, because an idle worker asks when the
/// soonest job is due and sleeps until then.
#[tokio::test]
async fn a_worker_sleeps_until_the_next_job_is_due() {
    let queue = queue_or_skip!();
    let token = token("sleep-until-due");
    let name = format!("q-{token}");

    // Before the enqueue, not after it: `in_` counts from the moment the
    // builder is made, and a contended database can take most of a second
    // to accept the row. Measuring from after it would make this test fail
    // when the machine is busy, which is a test measuring the wrong thing
    // rather than a worker waking early.
    let started = std::time::Instant::now();
    queue
        .enqueue(Echo {
            token: token.clone(),
            n: 1,
        })
        .queue(&name)
        .in_(Duration::from_millis(1200))
        .await
        .expect("enqueued for later");

    let worker = Worker::builder(&queue)
        .queues([name.clone()])
        .concurrency(1)
        .poll_interval(Duration::from_secs(60))
        .safety_poll(Duration::from_secs(60))
        .register::<Echo>()
        .start()
        .expect("the worker started");

    wait_for(
        "the scheduled job to run",
        Duration::from_secs(10),
        || async { !log(&token).is_empty() },
    )
    .await;

    let waited = started.elapsed();
    assert_eq!(log(&token), ["ran 1"]);
    assert!(
        waited >= Duration::from_millis(1000),
        "not before it was due: {waited:?}"
    );
    assert!(
        waited < Duration::from_secs(10),
        "and not a tick later: {waited:?}"
    );

    worker
        .shutdown(Duration::from_secs(5))
        .await
        .expect("the worker stopped");
}

/// With the listener off, the poll is the mechanism again.
///
/// The fallback has to work, because it is what a SQLite deployment runs and
/// what a Postgres deployment runs when its connection limit cannot spare a
/// session per worker.
#[tokio::test]
async fn a_worker_told_not_to_listen_still_finds_the_work() {
    let queue = queue_or_skip!();
    let token = token("no-listen");
    let name = format!("q-{token}");

    let worker = Worker::builder(&queue)
        .queues([name.clone()])
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
        .queue(&name)
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
