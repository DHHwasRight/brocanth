//! The seven behaviours this queue is required to have, end to end.
//!
//! Every one of these runs on SQLite, against a real file, with real workers
//! on a real Tokio runtime. There is no in-memory fake: a queue that is a
//! different implementation in a test is the Rails trap in miniature, and a
//! test that passes against a fake proves nothing about the SQL.

mod common;

use std::time::Duration;

use brocanth_jobs::{Enqueued, Queue, Worker};
use common::*;

// ---------------------------------------------------------------------------
// 1. A job round-trips through the queue and runs
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_job_goes_in_comes_out_and_runs() {
    let (queue, _dir) = sqlite_queue().await;
    let token = token("round-trip");

    let enqueued = queue
        .enqueue(Echo {
            token: token.clone(),
            n: 1,
        })
        .await
        .expect("enqueue");
    assert!(matches!(enqueued, Enqueued::Inserted(_)));

    let before = queue.stats().await.expect("stats");
    assert_eq!(before.ready, 1, "one job waiting");
    assert_eq!(before.running, 0);

    let worker = Worker::builder(&queue)
        .poll_interval(Duration::from_millis(10))
        .register::<Echo>()
        .start()
        .expect("start");

    wait_for("the job to run", Duration::from_secs(5), || async {
        log(&token) == vec!["ran 1"]
    })
    .await;

    worker
        .shutdown(Duration::from_secs(2))
        .await
        .expect("shutdown");

    let after = queue.stats().await.expect("stats");
    assert_eq!(after.total(), 0, "a finished job leaves no row behind");
    assert_eq!(after.dead, 0);
    assert!(
        queue
            .inspect(enqueued.id())
            .await
            .expect("inspect")
            .is_none(),
        "the row is gone"
    );
}

// ---------------------------------------------------------------------------
// 2. A failing job retries with growing backoff, then lands in the dead letter
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_job_backs_off_and_then_lands_in_the_dead_letter() {
    let (queue, _dir) = sqlite_queue().await;
    let token = token("retries");

    queue
        .enqueue(AlwaysFails {
            token: token.clone(),
        })
        .await
        .expect("enqueue");

    let worker = Worker::builder(&queue)
        .poll_interval(Duration::from_millis(10))
        .register::<AlwaysFails>()
        .start()
        .expect("start");

    // `AlwaysFails` allows three attempts with a 120 ms base, doubling, no
    // jitter: ~120 ms before the second and ~240 ms before the third.
    wait_for("three attempts", Duration::from_secs(10), || async {
        log(&token).len() >= 3
    })
    .await;

    let attempts = log(&token);
    assert_eq!(
        attempts,
        vec!["attempt 1", "attempt 2", "attempt 3"],
        "each retry reports its attempt number"
    );

    let at = timings(&token);
    let first_gap = at[1] - at[0];
    let second_gap = at[2] - at[1];
    assert!(
        first_gap >= Duration::from_millis(100),
        "the first retry waited {first_gap:?}, which is less than the 120 ms base"
    );
    assert!(
        second_gap >= Duration::from_millis(220),
        "the second retry waited {second_gap:?}, which is less than the 240 ms step"
    );
    assert!(
        second_gap > first_gap + Duration::from_millis(50),
        "the backoff did not grow: {first_gap:?} then {second_gap:?}"
    );

    wait_for("the dead letter", Duration::from_secs(5), || async {
        queue.stats().await.expect("stats").dead == 1
    })
    .await;

    worker
        .shutdown(Duration::from_secs(2))
        .await
        .expect("shutdown");

    let dead = queue.dead_jobs(10, 0).await.expect("dead jobs");
    assert_eq!(dead.len(), 1);
    let job = &dead[0];
    assert_eq!(job.job_name, "always_fails");
    assert_eq!(job.attempts, 3, "every attempt is accounted for");
    assert_eq!(job.max_attempts, 3);
    assert!(
        job.error.contains("nope, attempt 3"),
        "the dead letter holds the last error, not a placeholder: {}",
        job.error
    );
    assert_eq!(job.failure_kind, "returned");
    assert!(
        job.payload.contains(&token),
        "the dead letter holds the payload, so the job can be replayed"
    );
    assert_eq!(
        queue.stats().await.expect("stats").total(),
        0,
        "a dead job is out of the live table"
    );

    // And it is not gone: it can be put back.
    let requeued = queue
        .requeue_dead(job.id)
        .await
        .expect("requeue")
        .expect("a new row");
    let state = queue
        .inspect(requeued)
        .await
        .expect("inspect")
        .expect("the requeued row");
    assert_eq!(state.state, "pending");
    assert_eq!(state.attempts, 0, "a requeue starts the attempts again");
    assert_eq!(queue.stats().await.expect("stats").dead, 0);
}

