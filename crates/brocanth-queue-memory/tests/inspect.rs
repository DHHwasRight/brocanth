//! The screens, against the reference backend.
//!
//! These are the cases the Postgres adapter is held to as well. The two
//! compute their states from different columns, so a case that passes here
//! and fails there is the point of having both.

use std::time::{Duration, SystemTime};

// `Inspect` and not `DynInspect`: both carry the same method names and a
// concrete adapter has both, so importing the pair is ambiguous. A test
// knows its backend, so it takes the ergonomic one.
use brocanth_queue::{
    Backend, ClaimRequest, DeadId, Inspect, JobFailure, JobId, NewJob, Page, Settle, Stats,
    UniquenessKind, View,
};
use brocanth_queue_memory::Memory;

fn job(kind: &str) -> NewJob {
    NewJob {
        kind: kind.into(),
        queue: "q".into(),
        payload: b"{}".to_vec(),
        run_at: None,
        max_attempts: 3,
        unique_key: None,
        uniqueness: UniquenessKind::None,
    }
}

fn later(secs: u64) -> Option<SystemTime> {
    Some(SystemTime::now() + Duration::from_secs(secs))
}

fn claim(worker: &str) -> ClaimRequest {
    ClaimRequest {
        queue: "q".into(),
        max: 1,
        lease: Duration::from_secs(30),
        kinds: None,
        worker: worker.into(),
    }
}

/// Every live row is on exactly one screen, and the counts add up.
#[tokio::test]
async fn the_four_screens_partition_the_queue() {
    let m = Memory::new();
    // One ready, one scheduled.
    m.push(vec![
        job("a"),
        NewJob {
            run_at: later(600),
            ..job("b")
        },
    ])
    .await
    .unwrap();
    // A third, claimed, so it is running.
    m.push(vec![job("c")]).await.unwrap();
    let held = m.claim(&claim("w1")).await.unwrap().pop().unwrap();
    // A fourth, failed once, so it is retrying.
    m.push(vec![job("d")]).await.unwrap();
    let failed = m.claim(&claim("w1")).await.unwrap().pop().unwrap();
    m.settle(
        &failed.lease,
        Settle::Retry {
            at: SystemTime::now() + Duration::from_secs(60),
            failure: JobFailure::returned("nope"),
        },
    )
    .await
    .unwrap();

    let s = m.stats().await.unwrap();
    assert_eq!(
        (s.ready, s.running, s.retrying, s.scheduled, s.dead),
        (1, 1, 1, 1, 0),
        "{s:?}"
    );
    assert_eq!(s.ready + s.running + s.retrying + s.scheduled, 4);

    for (view, n) in [
        (View::Ready, 1),
        (View::Running, 1),
        (View::Retrying, 1),
        (View::Scheduled, 1),
    ] {
        let rows = m.list(view, Page::default()).await.unwrap();
        assert_eq!(rows.len(), n, "{view} returned {rows:?}");
    }

    // The running row names the worker that holds it. That column is the
    // whole worker roster: there is no second table to disagree with it.
    let running = m.list(View::Running, Page::default()).await.unwrap();
    assert_eq!(running[0].claimed_by.as_deref(), Some("w1"));
    assert!(running[0].running_for().is_some());
    assert_eq!(running[0].id, held.lease.id);

    // The retrying row carries why.
    let retrying = m.list(View::Retrying, Page::default()).await.unwrap();
    assert!(retrying[0].last_error.is_some(), "{:?}", retrying[0]);

    let depths = m.depths().await.unwrap();
    assert_eq!(depths.len(), 1);
    assert_eq!(depths[0].queue, "q");
    assert_eq!(depths[0].ready, 1);
}

/// The button that would cause the thing the lease prevents.
#[tokio::test]
async fn a_job_a_worker_is_holding_cannot_be_moved_or_deleted() {
    let m = Memory::new();
    m.push(vec![job("a")]).await.unwrap();
    let held = m.claim(&claim("w1")).await.unwrap().pop().unwrap();
    let id = held.lease.id.clone();

    assert!(!m.run_now(&id).await.unwrap(), "a held job was moved");
    assert!(!m.discard(&id).await.unwrap(), "a held job was deleted");
    // And the worker still owns it: nothing above cleared the token.
    assert_eq!(m.stats().await.unwrap().running, 1);
    assert!(matches!(
        m.settle(&held.lease, Settle::Done).await.unwrap(),
        brocanth_queue::LeaseState::Held
    ));
}

/// An expired lease is not a held one. Pushing an orphan along is the whole
/// reason the button exists.
#[tokio::test]
async fn a_job_whose_lease_expired_can_be_moved() {
    let m = Memory::new();
    m.push(vec![job("a")]).await.unwrap();
    let held = m
        .claim(&ClaimRequest {
            lease: Duration::from_millis(20),
            ..claim("w1")
        })
        .await
        .unwrap()
        .pop()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(m.run_now(&held.lease.id).await.unwrap());
    let s = m.stats().await.unwrap();
    assert_eq!((s.ready, s.running), (1, 0), "{s:?}");
}

