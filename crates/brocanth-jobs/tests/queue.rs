//! The seven behaviours this queue is required to have, end to end.
//!
//! Every one of these runs on the memory backend and, when a server is
//! named, on Postgres (`common::on_every_backend!`), with real workers on a
//! real Tokio runtime. The memory backend is not a fake: it is the engine's
//! reference backend, which passes the same conformance suite as Postgres.

#[macro_use]
mod common;

use std::time::Duration;

use brocanth_jobs::{Enqueued, Queue, Worker};
use common::*;

// ---------------------------------------------------------------------------
// 1. A job round-trips through the queue and runs
// ---------------------------------------------------------------------------

async fn a_job_goes_in_comes_out_and_runs(queue: Queue) {
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
            .inspect(enqueued.id().expect("an id"))
            .await
            .expect("inspect")
            .is_none(),
        "the row is gone"
    );
}

// ---------------------------------------------------------------------------
// 2. A failing job retries with growing backoff, then lands in the dead letter
// ---------------------------------------------------------------------------

async fn a_failing_job_backs_off_and_then_lands_in_the_dead_letter(queue: Queue) {
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
    assert_eq!(job.kind, "always_fails");
    assert_eq!(job.attempts, 3, "every attempt is accounted for");
    assert_eq!(job.max_attempts, Some(3));
    assert!(
        job.failure.contains("nope, attempt 3"),
        "the dead letter holds the last error, not a placeholder: {}",
        job.failure
    );
    assert!(job.failure.starts_with("returned"), "{}", job.failure);
    assert!(
        String::from_utf8_lossy(&job.payload).contains(&token),
        "the dead letter holds the payload, so the job can be replayed"
    );
    assert_eq!(
        queue.stats().await.expect("stats").total(),
        0,
        "a dead job is out of the live table"
    );

    // And it is not gone: it can be put back.
    let requeued = queue
        .requeue_dead(&job.id)
        .await
        .expect("requeue")
        .expect("a new row");
    let state = queue
        .inspect(&requeued)
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

async fn two_workers_never_run_the_same_job(queue: Queue) {
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
// 4. A crashed worker's job runs again once its lease runs out
// ---------------------------------------------------------------------------

async fn a_crashed_workers_job_runs_again_when_its_lease_runs_out(queue: Queue) {
    let token = token("orphan");
    let stale_after = Duration::from_millis(300);

    queue
        .enqueue(HangsOnce {
            token: token.clone(),
        })
        .await
        .expect("enqueue");

    // A worker that claims the job and dies: the claim goes through the
    // engine directly, and nothing ever settles it or renews its lease.
    // Aborting a `Worker` task would not do: its guard gives the job back on
    // the way down, which is the clean shutdown and not the crash.
    let claimed = queue
        .engine()
        .claim(&brocanth_queue::ClaimRequest {
            queue: "default".into(),
            max: 1,
            lease: stale_after,
            kinds: None,
            worker: "doomed".into(),
        })
        .await
        .expect("claim");
    assert_eq!(claimed.len(), 1);

    let held = queue.stats().await.expect("stats");
    assert_eq!(
        held.running, 1,
        "the job is still held by a worker that is gone"
    );
    assert_eq!(held.ready, 0);

    // Nothing reclaims it. The lease runs out, and a healthy worker finishes
    // it on the second attempt, because the first one was counted when it
    // was claimed.
    tokio::time::sleep(stale_after).await;
    let rescuer = Worker::builder(&queue)
        .id("rescuer")
        .poll_interval(Duration::from_millis(10))
        .register::<HangsOnce>()
        .start()
        .expect("start");

    wait_for(
        "the orphaned job to finish",
        Duration::from_secs(10),
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

async fn a_unique_key_prevents_a_duplicate(queue: Queue) {
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

async fn shutdown_finishes_what_it_can_and_returns_the_rest(queue: Queue) {
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
        queue
            .inspect(slow.id().expect("an id"))
            .await
            .expect("inspect")
            .is_none(),
        "and its row is gone"
    );

    assert_eq!(
        report.returned, 1,
        "the job that could not finish came back"
    );
    let state = queue
        .inspect(hangs.id().expect("an id"))
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

async fn a_scheduled_job_does_not_run_early(queue: Queue) {
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
        queue
            .inspect(now.id().expect("an id"))
            .await
            .expect("inspect")
            .is_none()
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
        .inspect(later.id().expect("an id"))
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

async fn the_stats_report_the_age_of_the_oldest_runnable_job(queue: Queue) {
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

async fn the_event_hook_reports_what_happened_without_choosing_a_log_crate(queue: Queue) {
    use std::sync::{Arc, Mutex};

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

async fn the_queue_reaches_a_handler_through_the_context(queue: Queue) {
    use brocanth::plugin_context::{Base, ContextBuilder, With};

    let mut builder = ContextBuilder::new();
    builder.insert(queue.clone());
    let cx = builder.build::<With<Queue, Base>>();

    assert_eq!(cx.get::<Queue, _>().backend(), queue.backend());
}

// ---------------------------------------------------------------------------
// Looking at what is waiting
// ---------------------------------------------------------------------------

/// Retries and scheduled jobs are one table and different `attempts`.
///
/// This is the test that pays for there being no fourth state. If a column
/// could not tell these apart, the table would need one.
async fn a_retry_and_a_scheduled_job_are_told_apart_by_attempts(queue: Queue) {
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
            .map(|j| j.kind.as_str())
            .collect::<Vec<_>>(),
        ["echo"],
        "never run, so it is scheduled and not a retry"
    );
    assert!(scheduled.iter().all(|j| j.attempt == 0));

    assert_eq!(
        retries.iter().map(|j| j.kind.as_str()).collect::<Vec<_>>(),
        ["always_fails"],
        "ran and failed, so it is a retry and not scheduled"
    );
    assert!(retries[0].attempt > 0);
    // And a retry carries why, which is the column the screen exists for.
    assert!(
        retries[0].last_error.is_some(),
        "a retry says what went wrong: {:?}",
        retries[0]
    );
}

/// A worker holding a job shows up as busy, and says which worker.
async fn busy_names_the_worker_holding_each_job(queue: Queue) {
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
    assert_eq!(busy[0].kind, "slow");
    assert_eq!(busy[0].attempt, 1, "the attempt that is running");
    assert_eq!(busy[0].claimed_by.as_deref(), Some("worker-under-test"));
    assert!(busy[0].claimed_at.is_some(), "how long it has been running");

    worker.shutdown(Duration::from_secs(5)).await.expect("stop");
}

/// The operator's override, and the one thing it must not touch.
async fn run_now_moves_a_waiting_job_and_leaves_a_running_one_alone(queue: Queue) {
    let t = token("run-now");

    let later = queue
        .enqueue(Echo {
            token: t.clone(),
            n: 1,
        })
        .in_(Duration::from_secs(3600))
        .await
        .expect("scheduled");
    let later = later.id().expect("an id");

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
async fn discard_removes_a_waiting_job_without_a_dead_letter_row(queue: Queue) {
    let t = token("discard");

    let enqueued = queue
        .enqueue(Echo {
            token: t.clone(),
            n: 1,
        })
        .in_(Duration::from_secs(3600))
        .await
        .expect("scheduled");
    let id = enqueued.id().expect("an id");

    assert!(queue.discard(id).await.expect("discard"));
    assert!(queue.scheduled(50, 0).await.expect("scheduled").is_empty());
    assert!(
        queue.dead_jobs(50, 0).await.expect("dead").is_empty(),
        "a deliberate deletion is not a failure and must not look like one"
    );
    assert!(!queue.discard(id).await.expect("discard"), "already gone");
}

/// `stop_when` is the shutdown a plugin wires to SIGTERM: the worker stops
/// claiming when the signal resolves, and a job still running at the
/// deadline goes back to the queue with its attempt given back. It is not
/// dead-lettered, and another worker runs it.
async fn a_job_interrupted_by_stop_when_is_given_back_and_runs_again(queue: Queue) {
    let token = token("stop-when");
    let hangs = queue
        .enqueue(Hangs {
            token: token.clone(),
        })
        .await
        .expect("enqueue");

    let worker = Worker::builder(&queue)
        .poll_interval(Duration::from_millis(10))
        .register::<Hangs>()
        .start()
        .expect("start");
    let (signal, stop) = tokio::sync::oneshot::channel::<()>();
    worker.stop_when(
        async move {
            let _ = stop.await;
        },
        Duration::from_millis(200),
    );

    wait_for("the job to start", Duration::from_secs(5), || async {
        queue.stats().await.expect("stats").running == 1
    })
    .await;
    signal.send(()).expect("the watcher waits");
    // Time for the watcher to send its stop. A second stop does not move the
    // deadline the first one set.
    tokio::time::sleep(Duration::from_millis(50)).await;
    worker.stop(Duration::from_secs(60));

    let report = tokio::time::timeout(Duration::from_secs(5), worker.finished())
        .await
        .expect("the worker ends by the first deadline")
        .expect("finished");
    assert_eq!(report.returned, 1);
    let state = queue
        .inspect(hangs.id().expect("an id"))
        .await
        .expect("inspect")
        .expect("the row is back");
    assert_eq!(state.state, "pending");
    assert_eq!(state.attempts, 0, "a shutdown is not a failed attempt");
    assert_eq!(queue.stats().await.expect("stats").dead, 0);

    // And a second worker runs it.
    let again = Worker::builder(&queue)
        .poll_interval(Duration::from_millis(10))
        .register::<Hangs>()
        .start()
        .expect("start");
    wait_for("the job to run again", Duration::from_secs(5), || async {
        queue.stats().await.expect("stats").running == 1
    })
    .await;
    again.abort();
}

on_every_backend!(
    a_job_goes_in_comes_out_and_runs,
    a_failing_job_backs_off_and_then_lands_in_the_dead_letter,
    two_workers_never_run_the_same_job,
    a_crashed_workers_job_runs_again_when_its_lease_runs_out,
    a_unique_key_prevents_a_duplicate,
    shutdown_finishes_what_it_can_and_returns_the_rest,
    a_scheduled_job_does_not_run_early,
    the_stats_report_the_age_of_the_oldest_runnable_job,
    the_event_hook_reports_what_happened_without_choosing_a_log_crate,
    the_queue_reaches_a_handler_through_the_context,
    a_retry_and_a_scheduled_job_are_told_apart_by_attempts,
    busy_names_the_worker_holding_each_job,
    run_now_moves_a_waiting_job_and_leaves_a_running_one_alone,
    discard_removes_a_waiting_job_without_a_dead_letter_row,
    a_job_interrupted_by_stop_when_is_given_back_and_runs_again,
);