// ---------------------------------------------------------------------------
// 3. Two workers never run the same job
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_workers_never_run_the_same_job() {
    let (queue, _dir) = sqlite_queue().await;
    let token = token("skip-locked");
    const JOBS: u32 = 40;

    for n in 0..JOBS {
        queue
            .enqueue(Echo {
                token: token.clone(),
                n,
            })
            .await
            .expect("enqueue");
    }

    let a = Worker::builder(&queue)
        .id("worker-a")
        .concurrency(4)
        .poll_interval(Duration::from_millis(5))
        .register::<Echo>()
        .start()
        .expect("start a");
    let b = Worker::builder(&queue)
        .id("worker-b")
        .concurrency(4)
        .poll_interval(Duration::from_millis(5))
        .register::<Echo>()
        .start()
        .expect("start b");

    wait_for("every job to run", Duration::from_secs(20), || async {
        log(&token).len() >= JOBS as usize
    })
    .await;

    a.shutdown(Duration::from_secs(2)).await.expect("stop a");
    b.shutdown(Duration::from_secs(2)).await.expect("stop b");

    let ran = log(&token);
    let unique: std::collections::HashSet<&String> = ran.iter().collect();
    assert_eq!(
        unique.len(),
        JOBS as usize,
        "{} runs for {JOBS} jobs: something was claimed twice",
        ran.len()
    );
    assert_eq!(
        ran.len(),
        JOBS as usize,
        "a job ran more than once: {:?}",
        duplicates(&ran)
    );
    assert_eq!(queue.stats().await.expect("stats").total(), 0);
}

fn duplicates(entries: &[String]) -> Vec<&String> {
    let mut seen = std::collections::HashSet::new();
    entries.iter().filter(|e| !seen.insert(*e)).collect()
}

// ---------------------------------------------------------------------------
// 4. A crashed worker's job is reclaimed once its heartbeat lapses
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_crashed_workers_job_is_reclaimed_and_finished_by_another() {
    let (queue, _dir) = sqlite_queue().await;
    let token = token("orphan");
    let stale_after = Duration::from_millis(300);

    queue
        .enqueue(HangsOnce {
            token: token.clone(),
        })
        .await
        .expect("enqueue");

    // A worker that will be killed mid-job. Its maintenance tick is long
    // enough that it cannot reclaim its own work before it dies.
    let doomed = Worker::builder(&queue)
        .id("doomed")
        .concurrency(1)
        .poll_interval(Duration::from_millis(10))
        .heartbeat_interval(Duration::from_millis(50))
        .stale_after(stale_after)
        .maintenance_interval(Duration::from_secs(60))
        .register::<HangsOnce>()
        .start()
        .expect("start");

    wait_for(
        "the job to be picked up",
        Duration::from_secs(5),
        || async { queue.stats().await.expect("stats").running == 1 },
    )
    .await;

    // `kill -9`: no drain, no release. The row stays locked and `running`.
    doomed.abort();

    let held = queue.stats().await.expect("stats");
    assert_eq!(
        held.running, 1,
        "the job is still held by a worker that is gone"
    );
    assert_eq!(held.ready, 0);

    // Too soon: the heartbeat has not lapsed yet, so nothing is taken.
    let early = queue
        .reclaim_orphans(Duration::from_secs(60))
        .await
        .expect("reclaim");
    assert_eq!(
        early.requeued, 0,
        "a worker that merely went quiet for a moment keeps its jobs"
    );

    tokio::time::sleep(stale_after + Duration::from_millis(100)).await;

    let reclaimed = queue.reclaim_orphans(stale_after).await.expect("reclaim");
    assert_eq!(reclaimed.requeued, 1, "the orphan came back");
    assert_eq!(reclaimed.dead_lettered, 0);

    let state = queue.stats().await.expect("stats");
    assert_eq!(state.ready, 1);
    assert_eq!(state.running, 0);

    // And a healthy worker finishes it, on the second attempt, because the
    // first one was counted when it was claimed.
    let rescuer = Worker::builder(&queue)
        .id("rescuer")
        .poll_interval(Duration::from_millis(10))
        .register::<HangsOnce>()
        .start()
        .expect("start");

    wait_for(
        "the reclaimed job to finish",
        Duration::from_secs(5),
        || async { log(&token).contains(&"finished on attempt 2".to_string()) },
    )
    .await;

    rescuer
        .shutdown(Duration::from_secs(2))
        .await
        .expect("shutdown");
    assert_eq!(queue.stats().await.expect("stats").total(), 0);
}

