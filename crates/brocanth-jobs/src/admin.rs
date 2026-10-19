//! The queue, as a tab in `brocanth-admin`.
//!
//! Six screens, shaped like Sidekiq's because Sidekiq settled what an
//! operator wants to see: how much is waiting, what is running, what failed
//! and will be tried again, what was scheduled, and what gave up.
//!
//! # Why this lives here and not in the admin
//!
//! `brocanth-admin` has four dependencies and no SQL driver. A jobs panel
//! written there would drag the queue into every application with a
//! backoffice. So the admin owns the seam -- [`Panel`] -- and the crate that
//! owns the data owns the screens, behind an `admin` feature that is off.
//! A worker process links none of this, not even a template.
//!
//! # What is not here
//!
//! **No graph.** Sidekiq's dashboard plots processed and failed over time.
//! This queue deletes the row when the job succeeds, on purpose, so the
//! history that graph needs does not exist. Drawing one means a counters
//! table, which is a decision to take on its own rather than smuggle in
//! behind a chart.
//!
//! **No pause.** Sidekiq can stop a queue being drained. Here that would
//! mean a predicate on the claim -- the hottest statement there is -- and a
//! second kind of state for a worker to misread. It is worth doing and it
//! is worth doing deliberately.
//!
//! # Every button is a POST
//!
//! Retrying and deleting change something, so they are forms rather than
//! links: a crawler, a prefetching browser or a pasted URL must not be able
//! to fire them. The admin asks the policy for
//! [`Action::ActOnPanel`](brocanth_admin::policy::Action::ActOnPanel)
//! before this code runs, and a reader who may see the tab but not press
//! its buttons gets the tables with the last column missing.

use std::time::Duration;

use async_trait::async_trait;
use brocanth_admin::admin::AdminError;
use brocanth_admin::panel::{Panel, PanelPage, PanelRequest};
use brocanth_admin::view::render;

use crate::clock::from_millis;
use crate::queue::Queue;
use crate::store::{DeadJob, RunningJob, WaitingJob};

/// The panel's stylesheet, built from `style/panel.css`. Inlined by the
/// tab strip, which every screen draws. See `style/README.md`.
pub(crate) const STYLE: &str = include_str!("admin.css");

/// How many rows a screen shows. No paging yet: the screens that can grow
/// without bound are the dead letter and the retry set, and both are
/// ordered so the rows that matter are first.
const LIMIT: u32 = 100;

/// The queue as a tab.
///
/// ```ignore
/// AdminConfig::new().panel(JobsPanel::new(queue.clone()))
/// ```
pub struct JobsPanel {
    queue: Queue,
    key: String,
    label: String,
    /// What counts as a worker that stopped answering. Matches the
    /// worker's own `stale_after`, and is only ever used to colour a row.
    stale_after: Duration,
}

impl JobsPanel {
    /// The panel, at `jobs`, labelled "Background jobs".
    pub fn new(queue: Queue) -> Self {
        Self {
            queue,
            key: "jobs".to_string(),
            label: "Background jobs".to_string(),
            stale_after: Duration::from_secs(60),
        }
    }

    /// A different key, for an application that already has a `jobs` model
    /// or runs two queues.
    #[must_use]
    pub fn at(mut self, key: impl Into<String>) -> Self {
        self.key = key.into();
        self
    }

    /// A different tab label.
    #[must_use]
    pub fn labelled(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// How quiet a worker must go before this screen calls it out. Set it
    /// to whatever the workers were built with.
    #[must_use]
    pub fn stale_after(mut self, stale_after: Duration) -> Self {
        self.stale_after = stale_after;
        self
    }
}

#[async_trait]
impl Panel for JobsPanel {
    fn key(&self) -> &str {
        &self.key
    }

    fn label(&self) -> String {
        self.label.clone()
    }

    async fn render(&self, r: PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError> {
        if r.post {
            return self.act(&r).await;
        }
        match r.path {
            "" => self.dashboard(&r).await.map(Some),
            "queues" => self.dashboard(&r).await.map(Some),
            "busy" => self.busy(&r).await.map(Some),
            "retries" => self.waiting(&r, Waiting::Retries).await.map(Some),
            "scheduled" => self.waiting(&r, Waiting::Scheduled).await.map(Some),
            "dead" => self.dead(&r).await.map(Some),
            _ => Ok(None),
        }
    }
}

/// Which half of the waiting set a screen is showing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Waiting {
    Retries,
    Scheduled,
}

impl JobsPanel {
    // ------------------------------------------------------------ screens

