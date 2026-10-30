//! `brocanth-jobs`, observed: a span and a log line per job run, run
//! counters, and the queue's depth as gauges.
//!
//! `brocanth-jobs` stays free of `tracing` on purpose: it emits
//! [`JobEvent`] values to whatever [`JobEvents`] sink the application
//! installed (see its `events` module). [`JobTelemetry`] is that sink. A
//! `Started` event opens a span; the `Completed`, `Retrying` or
//! `DeadLettered` event for the same job id records the outcome and closes
//! it. With the `otel` feature that span is exported like a request span.
//!
//! The limit, stated: the span opens and closes around the job, but the
//! job's own `perform` does not run inside it, because the queue calls the
//! sink and not the other way round. A `tracing::info!` inside `perform` is
//! logged, without the job's fields.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use brocanth_jobs::{JobEvent, JobEvents, JobId};

#[cfg(feature = "metrics")]
use prometheus_client::{
    encoding::EncodeLabelSet,
    metrics::{counter::Counter, family::Family, gauge::Gauge, histogram::Histogram},
};

#[cfg(feature = "metrics")]
use crate::metrics::{FloatGauge, Metrics};

/// The [`JobEvents`] sink an [`Observer`](crate::Observer) hands out.
///
/// Install it when the queue is built:
/// `Queue::connect(url).await?.with_events(observer.job_events())`.
#[derive(Clone, Default)]
pub struct JobTelemetry {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    running: Mutex<HashMap<JobId, Running>>,
    #[cfg(feature = "metrics")]
    series: JobSeries,
}

struct Running {
    started: Instant,
    #[cfg(feature = "logs")]
    span: tracing::Span,
}

impl std::fmt::Debug for JobTelemetry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobTelemetry").finish_non_exhaustive()
    }
}

impl JobTelemetry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Registers the run series in `metrics`. Called once, by the observer.
    #[cfg(feature = "metrics")]
    pub(crate) fn register(&self, metrics: &Metrics) {
        let series = &self.inner.series;
        metrics.register(
            "brocanth_jobs_runs",
            "Job attempts that finished, by job name and outcome (completed, retrying, dead)",
            series.runs.clone(),
        );
        metrics.register(
            "brocanth_jobs_run_duration_seconds",
            "Time from a worker claiming a job to the attempt finishing",
            series.duration.clone(),
        );
    }

    fn start(&self, id: JobId, name: &str, attempt: u32, worker: &str) {
        #[cfg(feature = "logs")]
        let span = {
            let span = tracing::info_span!(
                target: "brocanth::jobs",
                parent: None,
                "job",
                otel.name = %format!("job {name}"),
                otel.kind = "consumer",
                otel.status_code = tracing::field::Empty,
                job = %name,
                job_id = id.0,
                attempt,
                worker = %worker,
            );
            tracing::debug!(target: "brocanth::jobs", parent: &span, "job {name} started");
            span
        };
        #[cfg(not(feature = "logs"))]
        let _ = (name, attempt, worker);
        let running = Running {
            started: Instant::now(),
            #[cfg(feature = "logs")]
            span,
        };
        self.inner
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, running);
    }

    fn finish(&self, id: JobId, name: &str, outcome: Outcome<'_>) {
        let running = self
            .inner
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
        let took = running.as_ref().map(|r| r.started.elapsed());

        #[cfg(feature = "metrics")]
        {
            let series = &self.inner.series;
            series
                .runs
                .get_or_create(&RunLabels {
                    job: name.to_string(),
                    outcome: outcome.label().to_string(),
                })
                .inc();
            if let Some(took) = took {
                series
                    .duration
                    .get_or_create(&JobLabel {
                        job: name.to_string(),
                    })
                    .observe(took.as_secs_f64());
            }
        }

        #[cfg(feature = "logs")]
        {
            let span = running.map_or_else(tracing::Span::none, |r| r.span);
            let duration_ms = took.map(crate::text::millis);
            match outcome {
                Outcome::Completed => tracing::info!(
                    target: "brocanth::jobs", parent: &span,
                    outcome = "completed", duration_ms, "job {name} completed"
                ),
                Outcome::Retrying(error) => {
                    span.record("otel.status_code", "ERROR");
                    tracing::warn!(
                        target: "brocanth::jobs", parent: &span,
                        outcome = "retrying", duration_ms, error = %error, "job {name} failed, will retry"
                    );
                }
                Outcome::Dead(error) => {
                    span.record("otel.status_code", "ERROR");
                    tracing::error!(
                        target: "brocanth::jobs", parent: &span,
                        outcome = "dead", duration_ms, error = %error, "job {name} moved to the dead letter"
                    );
                }
            }
            // Dropping the last handle closes the span, which exports it.
            drop(span);
        }
        #[cfg(not(feature = "logs"))]
        let _ = (took, name, outcome);
    }
}

#[derive(Clone, Copy)]
#[cfg_attr(not(feature = "logs"), allow(dead_code))]
enum Outcome<'a> {
    Completed,
    Retrying(&'a dyn std::fmt::Display),
    Dead(&'a dyn std::fmt::Display),
}

#[cfg(feature = "metrics")]
impl Outcome<'_> {
    fn label(self) -> &'static str {
        match self {
            Outcome::Completed => "completed",
            Outcome::Retrying(_) => "retrying",
            Outcome::Dead(_) => "dead",
        }
    }
}

