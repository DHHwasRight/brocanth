//! The queue's tab, rendered through the real admin.
//!
//! These assert on HTML and not on a context struct, because a screen that
//! compiles and draws the wrong thing is the failure worth catching. The
//! admin is real, the policy is real, and the queue is a real SQLite file
//! with real rows in it.

#![cfg(all(feature = "admin", feature = "sqlite"))]

mod common;

use std::sync::Arc;
use std::time::Duration;

use brocanth_admin::policy::{policy_fn, Action, Principal, Request};
use brocanth_admin::rt::block_on;
use brocanth_admin::{Admin, AdminConfig, Discovery, Fixture, Submission};
use brocanth_jobs::admin::JobsPanel;
use brocanth_jobs::{Queue, Worker};
use common::{log, sqlite_queue, token, wait_for, AlwaysFails, Echo, Slow};

struct Operator;

/// An admin with the queue's tab on it. `act` decides whether this person
/// may press the buttons.
fn admin(queue: &Queue, act: bool) -> Admin {
    Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .panel(JobsPanel::new(queue.clone()))
            .with_policy(policy_fn(move |_: &Principal<'_>, r: Request<'_>| {
                act || r.action != Action::ActOnPanel
            })),
    )
}

fn get(admin: &Admin, path: &str) -> String {
    let who = Operator;
    let response =
        block_on(admin.dispatch(Principal::new(&who), "GET", path, "", &Submission::new()))
            .expect("dispatch");
    assert_eq!(response.status(), 200, "{path}");
    response.html().expect("a page").to_string()
}

