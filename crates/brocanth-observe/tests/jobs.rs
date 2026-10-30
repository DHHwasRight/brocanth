//! `brocanth-jobs`, observed: a log line and a counter per run, the queue's
//! depth on `/metrics`, and the queue's readiness check on `/readyz`.

#![cfg(all(feature = "_jobs", feature = "logs", feature = "metrics"))]

use std::time::Duration;

use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth_jobs::{
    Job, JobContext, JobEvent, JobEvents, JobFailure, JobId, JobsConfig, JobsPlugin, Queue,
};
use brocanth_observe::logs::Memory;
use brocanth_observe::{LogFormat, ObserveConfig, ObservePlugin, Observer};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct SendWelcome {
    user_id: i64,
}

impl Job for SendWelcome {
    const NAME: &'static str = "send_welcome";
    const QUEUE: &'static str = "mailers";
    type Error = std::io::Error;

    async fn perform(&self, _cx: &JobContext) -> Result<(), Self::Error> {
        Ok(())
    }
}

fn observer() -> Observer {
    let mut config = ObserveConfig::new("worker");
    config.install_subscriber = false;
    config.log_format = LogFormat::Json;
    config.check_timeout = Duration::from_secs(5);
    Observer::new(config).unwrap()
}

#[test]
fn each_run_is_a_log_line_in_its_span_and_a_counted_outcome() {
    let observer = observer();
    let memory = Memory::new();
    let events = observer.job_events();
    tracing::subscriber::with_default(observer.subscriber(memory.clone()).unwrap(), || {
        let failure = JobFailure::returned("smtp refused");
        for (id, end) in [(1, "completed"), (2, "retrying"), (3, "dead")] {
            events.handle(JobEvent::Started {
                id: JobId(id),
                name: "send_welcome",
                attempt: 1,
                worker: "w1",
            });
            events.handle(match end {
                "completed" => JobEvent::Completed {
                    id: JobId(id),
                    name: "send_welcome",
                    took: Duration::from_millis(3),
                    attempt: 1,
                },
                "retrying" => JobEvent::Retrying {
                    id: JobId(id),
                    name: "send_welcome",
                    attempt: 1,
                    delay: Duration::from_secs(1),
                    failure: &failure,
                },
                _ => JobEvent::DeadLettered {
                    id: JobId(id),
                    name: "send_welcome",
                    attempts: 1,
                    failure: &failure,
                },
            });
        }
    });

    let lines = memory.lines();
    let completed = lines
        .iter()
        .find(|l| l.contains("job send_welcome completed"))
        .expect("a completed line");
    for part in [
        r#""level":"info""#,
        r#""target":"brocanth::jobs""#,
        r#""job":"send_welcome""#,
        r#""job_id":1"#,
        r#""attempt":1"#,
        r#""worker":"w1""#,
        r#""outcome":"completed""#,
        r#""duration_ms":"#,
    ] {
        assert!(completed.contains(part), "missing {part} in {completed}");
    }
    let dead = lines
        .iter()
        .find(|l| l.contains("dead letter"))
        .expect("a dead line");
    assert!(dead.contains(r#""level":"error""#), "{dead}");
    assert!(dead.contains(r#""outcome":"dead""#), "{dead}");
    assert!(dead.contains("smtp refused\""), "{dead}");

    let text = observer.metrics().encode();
    for expected in [
        r#"brocanth_jobs_runs_total{job="send_welcome",outcome="completed"} 1"#,
        r#"brocanth_jobs_runs_total{job="send_welcome",outcome="retrying"} 1"#,
        r#"brocanth_jobs_runs_total{job="send_welcome",outcome="dead"} 1"#,
        r#"brocanth_jobs_run_duration_seconds_count{job="send_welcome"} 3"#,
    ] {
        assert!(text.contains(expected), "missing `{expected}` in:\n{text}");
    }
}

#[test]
fn the_queue_depth_is_scraped_and_its_table_is_a_readiness_check() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let path = std::env::temp_dir().join(format!(
        "brocanth-observe-jobs-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let url = format!("sqlite://{}?mode=rwc", path.display());
    let observer = observer();

    runtime.block_on(async {
        let queue = Queue::connect(&url)
            .await
            .unwrap()
            .with_events(observer.job_events());

        // Before the table exists: the jobs check fails.
        let unready = {
            let app = AppBuilder::new()
                .plugin(ObservePlugin::new(observer.clone()))
                .plugin(JobsPlugin::new(JobsConfig::new(queue.clone())))
                .build()
                .expect("builds");
            let report = observer.ready().await;
            drop(app);
            report
        };
        assert!(!unready.is_ready(), "{unready:?}");
        assert_eq!(unready.checks[0].name, "jobs");

        queue.migrate().await.unwrap();
        queue.enqueue(SendWelcome { user_id: 1 }).await.unwrap();
        queue.enqueue(SendWelcome { user_id: 2 }).await.unwrap();

        let report = observer.ready().await;
        assert!(report.is_ready(), "{report:?}");

        let text = observer.metrics().render(Duration::from_secs(5)).await;
        for expected in [
            r#"brocanth_jobs_ready{queue="mailers"} 2"#,
            "brocanth_jobs_dead 0",
            "# TYPE brocanth_jobs_oldest_ready_age_seconds gauge",
        ] {
            assert!(text.contains(expected), "missing `{expected}` in:\n{text}");
        }
    });
    let _ = std::fs::remove_file(&path);
}