impl JobEvents for JobTelemetry {
    fn handle(&self, event: JobEvent<'_>) {
        match event {
            JobEvent::Started {
                id,
                name,
                attempt,
                worker,
            } => self.start(id, name, attempt, worker),
            JobEvent::Completed { id, name, .. } => self.finish(id, name, Outcome::Completed),
            JobEvent::Retrying {
                id, name, failure, ..
            } => self.finish(id, name, Outcome::Retrying(failure)),
            JobEvent::DeadLettered {
                id, name, failure, ..
            } => self.finish(id, name, Outcome::Dead(failure)),
            #[cfg(feature = "logs")]
            other => log_other(&other),
            #[cfg(not(feature = "logs"))]
            _ => {}
        }
    }
}

/// Everything that is not one job's run is a log line and nothing else.
#[cfg(feature = "logs")]
fn log_other(event: &JobEvent<'_>) {
    match event {
        JobEvent::InternalError { .. } => {
            tracing::error!(target: "brocanth::jobs", "{event}");
        }
        JobEvent::ListenerFailed { .. } | JobEvent::Reclaimed { .. } => {
            tracing::warn!(target: "brocanth::jobs", "{event}");
        }
        JobEvent::Enqueued { .. } | JobEvent::Deduplicated { .. } => {
            tracing::debug!(target: "brocanth::jobs", "{event}");
        }
        _ => tracing::info!(target: "brocanth::jobs", "{event}"),
    }
}

#[cfg(feature = "metrics")]
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct RunLabels {
    job: String,
    outcome: String,
}

#[cfg(feature = "metrics")]
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct JobLabel {
    job: String,
}

#[cfg(feature = "metrics")]
struct JobSeries {
    runs: Family<RunLabels, Counter>,
    duration: Family<JobLabel, Histogram>,
}

#[cfg(feature = "metrics")]
impl Default for JobSeries {
    fn default() -> Self {
        Self {
            runs: Family::default(),
            duration: Family::new_with_constructor(|| {
                Histogram::new([0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 15.0, 60.0, 300.0])
            }),
        }
    }
}

/// The queue's depth, read from its tables when Prometheus scrapes.
#[cfg(feature = "metrics")]
pub(crate) fn watch_queue(metrics: &Metrics, queue: brocanth_jobs::Queue) {
    #[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
    struct QueueLabel {
        queue: String,
    }

    let ready: Family<QueueLabel, Gauge> = Family::default();
    let retrying: Family<QueueLabel, Gauge> = Family::default();
    let scheduled: Family<QueueLabel, Gauge> = Family::default();
    let running: Family<QueueLabel, Gauge> = Family::default();
    let dead: Gauge = Gauge::default();
    let oldest: FloatGauge = FloatGauge::default();

    metrics.register(
        "brocanth_jobs_ready",
        "Jobs that could run now",
        ready.clone(),
    );
    metrics.register(
        "brocanth_jobs_retrying",
        "Jobs waiting out a backoff",
        retrying.clone(),
    );
    metrics.register(
        "brocanth_jobs_scheduled",
        "Jobs waiting for their run_at",
        scheduled.clone(),
    );
    metrics.register(
        "brocanth_jobs_running",
        "Jobs a worker holds",
        running.clone(),
    );
    metrics.register(
        "brocanth_jobs_dead",
        "Rows in the dead letter",
        dead.clone(),
    );
    metrics.register(
        "brocanth_jobs_oldest_ready_age_seconds",
        "How long the oldest runnable job has waited; the number that says a queue fell behind",
        oldest.clone(),
    );

    metrics.on_scrape(move || {
        let queue = queue.clone();
        let (ready, retrying, scheduled, running, dead, oldest) = (
            ready.clone(),
            retrying.clone(),
            scheduled.clone(),
            running.clone(),
            dead.clone(),
            oldest.clone(),
        );
        async move {
            let stats = match queue.stats().await {
                Ok(stats) => stats,
                #[allow(unused_variables)]
                Err(error) => {
                    #[cfg(feature = "logs")]
                    tracing::warn!(target: "brocanth::metrics", "cannot read the job queue's stats: {error}");
                    return;
                }
            };
            // A queue that emptied and vanished from the stats must not keep
            // its last value for ever.
            for family in [&ready, &retrying, &scheduled, &running] {
                family.clear();
            }
            for depth in &stats.queues {
                let label = QueueLabel {
                    queue: depth.queue.clone(),
                };
                ready.get_or_create(&label).set(clamp(depth.ready));
                retrying.get_or_create(&label).set(clamp(depth.retrying));
                scheduled.get_or_create(&label).set(clamp(depth.scheduled));
                running.get_or_create(&label).set(clamp(depth.running));
            }
            dead.set(clamp(stats.dead));
            oldest.set(stats.oldest_ready_age.map_or(0.0, |age| age.as_secs_f64()));
        }
    });
}

#[cfg(feature = "metrics")]
fn clamp(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}
