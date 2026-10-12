//! The same guarantees, on Postgres, where the claim really does use
//! `FOR UPDATE SKIP LOCKED`.
//!
//! Compiled only with `--features postgres`, and skipped at run time unless
//! `BROCANTH_JOBS_TEST_POSTGRES` names a database:
//!
//! ```sh
//! BROCANTH_JOBS_TEST_POSTGRES=postgres://localhost/brocanth_jobs_test \
//!   cargo test -p brocanth-jobs --features postgres --test postgres
//! ```
//!
//! Skipping rather than failing, because `cargo test -p brocanth-jobs` has to
//! pass on a laptop with no server running — and because the suite in
//! `queue.rs` already proves the behaviour on a real database. What these add
//! is that the Postgres *dialect* of every statement is valid and that the
//! locking claim behaves the same as the serialised one.

#![cfg(feature = "postgres")]

mod common;

use std::time::Duration;

use brocanth_jobs::{Enqueued, Queue, Worker};
use common::{log, token, wait_for, AlwaysFails, Echo};

/// The queue under test, or `None` when no database was named.
async fn postgres_queue() -> Option<Queue> {
    let url = std::env::var("BROCANTH_JOBS_TEST_POSTGRES").ok()?;
    let queue = Queue::connect(&url).await.expect("connecting to Postgres");
    queue.migrate().await.expect("creating the tables");
    // One database serves every test in this file, and they run in parallel,
    // so nothing here truncates anything. Each test works inside its own queue
    // name and its own token instead, which is also what two applications
    // sharing a database would do.
    Some(queue)
}

macro_rules! queue_or_skip {
    () => {
        match postgres_queue().await {
            Some(queue) => queue,
            None => {
                eprintln!("skipped: set BROCANTH_JOBS_TEST_POSTGRES to run the Postgres tests");
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
