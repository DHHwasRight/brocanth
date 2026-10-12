//! What a recurring job does, and in particular what it does when the workers
//! were down across its window.
//!
//! These run on a [`ManualClock`] and call [`Queue::tick_schedules`] directly,
//! so "the worker was down for three hours" is three hours of clock and no
//! waiting. Downtime behaviour that can only be tested by taking a service down
//! does not get tested.

mod common;

use std::time::Duration;

use brocanth_jobs::{ManualClock, Misfire};
use chrono::{TimeZone, Utc};
use common::*;

/// Noon on a Wednesday, so that nothing here depends on today's date.
fn noon() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, 6, 12, 0, 0).unwrap()
}

#[tokio::test]
async fn an_hourly_schedule_fires_once_when_its_hour_arrives() {
    let (queue, _dir) = sqlite_queue().await;
    let clock = ManualClock::new(noon());
    let queue = queue.with_clock(clock.clone());
    let t = token("hourly");

    queue
        .recurring("hourly", "0 * * * *", Nightly { token: t.clone() })
        .install()
        .await
        .expect("install");

    // Nothing is due at noon: the next occurrence is 13:00.
    assert_eq!(queue.tick_schedules().await.expect("tick"), 0);
    assert_eq!(queue.stats().await.expect("stats").ready, 0);

    // 13:00:10 — ten seconds late, well inside the sixty-second grace.
    clock.set(noon() + chrono::Duration::seconds(3_610));
    assert_eq!(queue.tick_schedules().await.expect("tick"), 1);
    assert_eq!(queue.stats().await.expect("stats").ready, 1);

    // Ticking again changes nothing: the schedule has moved on to 14:00.
    assert_eq!(queue.tick_schedules().await.expect("tick"), 0);
    assert_eq!(queue.stats().await.expect("stats").ready, 1);
}

#[tokio::test]
async fn after_downtime_the_default_skips_the_window_it_slept_through() {
    let (queue, _dir) = sqlite_queue().await;
    let clock = ManualClock::new(noon());
    let queue = queue.with_clock(clock.clone());
    let t = token("skip");

    queue
        .recurring("hourly-skip", "0 * * * *", Nightly { token: t.clone() })
        // The default, written out because it is the decision being tested.
        .misfire(Misfire::Skip)
        .install()
        .await
        .expect("install");

    // The process was down from noon until 15:10. Three occurrences — 13:00,
    // 14:00 and 15:00 — came and went, and the most recent is ten minutes old,
    // which is outside the grace period.
    clock.set(noon() + chrono::Duration::minutes(190));

    assert_eq!(
        queue.tick_schedules().await.expect("tick"),
        0,
        "nothing fires: three missed hours do not become three jobs, or even one"
    );
    assert_eq!(queue.stats().await.expect("stats").total(), 0);

    // And the schedule has caught itself up to the next future occurrence
    // rather than staying stuck in the past.
    clock.set(noon() + chrono::Duration::minutes(240) + chrono::Duration::seconds(5));
    assert_eq!(
        queue.tick_schedules().await.expect("tick"),
        1,
        "the next occurrence after the downtime fires normally"
    );
}

#[tokio::test]
async fn fire_once_runs_the_most_recent_missed_occurrence_and_only_that_one() {
    let (queue, _dir) = sqlite_queue().await;
    let clock = ManualClock::new(noon());
    let queue = queue.with_clock(clock.clone());
    let t = token("fire-once");

    queue
        .recurring("hourly-once", "0 * * * *", Nightly { token: t.clone() })
        .misfire(Misfire::FireOnce)
        .install()
        .await
        .expect("install");

    // The same three-hour outage.
    clock.set(noon() + chrono::Duration::minutes(190));

    assert_eq!(
        queue.tick_schedules().await.expect("tick"),
        1,
        "exactly one job, not three"
    );
    assert_eq!(queue.stats().await.expect("stats").ready, 1);

    // Still one: the backlog was collapsed, not deferred.
    assert_eq!(queue.tick_schedules().await.expect("tick"), 0);
    assert_eq!(queue.stats().await.expect("stats").ready, 1);
}