// ---------------------------------------------------------------------------
// 5. A unique key prevents a duplicate
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_unique_key_prevents_a_duplicate() {
    let (queue, _dir) = sqlite_queue().await;
    let token = token("unique");

    let first = queue
        .enqueue(Echo {
            token: token.clone(),
            n: 1,
        })
        .unique("invoice:42")
        .await
        .expect("enqueue");
    let second = queue
        .enqueue(Echo {
            token: token.clone(),
            n: 2,
        })
        .unique("invoice:42")
        .await
        .expect("enqueue");

    assert!(matches!(first, Enqueued::Inserted(_)));
    assert!(
        second.is_duplicate(),
        "the second enqueue folded into the first"
    );
    assert_eq!(
        second.id(),
        first.id(),
        "and it reports the row that is actually queued"
    );
    assert_eq!(
        queue.stats().await.expect("stats").ready,
        1,
        "one row, not two"
    );

    // A different key is a different job.
    queue
        .enqueue(Echo {
            token: token.clone(),
            n: 3,
        })
        .unique("invoice:43")
        .await
        .expect("enqueue");
    assert_eq!(queue.stats().await.expect("stats").ready, 2);

    let worker = Worker::builder(&queue)
        .poll_interval(Duration::from_millis(10))
        .register::<Echo>()
        .start()
        .expect("start");

    wait_for("both jobs to run", Duration::from_secs(5), || async {
        log(&token).len() >= 2
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    worker
        .shutdown(Duration::from_secs(2))
        .await
        .expect("shutdown");

    let mut ran = log(&token);
    ran.sort();
    assert_eq!(
        ran,
        vec!["ran 1", "ran 3"],
        "the deduplicated payload never ran"
    );

    // The key is released when the job finishes: uniqueness means "not queued
    // twice", not "never run again".
    let again = queue
        .enqueue(Echo {
            token: token.clone(),
            n: 4,
        })
        .unique("invoice:42")
        .await
        .expect("enqueue");
    assert!(
        !again.is_duplicate(),
        "once the job has run, its key is free again"
    );
}

// ---------------------------------------------------------------------------
// 6. Shutdown finishes in-flight work and returns the rest
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_finishes_what_it_can_and_returns_the_rest() {
    let (queue, _dir) = sqlite_queue().await;
    let token = token("shutdown");

    let slow = queue
        .enqueue(Slow {
            token: token.clone(),
            millis: 200,
        })
        .await
        .expect("enqueue");
    let hangs = queue
        .enqueue(Hangs {
            token: token.clone(),
        })
        .await
        .expect("enqueue");

    let worker = Worker::builder(&queue)
        .concurrency(2)
        .poll_interval(Duration::from_millis(10))
        .register::<Slow>()
        .register::<Hangs>()
        .start()
        .expect("start");

    wait_for("both jobs to start", Duration::from_secs(5), || async {
        queue.stats().await.expect("stats").running == 2
    })
    .await;

    let report = worker
        .shutdown(Duration::from_secs(2))
        .await
        .expect("shutdown");

    assert!(
        log(&token).contains(&"slow finished".to_string()),
        "the job that could finish inside the grace period did: {:?}",
        log(&token)
    );
    assert!(
        queue.inspect(slow.id()).await.expect("inspect").is_none(),
        "and its row is gone"
    );

    assert_eq!(
        report.returned, 1,
        "the job that could not finish came back"
    );
    let state = queue
        .inspect(hangs.id())
        .await
        .expect("inspect")
        .expect("the hanging job's row");
    assert_eq!(state.state, "pending", "it is runnable again");
    assert_eq!(
        state.attempts, 0,
        "and the attempt it had taken was given back: a deploy is not a failure"
    );
    assert_eq!(queue.stats().await.expect("stats").dead, 0);
}

// ---------------------------------------------------------------------------
// 7. A scheduled job does not run early
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scheduled_job_does_not_run_early() {
    let (queue, _dir) = sqlite_queue().await;
    let token = token("scheduled");

    let later = queue
        .enqueue(Echo {
            token: token.clone(),
            n: 1,
        })
        .in_(Duration::from_secs(3_600))
        .await
        .expect("enqueue");
    let now = queue
        .enqueue(Echo {
            token: token.clone(),
            n: 2,
        })
        .await
        .expect("enqueue");

    let stats = queue.stats().await.expect("stats");
    assert_eq!(stats.scheduled, 1, "the future job is scheduled, not ready");
    assert_eq!(stats.ready, 1);

    let worker = Worker::builder(&queue)
        .poll_interval(Duration::from_millis(10))
        .register::<Echo>()
        .start()
        .expect("start");

    wait_for("the ready job to run", Duration::from_secs(5), || async {
        queue.inspect(now.id()).await.expect("inspect").is_none()
    })
    .await;
    // Give a worker polling every 10 ms three hundred chances to get it wrong.
    tokio::time::sleep(Duration::from_millis(300)).await;
    worker
        .shutdown(Duration::from_secs(2))
        .await
        .expect("shutdown");

    assert_eq!(log(&token), vec!["ran 2"], "only the job that was due ran");
    let state = queue
        .inspect(later.id())
        .await
        .expect("inspect")
        .expect("the scheduled row");
    assert_eq!(state.state, "pending");
    assert_eq!(state.attempts, 0, "it was never even claimed");
    assert!(state.run_at > queue.now() + chrono::Duration::minutes(50));
}

// ---------------------------------------------------------------------------
// The oldest-ready age, which is the number that says a queue is behind
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_stats_report_the_age_of_the_oldest_runnable_job() {
    let (queue, _dir) = sqlite_queue().await;
    let token = token("stats");

    assert!(queue
        .stats()
        .await
        .expect("stats")
        .oldest_ready_age
        .is_none());

    queue
        .enqueue(Echo {
            token: token.clone(),
            n: 1,
        })
        .queue("slow-lane")
        .await
        .expect("enqueue");
    tokio::time::sleep(Duration::from_millis(150)).await;
    queue
        .enqueue(Echo {
            token: token.clone(),
            n: 2,
        })
        .await
        .expect("enqueue");

    let stats = queue.stats().await.expect("stats");
    assert_eq!(stats.ready, 2);
    let age = stats.oldest_ready_age.expect("an age");
    assert!(
        age >= Duration::from_millis(140),
        "the oldest job has been waiting {age:?}"
    );
    assert_eq!(stats.queues.len(), 2, "counted per queue, sorted by name");
    assert_eq!(stats.queues[0].queue, "default");
    assert_eq!(stats.queues[1].queue, "slow-lane");
    assert_eq!(stats.queues[1].ready, 1);
    assert!(stats.queues[1].oldest_ready_age > stats.queues[0].oldest_ready_age);
}

// ---------------------------------------------------------------------------
// Events, which are how an application sees any of this
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_event_hook_reports_what_happened_without_choosing_a_log_crate() {
    use std::sync::{Arc, Mutex};

    let (queue, _dir) = sqlite_queue().await;
    let token = token("events");
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let sink = Arc::clone(&seen);
    let queue = queue.with_events(brocanth_jobs::EventFn::new(move |event| {
        sink.lock().expect("events").push(match event {
            brocanth_jobs::JobEvent::Enqueued { .. } => "enqueued".to_string(),
            brocanth_jobs::JobEvent::Started { .. } => "started".to_string(),
            brocanth_jobs::JobEvent::Completed { .. } => "completed".to_string(),
            other => format!("{other}"),
        });
    }));

    queue
        .enqueue(Echo {
            token: token.clone(),
            n: 1,
        })
        .await
        .expect("enqueue");

    let worker = Worker::builder(&queue)
        .poll_interval(Duration::from_millis(10))
        .register::<Echo>()
        .start()
        .expect("start");
    wait_for("the job to run", Duration::from_secs(5), || async {
        seen.lock()
            .expect("events")
            .iter()
            .any(|e| e == "completed")
    })
    .await;
    worker
        .shutdown(Duration::from_secs(2))
        .await
        .expect("shutdown");

    let events = seen.lock().expect("events").clone();
    for expected in ["enqueued", "started", "completed"] {
        assert!(
            events.iter().any(|e| e == expected),
            "no `{expected}` in {events:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Queue is a context item, so a handler can enqueue
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_queue_reaches_a_handler_through_the_context() {
    use brocanth::plugin_context::{Base, ContextBuilder, With};

    let (queue, _dir) = sqlite_queue().await;
    let mut builder = ContextBuilder::new();
    builder.insert(queue.clone());
    let cx = builder.build::<With<Queue, Base>>();

    assert_eq!(cx.get::<Queue, _>().backend(), "sqlite");
}

// ---------------------------------------------------------------------------
// 8. The other half of the migration-concurrency guarantee
// ---------------------------------------------------------------------------

/// `CREATE TABLE IF NOT EXISTS` racing itself, on SQLite.
///
/// On Postgres this is a real bug: the catalogue check and the create are
/// separate steps, so two sessions both decide to create and the loser fails
/// with a `pg_class_relname_nsp_index` violation. `tests/postgres.rs` holds
/// that race, and `Store::migrate` answers it with an advisory lock.
///
/// SQLite has no advisory locks, and the finding here is that it needs none:
/// this passes with or without the transaction `Store::migrate` now takes,
/// because SQLite holds one write lock for the whole file and no second writer
/// can get between the check and the create. The transaction buys the other
/// half — all three tables or none — not this half. Written down because
/// "SQLite is probably fine" is the kind of claim that is worth a test rather
/// than a sentence.
///
/// The pool is opened once first, on purpose, and that is a finding of its
/// own. Opening eight *new* pools on a database file that does not exist yet
/// fails intermittently with `SQLITE_BUSY`, in `Queue::connect` rather than in
/// `migrate`: the switch into WAL mode needs an exclusive lock that
/// `sqlite3_busy_timeout` explicitly cannot wait on, which sqlx documents on
/// `SqliteConnectOptions::journal_mode`. That is a separate bug and this test
/// deliberately does not cover it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn migrate_is_safe_from_several_connections_at_once() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let url = format!("sqlite://{}", dir.path().join("race.db").display());

    // Create the file and settle its journal mode before anything races.
    let first = Queue::connect(&url).await.expect("connect");

    let mut racers = Vec::new();
    for _ in 0..8 {
        let url = url.clone();
        racers.push(tokio::spawn(async move {
            Queue::connect(&url).await.expect("connect").migrate().await
        }));
    }
    for racer in racers {
        racer
            .await
            .expect("the task did not panic")
            .expect("concurrent migrate() must not fail");
    }

    // The queue works afterwards, which is the proof that the schema is whole
    // rather than merely that nothing returned an error.
    let t = token("sqlite-race");
    first
        .enqueue(Echo {
            token: t.clone(),
            n: 1,
        })
        .await
        .expect("enqueue");
    assert_eq!(first.stats().await.expect("stats").ready, 1);
    first
        .dead_jobs(1, 0)
        .await
        .expect("the dead letter is there");
    first
        .tick_schedules()
        .await
        .expect("the schedules are there");
}

// ---------------------------------------------------------------------------
// Looking at what is waiting
// ---------------------------------------------------------------------------

/// Retries and scheduled jobs are one table and different `attempts`.
///
/// This is the test that pays for there being no fourth state. If a column
/// could not tell these apart, the table would need one.
#[tokio::test]
async fn a_retry_and_a_scheduled_job_are_told_apart_by_attempts() {
    let (queue, _dir) = sqlite_queue().await;
    let t = token("waiting");

    // Never run, due in an hour.
    queue
        .enqueue(Echo {
            token: t.clone(),
            n: 1,
        })
        .in_(Duration::from_secs(3600))
        .await
        .expect("scheduled");

    // Runs, fails, and is put back with a backoff ahead of it.
    queue
        .enqueue(AlwaysFails { token: t.clone() })
        .await
        .expect("enqueued");
    let worker = Worker::builder(&queue)
        .concurrency(1)
        .poll_interval(Duration::from_millis(20))
        .register::<AlwaysFails>()
        .register::<Echo>()
        .start()
        .expect("started");
    wait_for("the first failure", Duration::from_secs(5), || async {
        !log(&t).is_empty()
    })
    .await;
    worker.shutdown(Duration::from_secs(5)).await.expect("stop");

    let scheduled = queue.scheduled(50, 0).await.expect("scheduled");
    let retries = queue.retries(50, 0).await.expect("retries");

    assert_eq!(
        scheduled
            .iter()
            .map(|j| j.job_name.as_str())
            .collect::<Vec<_>>(),
        ["echo"],
        "never run, so it is scheduled and not a retry"
    );
    assert!(scheduled.iter().all(|j| j.attempts == 0));

    assert_eq!(
        retries
            .iter()
            .map(|j| j.job_name.as_str())
            .collect::<Vec<_>>(),
        ["always_fails"],
        "ran and failed, so it is a retry and not scheduled"
    );
    assert!(retries[0].attempts > 0);
    // And a retry carries why, which is the column the screen exists for.
    assert!(
        retries[0].last_error.is_some(),
        "a retry says what went wrong: {:?}",
        retries[0]
    );
}

/// A worker holding a job shows up as busy, and says which worker.
#[tokio::test]
async fn busy_names_the_worker_holding_each_job() {
    let (queue, _dir) = sqlite_queue().await;
    let t = token("busy");

    assert!(
        queue.busy(50, 0).await.expect("busy").is_empty(),
        "nothing is running yet"
    );

    queue
        .enqueue(Slow {
            token: t.clone(),
            millis: 2_000,
        })
        .await
        .expect("enqueued");
    let worker = Worker::builder(&queue)
        .id("worker-under-test")
        .concurrency(1)
        .poll_interval(Duration::from_millis(20))
        .register::<Slow>()
        .start()
        .expect("started");

    wait_for("the job to be claimed", Duration::from_secs(5), || {
        let queue = queue.clone();
        async move { !queue.busy(50, 0).await.expect("busy").is_empty() }
    })
    .await;

    let busy = queue.busy(50, 0).await.expect("busy");
    assert_eq!(busy.len(), 1);
    assert_eq!(busy[0].job_name, "slow");
    assert_eq!(busy[0].attempts, 1, "the attempt that is running");
    assert_eq!(busy[0].locked_by.as_deref(), Some("worker-under-test"));
    assert!(busy[0].locked_at.is_some(), "how long it has been running");
    assert!(
        busy[0].heartbeat_at.is_some(),
        "and whether it is still alive"
    );

    worker.shutdown(Duration::from_secs(5)).await.expect("stop");
}

/// The operator's override, and the one thing it must not touch.
#[tokio::test]
async fn run_now_moves_a_waiting_job_and_leaves_a_running_one_alone() {
    let (queue, _dir) = sqlite_queue().await;
    let t = token("run-now");

    let later = queue
        .enqueue(Echo {
            token: t.clone(),
            n: 1,
        })
        .in_(Duration::from_secs(3600))
        .await
        .expect("scheduled")
        .id();

    assert_eq!(queue.scheduled(50, 0).await.expect("scheduled").len(), 1);
    assert!(
        queue.run_now(later).await.expect("run now"),
        "it was waiting"
    );
    assert!(
        queue.scheduled(50, 0).await.expect("scheduled").is_empty(),
        "no longer waiting for its time"
    );

    let worker = Worker::builder(&queue)
        .concurrency(1)
        .poll_interval(Duration::from_millis(20))
        .register::<Echo>()
        .start()
        .expect("started");
    wait_for("the job to run", Duration::from_secs(5), || async {
        !log(&t).is_empty()
    })
    .await;
    assert_eq!(log(&t), ["ran 1"]);
    worker.shutdown(Duration::from_secs(5)).await.expect("stop");

    // And a job that is gone is not a job that was moved.
    assert!(!queue.run_now(later).await.expect("run now"));
}

/// Discarding is not dead-lettering, and the dead letter must not collect
/// jobs somebody deliberately threw away.
#[tokio::test]
async fn discard_removes_a_waiting_job_without_a_dead_letter_row() {
    let (queue, _dir) = sqlite_queue().await;
    let t = token("discard");

    let id = queue
        .enqueue(Echo {
            token: t.clone(),
            n: 1,
        })
        .in_(Duration::from_secs(3600))
        .await
        .expect("scheduled")
        .id();

    assert!(queue.discard(id).await.expect("discard"));
    assert!(queue.scheduled(50, 0).await.expect("scheduled").is_empty());
    assert!(
        queue.dead_jobs(50, 0).await.expect("dead").is_empty(),
        "a deliberate deletion is not a failure and must not look like one"
    );
    assert!(!queue.discard(id).await.expect("discard"), "already gone");
}
