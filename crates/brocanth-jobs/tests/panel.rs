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
            .panel(JobsPanel::new(Arc::new(queue.clone())))
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
async fn a_button_is_a_dp_post_that_removes_its_row() {
    let (queue, _dir) = sqlite_queue().await;
    let t = token("panel-dp-post");
    let id = queue
        .enqueue(Echo {
            token: t.clone(),
            n: 1,
        })
        .in_(Duration::from_secs(3600))
        .await
        .expect("scheduled")
        .id();

    let html = get(&admin(&queue, true), "/admin/jobs/scheduled");
    // The form keeps its `action`, so it posts with no script; `dp-post`
    // with no value posts to that same action.
    let run = format!(r#"action="/admin/jobs/scheduled/{}/run""#, id.0);
    let at = html.find(&run).unwrap_or_else(|| panic!("{run} in {html}"));
    let form = &html[at..at + html[at..].find('>').expect("the tag closes")];
    assert!(form.contains("dp-post"), "{form}");
    assert!(form.contains(r#"dp-target="closest tr""#), "{form}");
    assert!(form.contains(r#"dp-swap="delete""#), "{form}");
    assert!(html.contains("dpmutate"), "the script is on the page");

    // And a reader who may not act gets neither the forms nor a reason to
    // fetch anything.
    let readonly = get(&admin(&queue, false), "/admin/jobs/scheduled");
    assert!(
        !readonly.contains(r#"dp-target="closest tr""#),
        "{readonly}"
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

// ---------------------------------------------------------------------------
// The same six screens, over a queue this crate knows nothing about.
// ---------------------------------------------------------------------------

/// The panel holds `Arc<dyn DynInspect>` and nothing else, so anything that
/// implements the contract draws the same screens. This is that claim as a
/// test: the backend is `brocanth-queue`'s in-process one, which shares no
/// code, no schema and no id type with the SQLite queue above.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_same_screens_render_over_another_queue_entirely() {
    use brocanth_queue::{Backend, ClaimRequest, JobFailure, NewJob, Settle, UniquenessKind};
    use brocanth_queue_memory::Memory;

    let store = Arc::new(Memory::new());
    let job = |kind: &str, run_at| NewJob {
        kind: kind.into(),
        queue: "mail".into(),
        payload: b"{}".to_vec(),
        run_at,
        max_attempts: 3,
        unique_key: None,
        uniqueness: UniquenessKind::None,
    };
    let later = Some(std::time::SystemTime::now() + Duration::from_secs(3600));
    store
        .push(vec![job("send_welcome", None), job("digest", later)])
        .await
        .expect("pushed");

    // One claimed, so the Busy screen has a worker to name, and one dead.
    let claim = ClaimRequest {
        queue: "mail".into(),
        max: 1,
        lease: Duration::from_secs(30),
        kinds: None,
        worker: "worker-7".into(),
    };
    let held = store
        .claim(&claim)
        .await
        .expect("claimed")
        .pop()
        .expect("one");
    store
        .push(vec![job("charge_card", None)])
        .await
        .expect("pushed");
    let doomed = store
        .claim(&claim)
        .await
        .expect("claimed")
        .pop()
        .expect("one");
    store
        .settle(
            &doomed.lease,
            Settle::Dead {
                failure: JobFailure::returned("the card was declined"),
            },
        )
        .await
        .expect("settled");

    let a = Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .panel(JobsPanel::new(store.clone()))
            .with_policy(policy_fn(|_: &Principal<'_>, _: Request<'_>| true)),
    );

    let dash = get(&a, "/admin/jobs");
    assert!(dash.contains("Background jobs"), "{dash}");
    assert!(
        dash.contains(">mail<"),
        "the queue name, from the other queue"
    );

    let busy = get(&a, "/admin/jobs/busy");
    assert!(busy.contains("send_welcome"), "{busy}");
    // `ClaimRequest::worker` was documented as "for the admin screens" and
    // stored nowhere until the column existed. This is the screen.
    assert!(busy.contains("worker-7"), "{busy}");

    let scheduled = get(&a, "/admin/jobs/scheduled");
    assert!(scheduled.contains("digest"), "{scheduled}");

    let dead = get(&a, "/admin/jobs/dead");
    assert!(dead.contains("charge_card"), "{dead}");
    assert!(dead.contains("the card was declined"), "{dead}");
    // The retry button posts in the background and takes its row away,
    // and still posts as a plain form to the same place with no script.
    assert!(
        dead.contains(r#"dp-post dp-target="closest tr" dp-swap="delete""#),
        "{dead}"
    );
    assert!(dead.contains("/admin/jobs/dead/"), "{dead}");
    assert!(dead.contains("dpmutate"), "the script is on the page");

    // And a button, which is where the id types matter: this queue numbers
    // its dead letter for itself, and the panel passes the id straight
    // through rather than parsing it into something of its own.
    let id = brocanth_queue::Inspect::dead(&*store, brocanth_queue::Page::default())
        .await
        .expect("dead")
        .remove(0)
        .id;
    assert_eq!(post(&a, &format!("/admin/jobs/dead/{id}/retry")), 303);
    assert_eq!(
        brocanth_queue::Inspect::stats(&*store).await.unwrap().dead,
        0,
        "the retry button put it back"
    );

    // The job the first worker still holds must not be movable from here.
    let _ = held;
}

/// A queue that keeps nothing has no History tab, and no History screen.
///
/// Drawing a chart of zeroes would read as an outage. The tab is left off,
/// and a pasted URL is refused rather than rendered, because the second is
/// how somebody gets to the first screen they were not meant to see.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_queue_that_keeps_no_history_offers_no_chart() {
    use brocanth_queue_memory::Memory;

    let store = Arc::new(Memory::new());
    assert!(!brocanth_queue::Inspect::has_history(&*store));

    let a = Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .panel(JobsPanel::new(store))
            .with_policy(policy_fn(|_: &Principal<'_>, _: Request<'_>| true)),
    );

    let dash = get(&a, "/admin/jobs");
    assert!(!dash.contains(">History<"), "the tab should not be there");

    let who = Operator;
    let response = block_on(a.dispatch(
        Principal::new(&who),
        "GET",
        "/admin/jobs/history",
        "",
        &Submission::new(),
    ))
    .expect("dispatch");
    assert_eq!(response.status(), 404, "a pasted URL drew the chart anyway");
}

/// The chart, over the one backend that keeps an archive.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_history_screen_plots_what_the_archive_kept() {
    use brocanth_queue::{Backend, ClaimRequest, NewJob, Settle, UniquenessKind};
    use brocanth_queue_postgres::{migrate, PgQueue};
    use sqlx::postgres::PgPoolOptions;

    let Ok(url) = std::env::var("BROCANTH_TEST_DATABASE_URL") else {
        eprintln!("!! SKIPPED the_history_screen_plots_what_the_archive_kept: no Postgres server");
        return;
    };
    // A schema of its own, and not a queue name of its own. The chart is
    // about the queue as a whole -- an operator asks "how much ran", not
    // "how much ran on `mail`" -- so every row in the archive is a bar
    // here, including rows another suite left in the same database. The
    // archive is append only as well, so a second run would plot the
    // first's jobs. Both problems go away with somewhere private to put
    // the tables.
    let schema = format!("panel_history_{}", std::process::id());
    let admin_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("connect");
    for sql in [
        format!("DROP SCHEMA IF EXISTS {schema} CASCADE"),
        format!("CREATE SCHEMA {schema}"),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .execute(&admin_pool)
            .await
            .expect("schema");
    }
    let search_path = format!("SET search_path TO {schema}");
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |conn, _| {
            let sql = search_path.clone();
            Box::pin(async move {
                sqlx::query(sqlx::AssertSqlSafe(sql)).execute(conn).await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .expect("connect");
    migrate(&pool).await.expect("migrate");

    let store = Arc::new(PgQueue::bulk(pool.clone()));
    assert!(
        brocanth_queue::Inspect::has_history(&*store),
        "the archive is on by default"
    );

    // Three jobs through to the archive, two of one kind and one of another.
    for (kind, n) in [("render_invoice", 2), ("send_digest", 1)] {
        for _ in 0..n {
            store
                .push(vec![NewJob {
                    kind: kind.into(),
                    queue: "panel.history".into(),
                    payload: b"{}".to_vec(),
                    run_at: None,
                    max_attempts: 3,
                    unique_key: None,
                    uniqueness: UniquenessKind::None,
                }])
                .await
                .expect("pushed");
            let c = store
                .claim(&ClaimRequest {
                    queue: "panel.history".into(),
                    max: 1,
                    lease: Duration::from_secs(30),
                    kinds: None,
                    worker: "w".into(),
                })
                .await
                .expect("claimed")
                .pop()
                .expect("one");
            store.settle(&c.lease, Settle::Done).await.expect("settled");
        }
    }

    let a = Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .panel(JobsPanel::new(store))
            .with_policy(policy_fn(|_: &Principal<'_>, _: Request<'_>| true)),
    );

    let dash = get(&a, "/admin/jobs");
    assert!(dash.contains(">History<"), "the tab is missing: {dash}");

    let html = get(&a, "/admin/jobs/history");
    assert!(html.contains("render_invoice"), "{html}");
    assert!(html.contains("send_digest"), "{html}");
    // Twenty-four hourly bars plus the one in progress, every empty hour
    // present. A chart that omits them reads as a quieter day than it was.
    // Matching the whole attribute, because `height:` on its own also
    // appears a few dozen times in the inlined stylesheet.
    assert_eq!(
        html.matches(r#"style="height:"#).count(),
        25,
        "wrong number of bars"
    );
    // One of them is this hour, holding all three, and it is the tallest.
    assert_eq!(html.matches(r#"style="height:100%""#).count(), 1, "{html}");
    assert!(
        html.contains(r#""3 at "#),
        "the bar should say what is in it"
    );
    // The caveat is on the screen, not only in a doc comment.
    assert!(html.contains("lower bound"), "{html}");

    pool.close().await;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP SCHEMA IF EXISTS {schema} CASCADE"
    )))
    .execute(&admin_pool)
    .await
    .expect("drop");
}
