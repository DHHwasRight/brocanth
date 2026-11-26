//! The jobs tab's data contract (docs/backoffice.md §7.6): each call and
//! answer survives JSON, every screen is the same in this process and over
//! the wire, and the data side checks the grant of each call. On the memory
//! queue and on Postgres.
//!
//! The wire is `remote::Loopback`: it encodes each call as the backoffice
//! wire does, and checks it with the same `remote::authorize`.

#![cfg(all(feature = "admin", feature = "memory"))]

#[macro_use]
mod common;

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use brocanth_admin::panel::remote::{Grants, Loopback};
use brocanth_admin::policy::{policy_fn, Principal, Request};
use brocanth_admin::{Admin, AdminConfig, Fixture, Submission};
use brocanth_jobs::admin::{JobsAct, JobsAnswer, JobsCall, JobsOverview, JobsPanel};
use brocanth_jobs::{Queue, Worker};
use brocanth_queue::{Bucket, DeadId, JobId, Stats, View};
use common::{token, wait_for, DiesAtOnce, Echo};

/// A member of staff, as the loopback reads their grants.
struct Who(Grants);

fn who(permissions: &[&str]) -> Who {
    Who(Grants {
        superuser: false,
        permissions: permissions.iter().map(|p| p.to_string()).collect(),
    })
}

fn grants(principal: &Principal<'_>) -> Grants {
    principal
        .downcast::<Who>()
        .map(|who| who.0.clone())
        .unwrap_or_default()
}

/// The application's admin, whose tab serves itself, and the backoffice's,
/// whose tab reads the same queue through the loopback. The backoffice's
/// own queue is empty: what its tab shows came over the wire.
fn both(queue: &Queue) -> (Admin, Admin) {
    let allow = || policy_fn(|_: &Principal<'_>, _: Request<'_>| true);
    let application = AdminConfig::new().panel(JobsPanel::new(Arc::new(queue.clone())));
    let wire = application
        .panel_serves()
        .into_iter()
        .fold(Loopback::new(grants), |wire, (tab, serve)| {
            wire.panel(tab, serve)
        })
        .shared();
    let local = Admin::new(Arc::new(Fixture::blog()), application.with_policy(allow()));
    let remote = Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .panel(JobsPanel::new(Arc::new(Queue::memory())))
            .panels_through(wire)
            .with_policy(allow()),
    );
    (local, remote)
}

async fn send(admin: &Admin, who: &Who, method: &str, path: &str) -> Result<(u16, String), String> {
    admin
        .dispatch(Principal::new(who), method, path, "", &Submission::new())
        .await
        .map(|page| {
            let shown = page
                .html()
                .or(page.location())
                .unwrap_or_default()
                .to_string();
            (page.status(), shown)
        })
        .map_err(|e| e.to_string())
}

/// A scheduled job and a dead one. Answers their ids.
async fn rows(queue: &Queue) -> (String, String) {
    let t = token("panel-wire");
    queue
        .enqueue(Echo {
            token: t.clone(),
            n: 1,
        })
        .in_(Duration::from_secs(3600))
        .await
        .expect("scheduled");
    queue
        .enqueue(DiesAtOnce { token: t.clone() })
        .await
        .expect("enqueued");
    let worker = Worker::builder(queue)
        .poll_interval(Duration::from_millis(10))
        .register::<DiesAtOnce>()
        .start()
        .expect("started");
    wait_for("one dead", Duration::from_secs(5), || async {
        queue.stats().await.expect("stats").dead == 1
    })
    .await;
    worker.shutdown(Duration::from_secs(5)).await.expect("stop");
    let scheduled = queue.scheduled(10, 0).await.expect("scheduled")[0]
        .id
        .0
        .clone();
    let dead = queue.dead_jobs(10, 0).await.expect("dead")[0].id.0.clone();
    (scheduled, dead)
}