fn post(admin: &Admin, path: &str) -> u16 {
    let who = Operator;
    block_on(admin.dispatch(Principal::new(&who), "POST", path, "", &Submission::new()))
        .expect("dispatch")
        .status()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_dashboard_counts_what_is_in_the_queue() {
    let (queue, _dir) = sqlite_queue().await;
    let t = token("panel-dash");
    queue
        .enqueue(Echo {
            token: t.clone(),
            n: 1,
        })
        .await
        .expect("enqueued");
    queue
        .enqueue(Echo {
            token: t.clone(),
            n: 2,
        })
        .in_(Duration::from_secs(3600))
        .await
        .expect("scheduled");

    let html = get(&admin(&queue, true), "/admin/jobs");
    assert!(html.contains("Background jobs"), "the tab's own label");
    assert!(html.contains("Enqueued"), "{html}");
    assert!(html.contains("Scheduled"), "{html}");
    // The queue table, with the default queue in it. Asserting on the
    // text and not on a cell's classes: a restyle must not fail a test
    // about what is in the queue.
    assert!(html.contains(">default<"), "{html}");
    // And the number that actually says a queue is in trouble.
    assert!(html.contains("Oldest runnable job"), "{html}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retries_and_scheduled_are_two_screens_of_one_table() {
    let (queue, _dir) = sqlite_queue().await;
    let t = token("panel-waiting");

    queue
        .enqueue(Echo {
            token: t.clone(),
            n: 1,
        })
        .in_(Duration::from_secs(3600))
        .await
        .expect("scheduled");
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

    let a = admin(&queue, true);

    let retries = get(&a, "/admin/jobs/retries");
    assert!(retries.contains("always_fails"), "{retries}");
    assert!(
        !retries.contains(">echo<"),
        "a scheduled job is not a retry"
    );
    // A retry says why, which is the column the screen exists for, and
    // the words are the job's own.
    assert!(retries.contains("nope, attempt"), "{retries}");

    let scheduled = get(&a, "/admin/jobs/scheduled");
    assert!(scheduled.contains("echo"), "{scheduled}");
    assert!(
        !scheduled.contains("always_fails"),
        "a retry is not a scheduled job"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn busy_names_the_worker_and_the_screen_shows_it() {
    let (queue, _dir) = sqlite_queue().await;
    let t = token("panel-busy");
    queue
        .enqueue(Slow {
            token: t.clone(),
            millis: 2_000,
        })
        .await
        .expect("enqueued");
    let worker = Worker::builder(&queue)
        .id("worker-on-screen")
        .concurrency(1)
        .poll_interval(Duration::from_millis(20))
        .register::<Slow>()
        .start()
        .expect("started");
    wait_for("the claim", Duration::from_secs(5), || {
        let queue = queue.clone();
        async move { !queue.busy(10, 0).await.expect("busy").is_empty() }
    })
    .await;

    let html = get(&admin(&queue, true), "/admin/jobs/busy");
    assert!(html.contains("worker-on-screen"), "{html}");
    assert!(html.contains("slow"), "{html}");
    worker.shutdown(Duration::from_secs(5)).await.expect("stop");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_button_changes_the_queue_and_redirects() {
    let (queue, _dir) = sqlite_queue().await;
    let t = token("panel-button");
    let id = queue
        .enqueue(Echo {
            token: t.clone(),
            n: 1,
        })
        .in_(Duration::from_secs(3600))
        .await
        .expect("scheduled")
        .id();

    let a = admin(&queue, true);
    assert_eq!(queue.scheduled(10, 0).await.expect("scheduled").len(), 1);

    let status = post(&a, &format!("/admin/jobs/scheduled/{}/run", id.0));
    assert_eq!(
        status, 303,
        "a redirect, so a reload does not press it twice"
    );
    assert!(
        queue.scheduled(10, 0).await.expect("scheduled").is_empty(),
        "the job is no longer waiting for its time"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reader_who_may_not_act_is_not_shown_the_buttons() {
    // The whole reason reading and acting are two policy questions. An
    // on-call engineer who may look at the dead letter must not be one
    // click from emptying it.
    let (queue, _dir) = sqlite_queue().await;
    let t = token("panel-readonly");
    queue
        .enqueue(Echo {
            token: t.clone(),
            n: 1,
        })
        .in_(Duration::from_secs(3600))
        .await
        .expect("scheduled");

    let readonly = admin(&queue, false);
    let html = get(&readonly, "/admin/jobs/scheduled");
    assert!(html.contains("echo"), "the row is there: {html}");
    assert!(!html.contains("Run now"), "and the button is not: {html}");

    // And pressing it anyway is refused rather than merely hidden.
    let id = queue.scheduled(10, 0).await.expect("scheduled")[0].id;
    assert_eq!(
        post(&readonly, &format!("/admin/jobs/scheduled/{id}/run")),
        403
    );
    assert_eq!(
        queue.scheduled(10, 0).await.expect("scheduled").len(),
        1,
        "and nothing happened"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_tab_renders() {
    let (queue, _dir) = sqlite_queue().await;
    let a = admin(&queue, true);
    for path in ["", "/queues", "/busy", "/retries", "/scheduled", "/dead"] {
        let html = get(&a, &format!("/admin/jobs{path}"));
        assert!(
            html.contains(r#"aria-label="Queue""#),
            "{path} has the tab strip"
        );
    }
    // And a path the panel does not serve is the admin's 404.
    let who = Operator;
    let response = block_on(a.dispatch(
        Principal::new(&who),
        "GET",
        "/admin/jobs/nope",
        "",
        &Submission::new(),
    ))
    .expect("dispatch");
    assert_eq!(response.status(), 404);
}

/// Prints the dashboard, for a human who wants to see it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "prints a page, asserts nothing"]
async fn show() {
    let (queue, _dir) = sqlite_queue().await;
    let t = token("show");
    for n in 0..3 {
        queue
            .enqueue(Echo {
                token: t.clone(),
                n,
            })
            .await
            .expect("enqueued");
    }
    queue
        .enqueue(AlwaysFails { token: t.clone() })
        .await
        .expect("enqueued");
    let worker = Worker::builder(&queue)
        .concurrency(1)
        .poll_interval(Duration::from_millis(20))
        .register::<AlwaysFails>()
        .start()
        .expect("started");
    wait_for("a failure", Duration::from_secs(5), || async {
        !log(&t).is_empty()
    })
    .await;
    worker.shutdown(Duration::from_secs(5)).await.expect("stop");

    let a = admin(&queue, true);
    for path in ["", "/retries"] {
        println!("\n===== /admin/jobs{path} =====");
        println!("{}", get(&a, &format!("/admin/jobs{path}")));
    }
}
