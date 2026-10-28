//! The adapter, held to the same contract as every other backend.
//!
//! Set `BROCANTH_QUEUE_TEST_DATABASE_URL` to run. Without it each test
//! prints that it was skipped and returns. Set `BROCANTH_QUEUE_REQUIRE_POSTGRES`
//! as well and a missing URL fails instead, which is what CI should do: a
//! conformance suite that quietly does not run is worse than none.

use std::sync::Arc;
use std::time::Duration;

// Only `DynBackend` is imported. Both traits carry the same method names,
// and the blanket impl means a concrete adapter has both, so importing both
// into one scope is ambiguous. In practice one of them is always the wrong
// one: an application uses neither, an adapter author uses `Backend`, and
// everything that holds a backend at run time uses `DynBackend`.
use brocanth_queue::{
    Bucket, ClaimRequest, Client, DynBackend, Inspect, Job, JobContext, JobFailure, Lane, NewJob,
    Outbox, Page, Pushed, Relay, RelayConfig, Settle, UniquenessKind, View,
};
use brocanth_queue_postgres::{migrate, PgOutbox, PgQueue, Table, Tx};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

async fn pool() -> Option<PgPool> {
    let url = std::env::var("BROCANTH_QUEUE_TEST_DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .expect("could not reach BROCANTH_QUEUE_TEST_DATABASE_URL");
    migrate(&pool).await.expect("migrate");
    Some(pool)
}

/// One database, one test at a time.
///
/// These tests share tables in a single database, and some of them work the
/// same queue. Run in parallel they read each other's rows, and the failure
/// looks like a bug in the adapter rather than a bug in the harness. A
/// conformance suite that reports a fault that is not there costs more than
/// the wall-clock time this lock spends.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

macro_rules! db {
    () => {
        match pool().await {
            Some(p) => p,
            None => {
                assert!(
                    std::env::var("BROCANTH_QUEUE_REQUIRE_POSTGRES").is_err(),
                    "BROCANTH_QUEUE_REQUIRE_POSTGRES is set and \
                     BROCANTH_QUEUE_TEST_DATABASE_URL is not"
                );
                eprintln!("skipped: BROCANTH_QUEUE_TEST_DATABASE_URL is not set");
                return;
            }
        }
    };
}

#[tokio::test(flavor = "multi_thread")]
async fn the_postgres_backend_conforms() {
    let _serial = SERIAL.lock().await;
    let pool = db!();
    let backend: Arc<dyn DynBackend> = Arc::new(PgQueue::bulk(pool));
    brocanth_queue_conformance::assert_conformant(backend).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_transactional_table_conforms_too() {
    // The lane that never moves is read with the same code, so it is held
    // to the same contract.
    let _serial = SERIAL.lock().await;
    let pool = db!();
    let backend: Arc<dyn DynBackend> = Arc::new(PgQueue::transactional(pool));
    brocanth_queue_conformance::assert_conformant(backend).await;
}

#[derive(Serialize, Deserialize)]
struct Charge {
    order: i64,
}

impl Job for Charge {
    const KIND: &'static str = "pg.charge";
    const LANE: Lane = Lane::Transactional;
    const QUEUE: &'static str = "money";
    type Error = std::io::Error;
    async fn run(&self, _cx: &JobContext) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct Receipt {
    order: i64,
}

impl Job for Receipt {
    const KIND: &'static str = "pg.receipt";
    const QUEUE: &'static str = "mail";
    type Error = std::io::Error;
    async fn run(&self, _cx: &JobContext) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rollback_takes_both_jobs_with_it() {
    let _serial = SERIAL.lock().await;
    let pool = db!();
    let bulk = PgQueue::bulk(pool.clone());
    let txq = PgQueue::transactional(pool.clone()).with_config(brocanth_queue_postgres::PgConfig {
        table: Table::Transactional,
        ..Default::default()
    });
    let client = Client::new(Arc::new(PgQueue::bulk(pool.clone())));
    let outbox = PgOutbox::new(pool.clone());

    for sql in [
        "DELETE FROM queue_tx WHERE queue = 'money'",
        "DELETE FROM queue_outbox WHERE queue = 'mail'",
    ] {
        sqlx::query(sql).execute(&pool).await.unwrap();
    }
    let before = outbox.pending().await.unwrap();
    // `queue_bulk` is not cleared and not asserted to be empty. Another test
    // in this file delivers to the same queue, they share one database, and
    // cargo runs them at the same time. What this test is about is the
    // difference the rolled back transaction made, so measure that.
    let bulk_before = bulk.approx_depth("mail").await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    client
        .enqueue(Charge { order: 1 })
        .in_tx(&mut Tx::new(&mut tx))
        .await
        .unwrap();
    client
        .enqueue(Receipt { order: 1 })
        .in_tx(&mut Tx::new(&mut tx))
        .await
        .unwrap();
    tx.rollback().await.unwrap();

    assert_eq!(
        txq.approx_depth("money").await.unwrap(),
        0,
        "a rolled back transaction must not leave a transactional job"
    );
    assert_eq!(
        outbox.pending().await.unwrap(),
        before,
        "a rolled back transaction must not leave an outbox row"
    );
    assert_eq!(
        bulk.approx_depth("mail").await.unwrap(),
        bulk_before,
        "a rolled back transaction must not add a bulk job"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_commit_delivers_the_bulk_job_through_the_relay() {
    let _serial = SERIAL.lock().await;
    let pool = db!();
    let bulk: Arc<dyn DynBackend> = Arc::new(PgQueue::bulk(pool.clone()));
    let client = Client::new(Arc::clone(&bulk));
    let outbox = Arc::new(PgOutbox::new(pool.clone()));

    sqlx::query("DELETE FROM queue_bulk WHERE queue = 'mail'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM queue_outbox WHERE queue = 'mail'")
        .execute(&pool)
        .await
        .unwrap();

    let mut tx = pool.begin().await.unwrap();
    let pushed = client
        .enqueue(Receipt { order: 7 })
        .in_tx(&mut Tx::new(&mut tx))
        .await
        .unwrap();
    assert!(matches!(pushed, Pushed::Enqueued(_)));
    // Before the commit, the relay can see nothing.
    assert_eq!(
        outbox
            .fetch(10, Duration::from_secs(5))
            .await
            .unwrap()
            .len(),
        0
    );
    tx.commit().await.unwrap();

    let mut relay = Relay::new(Arc::clone(&outbox), bulk.clone(), RelayConfig::default());
    let tick = relay.tick().await.unwrap();
    assert_eq!(tick.delivered, 1, "{tick:?}");
    assert_eq!(outbox.pending().await.unwrap(), 0);
    assert_eq!(bulk.approx_depth("mail").await.unwrap(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_outbox_refuses_work_it_cannot_deliver() {
    let _serial = SERIAL.lock().await;
    let pool = db!();
    let client = Client::new(Arc::new(PgQueue::bulk(pool.clone())));
    sqlx::query("DELETE FROM queue_outbox WHERE queue = 'mail'")
        .execute(&pool)
        .await
        .unwrap();

    let mut tx = pool.begin().await.unwrap();
    for n in 0..3 {
        client
            .enqueue(Receipt { order: n })
            .in_tx(&mut Tx::new(&mut tx).max_pending(100))
            .await
            .unwrap();
    }
    // The limit is now below what is already waiting.
    let err = client
        .enqueue(Receipt { order: 99 })
        .in_tx(&mut Tx::new(&mut tx).max_pending(2))
        .await
        .unwrap_err();
    assert!(
        matches!(err, brocanth_queue::EnqueueError::OutboxFull { .. }),
        "{err}"
    );
    tx.rollback().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_long_delay_waits_in_the_scheduled_table_and_is_promoted() {
    let _serial = SERIAL.lock().await;
    let pool = db!();
    let q = PgQueue::bulk(pool.clone()).with_config(brocanth_queue_postgres::PgConfig {
        // Anything more than a moment goes to the scheduled table, and the
        // promoter brings back anything due inside a minute. A job two
        // seconds out therefore takes both paths.
        schedule_after: Duration::from_millis(1),
        promote_ahead: Duration::from_secs(60),
        ..Default::default()
    });
    sqlx::query("DELETE FROM queue_scheduled WHERE queue = 'later'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM queue_bulk WHERE queue = 'later'")
        .execute(&pool)
        .await
        .unwrap();

    let job = brocanth_queue::NewJob {
        kind: "pg.later".into(),
        queue: "later".into(),
        payload: b"{}".to_vec(),
        run_at: Some(std::time::SystemTime::now() + Duration::from_secs(2)),
        max_attempts: 3,
        unique_key: None,
        uniqueness: brocanth_queue::UniquenessKind::None,
    };
    let pushed = q.push(vec![job]).await.unwrap();
    let Pushed::Enqueued(id) = &pushed[0] else {
        panic!("not enqueued");
    };
    assert!(
        id.as_str().starts_with('s'),
        "a long delay belongs in the scheduled table, not the claim index"
    );
    assert_eq!(q.approx_depth("later").await.unwrap(), 0);

    q.sweep().await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM queue_bulk WHERE queue = 'later'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1, "the promoter moved it into the queue");
    // It is in the queue but not yet due.
    assert_eq!(q.approx_depth("later").await.unwrap(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_job_whose_worker_dies_every_time_reaches_the_dead_letter() {
    let _serial = SERIAL.lock().await;
    let pool = db!();
    let q = PgQueue::bulk(pool.clone());
    sqlx::query("DELETE FROM queue_bulk WHERE queue = 'poison'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM queue_dead WHERE queue = 'poison'")
        .execute(&pool)
        .await
        .unwrap();

    let job = brocanth_queue::NewJob {
        kind: "pg.poison".into(),
        queue: "poison".into(),
        payload: b"{}".to_vec(),
        run_at: None,
        max_attempts: 2,
        unique_key: None,
        uniqueness: brocanth_queue::UniquenessKind::None,
    };
    q.push(vec![job]).await.unwrap();

    let req = brocanth_queue::ClaimRequest {
        queue: "poison".into(),
        max: 1,
        lease: Duration::from_millis(100),
        kinds: None,
        worker: "dies".into(),
    };
    // Claim twice and never settle: this is a worker that is killed mid-job.
    for _ in 0..2 {
        let got = q.claim(&req).await.unwrap();
        assert_eq!(got.len(), 1);
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    assert!(
        q.claim(&req).await.unwrap().is_empty(),
        "the attempts are spent, so the claim must refuse it"
    );
    q.sweep().await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM queue_dead WHERE queue = 'poison'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1, "the sweep moves it out rather than leaving it stuck");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_archive_partition_is_dropped_whole() {
    let _serial = SERIAL.lock().await;
    let pool = db!();
    brocanth_queue_postgres::ensure_archive_partitions(&pool, 1)
        .await
        .unwrap();
    let today = chrono::Utc::now().date_naive();
    let dropped = brocanth_queue_postgres::drop_archive_partitions_before(&pool, today)
        .await
        .unwrap();
    // Nothing older than today exists on a fresh database; the call must
    // still succeed and report honestly.
    assert!(dropped
        .iter()
        .all(|t| t < &format!("queue_archive_{}", today.format("%Y%m%d"))));
}

/// `migrate` creates today's partition and the next two days'. Nothing else
/// ever created one, so a process that stayed up longer than that reached a
/// day with nowhere to insert, and every archiving settle failed from then
/// on. A settle that does not land leaves the job to expire and run again,
/// so a retention detail turned into duplicate execution across the queue.
/// The sweep now carries the partitions forward.
#[tokio::test(flavor = "multi_thread")]
async fn the_sweep_carries_the_archive_partitions_forward() {
    let _serial = SERIAL.lock().await;
    let pool = db!();

    let today = chrono::Utc::now().date_naive();
    let horizon = today + chrono::Duration::days(2);
    let doomed = format!("queue_archive_{}", horizon.format("%Y%m%d"));
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP TABLE IF EXISTS {doomed}"
    )))
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        !partition_exists(&pool, &doomed).await,
        "the test has to start with the partition missing"
    );

    PgQueue::bulk(pool.clone()).sweep().await.unwrap();

    assert!(
        partition_exists(&pool, &doomed).await,
        "the sweep must create the partitions an archiving settle will need"
    );
}

/// A successful job leaves a row behind, because the archive is on by
/// default now. The question "did that job run?" cannot be answered
/// backwards, so the answer has to be kept before anybody asks.
#[tokio::test(flavor = "multi_thread")]
async fn a_finished_job_is_archived_by_default() {
    let _serial = SERIAL.lock().await;
    let pool = db!();
    let q = PgQueue::bulk(pool.clone());
    assert!(
        brocanth_queue_postgres::PgConfig::default().archive,
        "this test is about the default"
    );

    sqlx::query("DELETE FROM queue_bulk WHERE queue = 'kept'")
        .execute(&pool)
        .await
        .unwrap();
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM queue_archive WHERE queue = 'kept'")
        .fetch_one(&pool)
        .await
        .unwrap();

    q.push(vec![brocanth_queue::NewJob {
        kind: "t.kept".into(),
        queue: "kept".into(),
        payload: b"{}".to_vec(),
        run_at: None,
        max_attempts: 3,
        unique_key: None,
        uniqueness: brocanth_queue::UniquenessKind::None,
    }])
    .await
    .unwrap();

    let req = brocanth_queue::ClaimRequest {
        queue: "kept".into(),
        max: 1,
        lease: Duration::from_secs(30),
        kinds: None,
        worker: "test".into(),
    };
    let got = q.claim(&req).await.unwrap();
    assert_eq!(got.len(), 1);
    q.settle(&got[0].lease, brocanth_queue::Settle::Done)
        .await
        .unwrap();

    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM queue_archive WHERE queue = 'kept'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(after, before + 1, "the finished job has to leave a record");
    let live: i64 = sqlx::query_scalar("SELECT count(*) FROM queue_bulk WHERE queue = 'kept'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(live, 0, "and it still has to leave the live table");
}

async fn partition_exists(pool: &PgPool, name: &str) -> bool {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM pg_class c \
           JOIN pg_inherits i ON i.inhrelid = c.oid \
           JOIN pg_class p ON p.oid = i.inhparent \
          WHERE p.relname = 'queue_archive' AND c.relname = $1",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
        > 0
}

// ---------------------------------------------------------------------------
// The screens.
// ---------------------------------------------------------------------------

/// A queue name no other test in this file writes to.
const SEEN: &str = "pg.inspect";

fn seen(kind: &str) -> NewJob {
    NewJob {
        kind: kind.into(),
        queue: SEEN.into(),
        payload: b"{}".to_vec(),
        run_at: None,
        max_attempts: 3,
        unique_key: None,
        uniqueness: UniquenessKind::None,
    }
}

fn take_one(worker: &str) -> ClaimRequest {
    ClaimRequest {
        queue: SEEN.into(),
        max: 1,
        lease: Duration::from_secs(30),
        kinds: None,
        worker: worker.into(),
    }
}

/// Clear this queue out of every table it can be in.
async fn clear(pool: &PgPool) {
    for sql in [
        "DELETE FROM queue_bulk WHERE queue = $1",
        "DELETE FROM queue_scheduled WHERE queue = $1",
        "DELETE FROM queue_dead WHERE queue = $1",
    ] {
        sqlx::query(sql).bind(SEEN).execute(pool).await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_four_screens_partition_the_queue() {
    let _serial = SERIAL.lock().await;
    let pool = db!();
    clear(&pool).await;
    let q = PgQueue::bulk(pool.clone());

    // Ready, and one far enough out to land in `queue_scheduled`.
    q.push(vec![seen("a")]).await.unwrap();
    Client::new(Arc::new(PgQueue::bulk(pool.clone())));
    q.push(vec![NewJob {
        run_at: Some(std::time::SystemTime::now() + Duration::from_secs(3 * 3600)),
        ..seen("far")
    }])
    .await
    .unwrap();
    // Running.
    q.push(vec![seen("c")]).await.unwrap();
    let held = q.claim(&take_one("w1")).await.unwrap().pop().unwrap();
    // Retrying.
    q.push(vec![seen("d")]).await.unwrap();
    let failed = q.claim(&take_one("w1")).await.unwrap().pop().unwrap();
    q.settle(
        &failed.lease,
        Settle::Retry {
            at: std::time::SystemTime::now() + Duration::from_secs(600),
            failure: JobFailure::returned("nope"),
        },
    )
    .await
    .unwrap();

    let depth = |v: &Vec<brocanth_queue::QueueDepth>| {
        v.iter().find(|d| d.queue == SEEN).cloned().expect("queue")
    };
    let d = depth(&q.depths().await.unwrap());
    assert_eq!(
        (d.ready, d.running, d.retrying, d.scheduled),
        (1, 1, 1, 1),
        "{d:?}"
    );

    // The running row names the worker that took it. `ClaimRequest::worker`
    // was documented as "for the admin screens" and stored nowhere until
    // the column existed.
    let running: Vec<_> = q
        .list(View::Running, Page::default())
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.queue == SEEN)
        .collect();
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].claimed_by.as_deref(), Some("w1"));
    assert!(running[0].running_for().is_some());
    assert_eq!(running[0].id, held.lease.id);

    // The scheduled screen reads both tables and hides that it did.
    let scheduled: Vec<_> = q
        .list(View::Scheduled, Page::default())
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.queue == SEEN)
        .collect();
    assert_eq!(scheduled.len(), 1, "{scheduled:?}");
    assert!(
        scheduled[0].id.as_str().starts_with('s'),
        "a row from queue_scheduled should be tagged: {:?}",
        scheduled[0].id
    );

    // And the tag survives a round trip through `job`.
    let back = q.job(&scheduled[0].id).await.unwrap().expect("by id");
    assert_eq!(back.id, scheduled[0].id);
    assert_eq!(back.kind, "far");

    clear(&pool).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_job_a_worker_is_holding_cannot_be_moved_or_deleted() {
    let _serial = SERIAL.lock().await;
    let pool = db!();
    clear(&pool).await;
    let q = PgQueue::bulk(pool.clone());
    q.push(vec![seen("a")]).await.unwrap();
    let held = q.claim(&take_one("w1")).await.unwrap().pop().unwrap();

    assert!(!q.run_now(&held.lease.id).await.unwrap());
    assert!(!q.discard(&held.lease.id).await.unwrap());
    // The worker still owns it, which is what those two refusals protect.
    assert!(matches!(
        q.settle(&held.lease, Settle::Done).await.unwrap(),
        brocanth_queue::LeaseState::Held
    ));
    clear(&pool).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_scheduled_job_is_promoted_by_the_button_that_runs_it_now() {
    let _serial = SERIAL.lock().await;
    let pool = db!();
    clear(&pool).await;
    let q = PgQueue::bulk(pool.clone());
    q.push(vec![NewJob {
        run_at: Some(std::time::SystemTime::now() + Duration::from_secs(3 * 3600)),
        ..seen("far")
    }])
    .await
    .unwrap();

    let row = q
        .list(View::Scheduled, Page::default())
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.queue == SEEN)
        .expect("scheduled");
    assert!(q.run_now(&row.id).await.unwrap());

    // It left `queue_scheduled` and is claimable from the live table now.
    assert!(q.job(&row.id).await.unwrap().is_none());
    let claimed = q.claim(&take_one("w1")).await.unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].kind, "far");
    clear(&pool).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dead_job_keeps_what_a_requeue_needs() {
    let _serial = SERIAL.lock().await;
    let pool = db!();
    clear(&pool).await;
    let q = PgQueue::bulk(pool.clone());
    q.push(vec![NewJob {
        max_attempts: 7,
        ..seen("doomed")
    }])
    .await
    .unwrap();
    let c = q.claim(&take_one("w1")).await.unwrap().pop().unwrap();
    q.settle(
        &c.lease,
        Settle::Dead {
            failure: JobFailure::returned("gave up"),
        },
    )
    .await
    .unwrap();

    let dead: Vec<_> = q
        .dead(Page::default())
        .await
        .unwrap()
        .into_iter()
        .filter(|d| d.queue == SEEN)
        .collect();
    assert_eq!(dead.len(), 1);
    // Without these two the row describes a job that cannot be rebuilt.
    assert_eq!(dead[0].max_attempts, Some(7));
    assert_eq!(dead[0].job_id.as_ref(), Some(&c.lease.id));
    assert!(dead[0].failure.contains("gave up"));

    let new = q.requeue(&dead[0].id).await.unwrap().expect("requeued");
    assert_ne!(new, c.lease.id, "a requeue is a new job, not the old one");
    // Pressing it twice finds nothing, rather than making a second copy.
    assert!(q.requeue(&dead[0].id).await.unwrap().is_none());

    let back = q.job(&new).await.unwrap().expect("the requeued job");
    assert_eq!(back.attempt, 0, "a requeue gets a fresh budget");
    assert_eq!(back.max_attempts, 7);
    clear(&pool).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_archive_answers_how_much_finished() {
    let _serial = SERIAL.lock().await;
    let pool = db!();
    clear(&pool).await;
    let q = PgQueue::bulk(pool.clone());
    assert!(q.has_history(), "the archive is on by default");

    let before: u64 = q
        .finished(
            std::time::SystemTime::now() - Duration::from_secs(3600),
            Bucket::Hour,
        )
        .await
        .unwrap()
        .iter()
        .filter(|f| f.queue == SEEN)
        .map(|f| f.count)
        .sum();

    q.push(vec![seen("done"), seen("done")]).await.unwrap();
    for _ in 0..2 {
        let c = q.claim(&take_one("w1")).await.unwrap().pop().unwrap();
        q.settle(&c.lease, Settle::Done).await.unwrap();
    }

    let after: u64 = q
        .finished(
            std::time::SystemTime::now() - Duration::from_secs(3600),
            Bucket::Hour,
        )
        .await
        .unwrap()
        .iter()
        .filter(|f| f.queue == SEEN)
        .map(|f| f.count)
        .sum();
    assert_eq!(after - before, 2, "the archive is where the history is");
    clear(&pool).await;
}