#[test]
fn each_call_and_answer_survives_json() {
    let calls = [
        JobsCall::HasHistory,
        JobsCall::Overview,
        JobsCall::Jobs {
            view: View::Scheduled,
            limit: 100,
        },
        JobsCall::Dead { limit: 100 },
        JobsCall::Finished {
            since: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
            bucket: Bucket::Hour,
        },
        JobsCall::Act(JobsAct::RunNow(JobId("7".into()))),
        JobsCall::Act(JobsAct::Forget(DeadId("d7".into()))),
    ];
    for call in calls {
        let text = serde_json::to_string(&call).expect("it encodes");
        assert_eq!(serde_json::from_str::<JobsCall>(&text).unwrap(), call);
    }
    let answers = [
        JobsAnswer::History(true),
        JobsAnswer::Overview(JobsOverview {
            stats: Stats {
                ready: 1,
                oldest_ready: Some(Duration::from_secs(3)),
                ..Stats::default()
            },
            depths: Vec::new(),
        }),
        JobsAnswer::Done,
    ];
    for answer in answers {
        let text = serde_json::to_string(&answer).expect("it encodes");
        assert_eq!(serde_json::from_str::<JobsAnswer>(&text).unwrap(), answer);
    }
}

async fn every_screen_is_the_same_over_the_wire(queue: Queue) {
    let (scheduled, dead) = rows(&queue).await;
    let (local, remote) = both(&queue);
    let editor = who(&["jobs.edit"]);
    for path in [
        "/admin/jobs",
        "/admin/jobs/busy",
        "/admin/jobs/retries",
        "/admin/jobs/scheduled",
        "/admin/jobs/dead",
    ] {
        let here = send(&local, &editor, "GET", path).await;
        let there = send(&remote, &editor, "GET", path).await;
        assert_eq!(here, there, "{path}");
        assert_eq!(there.map(|(status, _)| status), Ok(200), "{path}");
    }
    let shown = send(&remote, &editor, "GET", "/admin/jobs/dead").await;
    assert!(
        shown.is_ok_and(|(_, html)| html.contains(&dead)),
        "the dead job crossed the wire"
    );
    // The buttons act on the application's queue.
    let run = format!("/admin/jobs/scheduled/{scheduled}/run");
    assert_eq!(
        send(&remote, &editor, "POST", &run).await,
        Ok((303, "/admin/jobs/scheduled".to_string()))
    );
    let forget = format!("/admin/jobs/dead/{dead}/delete");
    assert_eq!(
        send(&remote, &editor, "POST", &forget).await,
        Ok((303, "/admin/jobs/dead".to_string()))
    );
    assert!(queue.scheduled(10, 0).await.expect("scheduled").is_empty());
    assert!(queue.dead_jobs(10, 0).await.expect("dead").is_empty());
}

async fn the_data_side_checks_the_grant_of_each_call(queue: Queue) {
    let (scheduled, dead) = rows(&queue).await;
    let (_, remote) = both(&queue);
    let reader = who(&["jobs.read"]);
    let nobody = who(&[]);

    let read = send(&remote, &reader, "GET", "/admin/jobs/dead").await;
    assert_eq!(read.map(|(status, _)| status), Ok(200));
    for path in [
        format!("/admin/jobs/scheduled/{scheduled}/run"),
        format!("/admin/jobs/dead/{dead}/retry"),
        format!("/admin/jobs/dead/{dead}/delete"),
    ] {
        let refused = send(&remote, &reader, "POST", &path).await;
        assert!(
            refused.as_ref().is_err_and(|e| e.contains("jobs.edit")),
            "{path}: {refused:?}"
        );
    }
    assert_eq!(queue.scheduled(10, 0).await.expect("scheduled").len(), 1);
    assert_eq!(queue.dead_jobs(10, 0).await.expect("dead").len(), 1);
    let refused = send(&remote, &nobody, "GET", "/admin/jobs").await;
    assert!(
        refused.as_ref().is_err_and(|e| e.contains("jobs.read")),
        "{refused:?}"
    );
}

on_every_backend!(
    every_screen_is_the_same_over_the_wire,
    the_data_side_checks_the_grant_of_each_call,
);