/// A scheduled job pulled forward, and one deleted before it ever runs.
#[tokio::test]
async fn a_scheduled_job_can_be_pulled_forward_or_dropped() {
    let m = Memory::new();
    m.push(vec![
        NewJob {
            run_at: later(3600),
            ..job("a")
        },
        NewJob {
            run_at: later(3600),
            ..job("b")
        },
    ])
    .await
    .unwrap();
    let rows = m.list(View::Scheduled, Page::default()).await.unwrap();
    assert_eq!(rows.len(), 2);

    assert!(m.run_now(&rows[0].id).await.unwrap());
    assert!(m.discard(&rows[1].id).await.unwrap());
    let s = m.stats().await.unwrap();
    assert_eq!((s.ready, s.scheduled), (1, 0), "{s:?}");

    // A pasted id that names nothing changes nothing, and does not error.
    assert!(!m.run_now(&JobId("nonsense".into())).await.unwrap());
    assert!(!m.discard(&JobId("9999".into())).await.unwrap());
}

/// The dead letter, and putting a row back.
#[tokio::test]
async fn a_dead_job_can_be_read_requeued_and_forgotten() {
    let m = Memory::new();
    m.push(vec![job("a"), job("b")]).await.unwrap();
    for _ in 0..2 {
        let c = m.claim(&claim("w1")).await.unwrap().pop().unwrap();
        m.settle(
            &c.lease,
            Settle::Dead {
                failure: JobFailure::returned("gave up"),
            },
        )
        .await
        .unwrap();
    }

    let dead = m.dead(Page::default()).await.unwrap();
    assert_eq!(dead.len(), 2);
    // Most recently dead first.
    assert!(dead[0].died_at >= dead[1].died_at);
    // Everything a requeue needs is on the row.
    assert_eq!(dead[0].max_attempts, Some(3));
    assert!(dead[0].job_id.is_some());
    assert_eq!(dead[0].payload, b"{}".to_vec());
    assert!(dead[0].failure.contains("gave up"));

    let one = m.dead_one(&dead[0].id).await.unwrap().expect("by id");
    assert_eq!(one.id, dead[0].id);

    // A requeue is a new job, not the old one resurrected. That is what
    // makes a second accidental press do nothing.
    let new = m.requeue(&dead[0].id).await.unwrap().expect("requeued");
    assert_ne!(Some(&new), dead[0].job_id.as_ref());
    assert!(m.requeue(&dead[0].id).await.unwrap().is_none());
    assert_eq!(m.stats().await.unwrap().ready, 1);

    assert!(m.forget(&dead[1].id).await.unwrap());
    assert!(!m.forget(&dead[1].id).await.unwrap());
    assert_eq!(m.stats().await.unwrap().dead, 0);
    assert!(m
        .dead_one(&DeadId("nonsense".into()))
        .await
        .unwrap()
        .is_none());
}

/// A requeue that would collide keeps the corpse.
#[tokio::test]
async fn a_requeue_blocked_by_a_live_key_does_not_lose_the_record() {
    let m = Memory::new();
    let unique = NewJob {
        unique_key: Some("k".into()),
        uniqueness: UniquenessKind::UntilSuccess,
        ..job("a")
    };
    m.push(vec![unique.clone()]).await.unwrap();
    let c = m.claim(&claim("w1")).await.unwrap().pop().unwrap();
    m.settle(
        &c.lease,
        Settle::Dead {
            failure: JobFailure::returned("died holding k"),
        },
    )
    .await
    .unwrap();

    // Something else takes the key before anyone presses retry.
    m.push(vec![unique]).await.unwrap();
    let dead = m.dead(Page::default()).await.unwrap();
    assert!(m.requeue(&dead[0].id).await.unwrap().is_none());
    assert_eq!(
        m.stats().await.unwrap().dead,
        1,
        "the only record of why it died was thrown away"
    );
}

/// An empty queue answers, rather than failing.
#[tokio::test]
async fn an_empty_queue_has_screens_too() {
    let m = Memory::new();
    assert_eq!(m.stats().await.unwrap(), Stats::default());
    assert!(m.depths().await.unwrap().is_empty());
    for view in View::ALL {
        assert!(m.list(view, Page::default()).await.unwrap().is_empty());
    }
    assert!(m.dead(Page::default()).await.unwrap().is_empty());
    // No archive, so no history, and it says so rather than drawing zero.
    assert!(!m.has_history());
    assert!(m
        .finished(SystemTime::now(), brocanth_queue::Bucket::Hour)
        .await
        .is_err());
}

/// Paging walks the list without repeating or skipping a row.
#[tokio::test]
async fn paging_covers_the_list_exactly_once() {
    let m = Memory::new();
    m.push((0..10).map(|_| job("a")).collect()).await.unwrap();
    let mut seen = Vec::new();
    for page in 0..4u64 {
        for row in m.list(View::Ready, Page::at(page, 3)).await.unwrap() {
            seen.push(row.id);
        }
    }
    assert_eq!(seen.len(), 10);
    let mut sorted = seen.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), 10, "a row was listed twice");
}