    async fn dashboard(&self, r: &PanelRequest<'_>) -> Result<PanelPage, AdminError> {
        let stats = self.queue.stats().await.map_err(failed)?;
        let retries = self.queue.retries(LIMIT, 0).await.map_err(failed)?;
        let scheduled = self.queue.scheduled(LIMIT, 0).await.map_err(failed)?;

        let ctx = DashboardCtx {
            tabs: self.tabs(r, ""),
            counts: vec![
                count("Enqueued", stats.ready, r.url("queues")),
                count("Running", stats.running, r.url("busy")),
                count("Retries", retries.len() as u64, r.url("retries")),
                count("Scheduled", scheduled.len() as u64, r.url("scheduled")),
                count("Dead", stats.dead, r.url("dead")),
            ],
            oldest: match stats.oldest_ready_age {
                Some(age) => ago(age),
                None => "nothing waiting".to_string(),
            },
            queues: stats
                .queues
                .iter()
                .map(|q| QueueRow {
                    queue: q.queue.clone(),
                    ready: q.ready,
                    running: q.running,
                    scheduled: q.scheduled,
                    latency: match q.oldest_ready_age {
                        Some(age) => ago(age),
                        None => "—".to_string(),
                    },
                })
                .collect(),
        };
        self.page("Background jobs", &ctx, |ctx, w| {
            use rsx::Template as _;
            crate::admin_templates::dashboard::Dashboard { ctx }.render(w)
        })
    }

    async fn busy(&self, r: &PanelRequest<'_>) -> Result<PanelPage, AdminError> {
        let rows = self.queue.busy(LIMIT, 0).await.map_err(failed)?;
        let now = self.queue.now();
        let ctx = BusyCtx {
            tabs: self.tabs(r, "busy"),
            rows: rows.iter().map(|j| self.busy_row(j, now)).collect(),
        };
        self.page("Busy", &ctx, |ctx, w| {
            use rsx::Template as _;
            crate::admin_templates::busy::Busy { ctx }.render(w)
        })
    }

    async fn waiting(&self, r: &PanelRequest<'_>, which: Waiting) -> Result<PanelPage, AdminError> {
        let (rows, tab, title, empty) = match which {
            Waiting::Retries => (
                self.queue.retries(LIMIT, 0).await.map_err(failed)?,
                "retries",
                "Retries",
                "Nothing failed and is waiting to be tried again.",
            ),
            Waiting::Scheduled => (
                self.queue.scheduled(LIMIT, 0).await.map_err(failed)?,
                "scheduled",
                "Scheduled",
                "Nothing is waiting for a later time.",
            ),
        };
        let now = self.queue.now();
        let ctx = WaitingCtx {
            tabs: self.tabs(r, tab),
            title: title.to_string(),
            empty: empty.to_string(),
            can_act: r.can_act,
            rows: rows
                .iter()
                .map(|j| self.waiting_row(j, now, r, tab))
                .collect(),
        };
        self.page(title, &ctx, |ctx, w| {
            use rsx::Template as _;
            crate::admin_templates::waiting::Waiting { ctx }.render(w)
        })
    }

    async fn dead(&self, r: &PanelRequest<'_>) -> Result<PanelPage, AdminError> {
        let rows = self.queue.dead_jobs(LIMIT, 0).await.map_err(failed)?;
        let now = self.queue.now();
        let ctx = DeadCtx {
            tabs: self.tabs(r, "dead"),
            can_act: r.can_act,
            rows: rows.iter().map(|j| dead_row(j, now, r)).collect(),
        };
        self.page("Dead", &ctx, |ctx, w| {
            use rsx::Template as _;
            crate::admin_templates::dead::Dead { ctx }.render(w)
        })
    }

    // ------------------------------------------------------------ buttons