#[tokio::test]
async fn two_workers_ticking_at_once_enqueue_one_job() {
    let (queue, _dir) = sqlite_queue().await;
    let clock = ManualClock::new(noon());
    let queue = queue.with_clock(clock.clone());
    let t = token("one-tick");

    queue
        .recurring("shared", "0 * * * *", Nightly { token: t.clone() })
        .install()
        .await
        .expect("install");
    clock.set(noon() + chrono::Duration::seconds(3_605));

    // Two handles on the same database, the way two worker processes would be.
    let other = queue.clone();
    let (a, b) = tokio::join!(queue.tick_schedules(), other.tick_schedules());
    let fired = a.expect("tick a") + b.expect("tick b");

    assert_eq!(
        fired, 1,
        "the occurrence's unique key folded the second insert"
    );
    assert_eq!(queue.stats().await.expect("stats").ready, 1);
}

#[tokio::test]
async fn reinstalling_an_unchanged_schedule_does_not_push_it_back() {
    let (queue, _dir) = sqlite_queue().await;
    let clock = ManualClock::new(noon());
    let queue = queue.with_clock(clock.clone());
    let t = token("reinstall");

    queue
        .recurring("stable", "0 * * * *", Nightly { token: t.clone() })
        .install()
        .await
        .expect("install");

    // A restart at 12:59 re-installs the same schedule.
    clock.set(noon() + chrono::Duration::minutes(59));
    queue
        .recurring("stable", "0 * * * *", Nightly { token: t.clone() })
        .install()
        .await
        .expect("reinstall");

    // 13:00 still fires. If re-installing had recomputed the next occurrence
    // from the restart time, this would have slipped to 14:00 — which is how a
    // frequently restarted application silently stops running its schedules.
    clock.set(noon() + chrono::Duration::seconds(3_605));
    assert_eq!(queue.tick_schedules().await.expect("tick"), 1);
}

#[tokio::test]
async fn changing_the_expression_does_recompute_the_next_occurrence() {
    let (queue, _dir) = sqlite_queue().await;
    let clock = ManualClock::new(noon());
    let queue = queue.with_clock(clock.clone());
    let t = token("changed");

    queue
        .recurring("movable", "0 * * * *", Nightly { token: t.clone() })
        .install()
        .await
        .expect("install");
    queue
        .recurring("movable", "*/5 * * * *", Nightly { token: t.clone() })
        .install()
        .await
        .expect("reinstall");

    // Every five minutes now, so 12:05 rather than 13:00.
    clock.set(noon() + chrono::Duration::seconds(305));
    assert_eq!(queue.tick_schedules().await.expect("tick"), 1);
}

#[tokio::test]
async fn a_schedule_can_be_removed() {
    let (queue, _dir) = sqlite_queue().await;
    let clock = ManualClock::new(noon());
    let queue = queue.with_clock(clock.clone());
    let t = token("removable");

    queue
        .recurring("temporary", "0 * * * *", Nightly { token: t })
        .install()
        .await
        .expect("install");
    assert!(queue.remove_recurring("temporary").await.expect("remove"));
    assert!(!queue.remove_recurring("temporary").await.expect("remove"));

    clock.set(noon() + chrono::Duration::seconds(3_605));
    assert_eq!(queue.tick_schedules().await.expect("tick"), 0);
}

#[tokio::test]
async fn a_bad_cron_expression_is_refused_at_install_rather_than_at_three_in_the_morning() {
    let (queue, _dir) = sqlite_queue().await;
    let t = token("bad-cron");

    let error = queue
        .recurring("broken", "0 25 * * *", Nightly { token: t })
        .install()
        .await
        .expect_err("25 is not an hour");
    assert!(error.to_string().contains("outside 0-23"), "{error}");
}

#[tokio::test]
async fn a_recurring_job_runs_on_a_worker_like_any_other() {
    let (queue, _dir) = sqlite_queue().await;
    let clock = ManualClock::new(noon());
    let queue = queue.with_clock(clock.clone());
    let t = token("recurring-runs");

    queue
        .recurring("runs", "0 * * * *", Nightly { token: t.clone() })
        .install()
        .await
        .expect("install");
    clock.set(noon() + chrono::Duration::seconds(3_605));
    assert_eq!(queue.tick_schedules().await.expect("tick"), 1);

    let worker = brocanth_jobs::Worker::builder(&queue)
        .poll_interval(Duration::from_millis(10))
        .register::<Nightly>()
        .start()
        .expect("start");
    wait_for(
        "the recurring job to run",
        Duration::from_secs(5),
        || async { log(&t) == vec!["nightly ran"] },
    )
    .await;
    worker
        .shutdown(Duration::from_secs(2))
        .await
        .expect("shutdown");
}