    /// Every POST. The admin has already asked the policy whether this
    /// person may press a button here, so what is left is which button.
    async fn act(&self, r: &PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError> {
        let parts: Vec<&str> = r.path.split('/').collect();
        let back = match parts.as_slice() {
            [screen, id, verb] => {
                let Ok(id) = id.parse::<i64>() else {
                    return Ok(None);
                };
                match (*screen, *verb) {
                    ("retries" | "scheduled", "run") => {
                        self.queue
                            .run_now(crate::job::JobId(id))
                            .await
                            .map_err(failed)?;
                    }
                    ("retries" | "scheduled", "delete") => {
                        self.queue
                            .discard(crate::job::JobId(id))
                            .await
                            .map_err(failed)?;
                    }
                    ("dead", "retry") => {
                        self.queue.requeue_dead(id).await.map_err(failed)?;
                    }
                    ("dead", "delete") => {
                        self.queue.discard_dead(id).await.map_err(failed)?;
                    }
                    _ => return Ok(None),
                }
                *screen
            }
            _ => return Ok(None),
        };
        // Back to the screen the button was on. A redirect and not a
        // re-render, so a reload does not press it again.
        Ok(Some(PanelPage::redirect(r.url(back))))
    }

    // ------------------------------------------------------------ helpers

    fn page<C>(
        &self,
        heading: &str,
        ctx: &C,
        view: impl Fn(&C, &mut rsx::Writer<'_>) -> rsx::Result + Send + Sync + 'static,
    ) -> Result<PanelPage, AdminError>
    where
        C: 'static,
    {
        Ok(PanelPage::body(heading, render(&view, ctx)?))
    }

    fn tabs(&self, r: &PanelRequest<'_>, current: &str) -> TabBar {
        let tabs = [
            ("", "Dashboard"),
            ("busy", "Busy"),
            ("retries", "Retries"),
            ("scheduled", "Scheduled"),
            ("dead", "Dead"),
        ];
        TabBar {
            tabs: tabs
                .iter()
                .map(|(path, label)| Tab {
                    label: (*label).to_string(),
                    href: r.url(path),
                    // "queues" is the dashboard under another name, so the
                    // first tab stays lit rather than none of them.
                    current: *path == current || (path.is_empty() && current == "queues"),
                })
                .collect(),
        }
    }

    fn busy_row(&self, j: &RunningJob, now: chrono::DateTime<chrono::Utc>) -> BusyRow {
        let since =
            |at: Option<i64>| at.map(|at| (now - from_millis(at)).to_std().unwrap_or_default());
        BusyRow {
            job_name: j.job_name.clone(),
            queue: j.queue.clone(),
            worker: j.locked_by.clone().unwrap_or_else(|| "—".to_string()),
            attempt: format!("{} of {}", j.attempts, j.max_attempts),
            running_for: since(j.locked_at).map_or("—".to_string(), ago),
            // Still holding its jobs, and no longer saying it is alive.
            // Until somebody reclaims them it looks exactly like a busy
            // worker, which is the one thing this screen must not let it do.
            stale: since(j.heartbeat_at).is_some_and(|quiet| quiet > self.stale_after),
        }
    }

    fn waiting_row(
        &self,
        j: &WaitingJob,
        now: chrono::DateTime<chrono::Utc>,
        r: &PanelRequest<'_>,
        screen: &str,
    ) -> WaitingRow {
        WaitingRow {
            job_name: j.job_name.clone(),
            queue: j.queue.clone(),
            attempt: format!("{} of {}", j.attempts, j.max_attempts),
            when: in_(from_millis(j.run_at) - now),
            error: j.last_error.clone(),
            run_href: r.url(&format!("{screen}/{}/run", j.id)),
            delete_href: r.url(&format!("{screen}/{}/delete", j.id)),
        }
    }
}

fn dead_row(j: &DeadJob, now: chrono::DateTime<chrono::Utc>, r: &PanelRequest<'_>) -> DeadRow {
    DeadRow {
        job_name: j.job_name.clone(),
        queue: j.queue.clone(),
        attempts: format!("{} of {}", j.attempts, j.max_attempts),
        failed: ago((now - from_millis(j.failed_at))
            .to_std()
            .unwrap_or_default()),
        error: j.error.clone(),
        payload: truncate(&j.payload, 120),
        retry_href: r.url(&format!("dead/{}/retry", j.id)),
        delete_href: r.url(&format!("dead/{}/delete", j.id)),
    }
}

fn count(label: &str, value: u64, href: String) -> CountCard {
    CountCard {
        label: label.to_string(),
        value,
        href,
    }
}

/// The queue's errors are not the admin's, and the admin's error type has
/// no arm for them. They arrive as a render failure carrying the text,
/// which is what a screen can actually show somebody.
fn failed(e: crate::Error) -> AdminError {
    AdminError::Render(rsx::Error::custom(format!("the queue failed: {e}")))
}

/// "4m" rather than "4 minutes 13 seconds 900 milliseconds". A column, not
/// a sentence.
fn ago(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0 => "just now".to_string(),
        1..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        3600..=86_399 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86_400),
    }
}

/// The same, forwards, for a job that has not run yet.
fn in_(d: chrono::Duration) -> String {
    match d.to_std() {
        Ok(d) => format!("in {}", ago(d)),
        // Due already, and waiting for a worker rather than for the clock.
        Err(_) => "now".to_string(),
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

// ---------------------------------------------------------------------------
// What the templates are given
// ---------------------------------------------------------------------------
//
// Every field is already a string the screen prints. No formatting happens
// in a template, which is what lets somebody replace one without
// reimplementing the duration arithmetic above.

/// The tab strip.
pub struct TabBar {
    /// Every tab, in order.
    pub tabs: Vec<Tab>,
}

/// One tab.
pub struct Tab {
    /// What it says.
    pub label: String,
    /// Where it goes.
    pub href: String,
    /// Whether it is the screen being shown.
    pub current: bool,
}

/// One of the five numbers at the top of the dashboard.
pub struct CountCard {
    /// What it counts.
    pub label: String,
    /// How many.
    pub value: u64,
    /// The screen that lists them.
    pub href: String,
}

/// One queue's depths.
pub struct QueueRow {
    /// Its name.
    pub queue: String,
    /// Jobs that could run now.
    pub ready: u64,
    /// Jobs a worker is holding.
    pub running: u64,
    /// Jobs waiting for their time.
    pub scheduled: u64,
    /// How long the oldest runnable job has waited.
    pub latency: String,
}

/// The dashboard.
pub struct DashboardCtx {
    /// The tab strip.
    pub tabs: TabBar,
    /// The five numbers.
    pub counts: Vec<CountCard>,
    /// How long the oldest runnable job anywhere has waited.
    pub oldest: String,
    /// One row per queue.
    pub queues: Vec<QueueRow>,
}

/// One row of the busy screen.
pub struct BusyRow {
    /// The job's name.
    pub job_name: String,
    /// Its queue.
    pub queue: String,
    /// Which worker holds it.
    pub worker: String,
    /// "2 of 25".
    pub attempt: String,
    /// How long it has been running.
    pub running_for: String,
    /// Whether that worker has stopped saying it is alive.
    pub stale: bool,
}

/// The busy screen.
pub struct BusyCtx {
    /// The tab strip.
    pub tabs: TabBar,
    /// One row per running job.
    pub rows: Vec<BusyRow>,
}

/// One row of the retries or scheduled screen.
pub struct WaitingRow {
    /// The job's name.
    pub job_name: String,
    /// Its queue.
    pub queue: String,
    /// "2 of 25".
    pub attempt: String,
    /// "in 4m".
    pub when: String,
    /// Why the last attempt failed, for a retry.
    pub error: Option<String>,
    /// Where the Run now button posts.
    pub run_href: String,
    /// Where the Delete button posts.
    pub delete_href: String,
}

/// The retries or scheduled screen.
pub struct WaitingCtx {
    /// The tab strip.
    pub tabs: TabBar,
    /// "Retries" or "Scheduled".
    pub title: String,
    /// What to say when there are none.
    pub empty: String,
    /// Whether to draw the buttons.
    pub can_act: bool,
    /// The rows.
    pub rows: Vec<WaitingRow>,
}

/// One row of the dead letter.
pub struct DeadRow {
    /// The job's name.
    pub job_name: String,
    /// Its queue.
    pub queue: String,
    /// "25 of 25".
    pub attempts: String,
    /// How long ago it gave up.
    pub failed: String,
    /// The last error.
    pub error: String,
    /// The payload, shortened.
    pub payload: String,
    /// Where the Retry button posts.
    pub retry_href: String,
    /// Where the Delete button posts.
    pub delete_href: String,
}

/// The dead letter screen.
pub struct DeadCtx {
    /// The tab strip.
    pub tabs: TabBar,
    /// Whether to draw the buttons.
    pub can_act: bool,
    /// The rows.
    pub rows: Vec<DeadRow>,
}
