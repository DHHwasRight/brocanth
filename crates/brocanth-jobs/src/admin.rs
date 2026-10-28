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
//! **The graph is conditional.** Sidekiq's dashboard plots throughput over
//! time, and a queue that deletes the row when the job succeeds has no
//! history to plot. So the History tab appears only when the queue behind
//! the panel answers
//! [`has_history`](brocanth_queue::Inspect::has_history) — a seven day
//! archive, in `brocanth-queue`'s case. A queue that keeps nothing shows
//! no tab, rather than a chart of zeroes that reads like an outage.
//!
//! What it plots is **settles, not executions**. Delivery is at least
//! once, so a job that ran, had its effect, then lost its lease and was
//! retried leaves one row. It is a lower bound on what ran, and the screen
//! says so, because the wrong reading is the one somebody reaches for
//! during the incident the history exists for.
//!
//! **No pause.** Sidekiq can stop a queue being drained. Here that would
//! mean a predicate on the claim -- the hottest statement there is -- and a
//! second kind of state for a worker to misread. It is worth doing and it
//! is worth doing deliberately.
//!
//! # It does not know which queue it has
//!
//! The screens are written against [`Inspect`], the read side every queue
//! in this workspace implements, and the panel holds
//! `Arc<dyn DynInspect>`. So the same six screens serve this crate's
//! `Queue` and `brocanth-queue`, and an application that moves its jobs
//! from one to the other keeps its backoffice unchanged.
//!
//! There is no worker table behind the Busy screen, on either queue. A
//! worker is known by the jobs it holds, so one holding nothing is
//! invisible, which is correct: there is nothing to show about it and
//! nothing to do to it.
//!
//! # Every button is a POST
//!
//! Retrying and deleting change something, so they are forms rather than
//! links: a crawler, a prefetching browser or a pasted URL must not be able
//! to fire them. The admin asks the policy for
//! [`Action::ActOnPanel`](brocanth_admin::policy::Action::ActOnPanel)
//! before this code runs, and a reader who may see the tab but not press
//! its buttons gets the tables with the last column missing.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use brocanth_admin::admin::AdminError;
use brocanth_admin::panel::{Panel, PanelPage, PanelRequest};
use brocanth_admin::view::render;
// `DynInspect` and not `Inspect`: both carry the same method names, every
// implementor has both through the blanket impl, and a panel chooses its
// queue at run time. Importing the pair makes every call ambiguous.
use brocanth_queue::{Bucket, DeadId, DynInspect, JobId, JobRow, Page, View};

/// The panel's stylesheet, built from `style/panel.css`. Inlined by the
/// tab strip, which every screen draws. See `style/README.md`.
pub(crate) const STYLE: &str = include_str!("admin.css");

/// How many rows a screen shows. No paging yet: the screens that can grow
/// without bound are the dead letter and the retry set, and both are
/// ordered so the rows that matter are first.
const LIMIT: u32 = 100;

/// How long a job may be held before the Busy screen says so.
///
/// It replaces a heartbeat column, and is a weaker signal on purpose.
/// Neither queue has a worker roster to go quiet: a lease is the proof a
/// worker is alive, and a lease that runs out puts the job back on the
/// ready list by itself. So the thing left worth flagging is a job that is
/// genuinely taking too long, which no queue can tell apart from a slow
/// one — only the person reading can.
const LONG_RUNNING: Duration = Duration::from_secs(5 * 60);

/// How far back the History screen looks, and how wide its bars are.
///
/// A day in hours. Wider than the archive's retention would draw bars for
/// days that were dropped, and the chart would say the queue went quiet
/// when all that happened was a partition being deleted.
const HISTORY_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);
const HISTORY_BUCKET: Bucket = Bucket::Hour;

/// The queue as a tab.
///
/// ```ignore
/// AdminConfig::new().panel(JobsPanel::new(Arc::new(queue.clone())))
/// ```
pub struct JobsPanel {
    inspect: Arc<dyn DynInspect>,
    key: String,
    label: String,
    /// When a running job is called out as long running. Only ever used to
    /// colour a row.
    long_running_after: Duration,
}

impl JobsPanel {
    /// The panel, at `jobs`, labelled "Background jobs".
    ///
    /// Takes any queue that implements
    /// [`Inspect`](brocanth_queue::Inspect), which is both this crate's
    /// `Queue` and `brocanth-queue`.
    pub fn new(inspect: Arc<dyn DynInspect>) -> Self {
        Self {
            inspect,
            key: "jobs".to_string(),
            label: "Background jobs".to_string(),
            long_running_after: LONG_RUNNING,
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

    /// How long a job may run before the Busy screen calls it out.
    ///
    /// Set it above the slowest job you expect, or the screen cries wolf.
    #[must_use]
    pub fn long_running_after(mut self, after: Duration) -> Self {
        self.long_running_after = after;
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
            // Not reachable from the tab strip when the queue keeps no
            // history, and refused here too: a pasted URL must not render
            // a chart of nothing.
            "history" if self.inspect.has_history() => self.history(&r).await.map(Some),
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
        // Two statements and not five: the counts are one aggregate and
        // the per-queue table is one grouping. The old screen counted the
        // retry and scheduled cards by listing a hundred rows and taking
        // the length, which stopped being true at a hundred and one.
        let stats = self.inspect.stats().await.map_err(failed)?;
        let depths = self.inspect.depths().await.map_err(failed)?;

        let ctx = DashboardCtx {
            tabs: self.tabs(r, ""),
            counts: vec![
                count("Enqueued", stats.ready, r.url("queues")),
                count("Running", stats.running, r.url("busy")),
                count("Retries", stats.retrying, r.url("retries")),
                count("Scheduled", stats.scheduled, r.url("scheduled")),
                count("Dead", stats.dead, r.url("dead")),
            ],
            oldest: match stats.oldest_ready {
                Some(age) => ago(age),
                None => "nothing waiting".to_string(),
            },
            queues: depths
                .iter()
                .map(|q| QueueRow {
                    queue: q.queue.clone(),
                    ready: q.ready,
                    running: q.running,
                    scheduled: q.scheduled + q.retrying,
                    latency: match q.oldest_ready {
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
        let rows = self
            .inspect
            .list(View::Running, Page::first(LIMIT))
            .await
            .map_err(failed)?;
        let ctx = BusyCtx {
            tabs: self.tabs(r, "busy"),
            rows: rows.iter().map(|j| self.busy_row(j)).collect(),
        };
        self.page("Busy", &ctx, |ctx, w| {
            use rsx::Template as _;
            crate::admin_templates::busy::Busy { ctx }.render(w)
        })
    }

    async fn waiting(&self, r: &PanelRequest<'_>, which: Waiting) -> Result<PanelPage, AdminError> {
        let (view, tab, title, empty) = match which {
            Waiting::Retries => (
                View::Retrying,
                "retries",
                "Retries",
                "Nothing failed and is waiting to be tried again.",
            ),
            Waiting::Scheduled => (
                View::Scheduled,
                "scheduled",
                "Scheduled",
                "Nothing is waiting for a later time.",
            ),
        };
        let rows = self
            .inspect
            .list(view, Page::first(LIMIT))
            .await
            .map_err(failed)?;
        let ctx = WaitingCtx {
            tabs: self.tabs(r, tab),
            title: title.to_string(),
            empty: empty.to_string(),
            can_act: r.can_act,
            rows: rows.iter().map(|j| waiting_row(j, r, tab)).collect(),
        };
        self.page(title, &ctx, |ctx, w| {
            use rsx::Template as _;
            crate::admin_templates::waiting::Waiting { ctx }.render(w)
        })
    }

    async fn dead(&self, r: &PanelRequest<'_>) -> Result<PanelPage, AdminError> {
        let rows = self
            .inspect
            .dead(Page::first(LIMIT))
            .await
            .map_err(failed)?;
        let ctx = DeadCtx {
            tabs: self.tabs(r, "dead"),
            can_act: r.can_act,
            rows: rows.iter().map(|j| dead_row(j, r)).collect(),
        };
        self.page("Dead", &ctx, |ctx, w| {
            use rsx::Template as _;
            crate::admin_templates::dead::Dead { ctx }.render(w)
        })
    }

    async fn history(&self, r: &PanelRequest<'_>) -> Result<PanelPage, AdminError> {
        let now = SystemTime::now();
        let since = now - HISTORY_WINDOW;
        let rows = self
            .inspect
            .finished(since, HISTORY_BUCKET)
            .await
            .map_err(failed)?;

        let width = HISTORY_BUCKET.seconds();
        let slots = (HISTORY_WINDOW.as_secs() / width) as usize;

        // The axis is generated and then filled, rather than drawn from
        // what came back. An hour with no finished job has no row, and a
        // chart that simply omits it is a chart that reads as a quieter
        // day than it was. Both ends floor the same epoch arithmetic, so
        // the two line up whatever the database's time zone is.
        let first = HISTORY_BUCKET.floor(since);
        let mut counts = vec![0u64; slots + 1];
        let mut by_kind: std::collections::BTreeMap<String, u64> = Default::default();
        for f in &rows {
            by_kind
                .entry(f.kind.clone())
                .and_modify(|n| *n += f.count)
                .or_insert(f.count);
            if let Ok(offset) = f.at.duration_since(first) {
                let slot = (offset.as_secs() / width) as usize;
                if let Some(c) = counts.get_mut(slot) {
                    *c += f.count;
                }
            }
        }

        let tallest = counts.iter().copied().max().unwrap_or(0);
        let total: u64 = counts.iter().sum();
        let bars = counts
            .iter()
            .enumerate()
            .map(|(i, &count)| {
                let at = first + Duration::from_secs(i as u64 * width);
                let label = clock_label(at);
                Bar {
                    // Zero stays zero rather than becoming a sliver. An
                    // hour with nothing in it should look like nothing.
                    height: if tallest == 0 || count == 0 {
                        0
                    } else {
                        ((count * 100) / tallest).max(2) as u8
                    },
                    title: format!("{count} at {label}"),
                    // One label every six bars. Twenty-four of them do not
                    // fit, and dropping every other one still does not.
                    label: if i % 6 == 0 { label } else { String::new() },
                    count,
                }
            })
            .collect();

        let mut kinds: Vec<KindRow> = by_kind
            .into_iter()
            .map(|(kind, count)| KindRow {
                share: (count * 100).checked_div(total).unwrap_or(0) as u8,
                kind,
                count,
            })
            .collect();
        kinds.sort_by(|a, b| b.count.cmp(&a.count).then(a.kind.cmp(&b.kind)));

        let ctx = HistoryCtx {
            tabs: self.tabs(r, "history"),
            bars,
            kinds,
            total,
            window: format!("the last {}", ago(HISTORY_WINDOW)),
        };
        self.page("History", &ctx, |ctx, w| {
            use rsx::Template as _;
            crate::admin_templates::history::History { ctx }.render(w)
        })
    }

    // ------------------------------------------------------------ buttons

    /// Every POST. The admin has already asked the policy whether this
    /// person may press a button here, so what is left is which button.
    async fn act(&self, r: &PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError> {
        let parts: Vec<&str> = r.path.split('/').collect();
        let back = match parts.as_slice() {
            // The id is passed through as text rather than parsed. It
            // belongs to whichever queue is behind the trait, and one of
            // them tags a row from its delayed table so the button knows
            // which table to act on. Parsing it here would throw that away.
            [screen, id, verb] => {
                if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
                    return Ok(None);
                }
                match (*screen, *verb) {
                    ("retries" | "scheduled", "run") => {
                        self.inspect
                            .run_now(&JobId((*id).to_string()))
                            .await
                            .map_err(failed)?;
                    }
                    ("retries" | "scheduled", "delete") => {
                        self.inspect
                            .discard(&JobId((*id).to_string()))
                            .await
                            .map_err(failed)?;
                    }
                    ("dead", "retry") => {
                        self.inspect
                            .requeue(&DeadId((*id).to_string()))
                            .await
                            .map_err(failed)?;
                    }
                    ("dead", "delete") => {
                        self.inspect
                            .forget(&DeadId((*id).to_string()))
                            .await
                            .map_err(failed)?;
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
        let mut tabs = vec![
            ("", "Dashboard"),
            ("busy", "Busy"),
            ("retries", "Retries"),
            ("scheduled", "Scheduled"),
            ("dead", "Dead"),
        ];
        // Last, and only when there is something to plot.
        if self.inspect.has_history() {
            tabs.push(("history", "History"));
        }
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

    fn busy_row(&self, j: &JobRow) -> BusyRow {
        let held = j.running_for();
        BusyRow {
            job_name: j.kind.clone(),
            queue: j.queue.clone(),
            worker: j.claimed_by.clone().unwrap_or_else(|| "—".to_string()),
            attempt: format!("{} of {}", j.attempt, j.max_attempts),
            running_for: held.map_or("—".to_string(), ago),
            // Not "the worker stopped answering": a lease is the proof a
            // worker is alive and an expired one puts the job back on the
            // ready list by itself. What is left worth flagging is a job
            // that has been held a long time.
            stale: held.is_some_and(|d| d > self.long_running_after),
        }
    }
}

fn waiting_row(j: &JobRow, r: &PanelRequest<'_>, screen: &str) -> WaitingRow {
    WaitingRow {
        job_name: j.kind.clone(),
        queue: j.queue.clone(),
        attempt: format!("{} of {}", j.attempt, j.max_attempts),
        when: in_(j.available_at),
        error: j.last_error.clone(),
        run_href: r.url(&format!("{screen}/{}/run", j.id)),
        delete_href: r.url(&format!("{screen}/{}/delete", j.id)),
    }
}

fn dead_row(j: &brocanth_queue::DeadRow, r: &PanelRequest<'_>) -> DeadRow {
    DeadRow {
        job_name: j.kind.clone(),
        queue: j.queue.clone(),
        attempts: match j.max_attempts {
            Some(max) => format!("{} of {}", j.attempts, max),
            // A row written before the column existed. The screen says so
            // rather than printing "3 of 0".
            None => format!("{}", j.attempts),
        },
        failed: ago(since(j.died_at)),
        error: j.failure.clone(),
        payload: truncate(&String::from_utf8_lossy(&j.payload), 120),
        retry_href: r.url(&format!("dead/{}/retry", j.id)),
        delete_href: r.url(&format!("dead/{}/delete", j.id)),
    }
}

/// How long ago an instant was, clamped at zero.
///
/// Zero rather than nothing when it is in the future: the queue stamps
/// these with the database's clock and this reads the process's, and the
/// two are not the same clock.
fn since(t: SystemTime) -> Duration {
    SystemTime::now().duration_since(t).unwrap_or_default()
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
fn failed(e: brocanth_queue::BackendError) -> AdminError {
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
fn in_(t: SystemTime) -> String {
    match t.duration_since(SystemTime::now()) {
        Ok(d) => format!("in {}", ago(d)),
        // Due already, and waiting for a worker rather than for the clock.
        Err(_) => "now".to_string(),
    }
}

/// "14:00", in UTC.
///
/// UTC and not the server's local zone, because the buckets are epoch
/// arithmetic and a label in another zone would disagree with the bar it
/// sits under.
fn clock_label(t: SystemTime) -> String {
    let secs = t
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    format!("{:02}:{:02}", (secs / 3600) % 24, (secs / 60) % 60)
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

/// One bar on the History chart.
pub struct Bar {
    /// The hour it covers, or empty when the axis does not label it.
    pub label: String,
    /// How many finished in it.
    pub count: u64,
    /// Its height as a percentage of the tallest bar, 0 to 100.
    pub height: u8,
    /// The whole fact, for a tooltip.
    pub title: String,
}

/// One job kind's share of the window.
pub struct KindRow {
    /// Its registered kind.
    pub kind: String,
    /// How many finished.
    pub count: u64,
    /// Its share of the total, as a percentage.
    pub share: u8,
}

/// The History screen.
pub struct HistoryCtx {
    /// The tab strip.
    pub tabs: TabBar,
    /// One bar per bucket, oldest first, with the empty ones present.
    pub bars: Vec<Bar>,
    /// What finished, by kind, most first.
    pub kinds: Vec<KindRow>,
    /// How many finished in the whole window.
    pub total: u64,
    /// How far back the screen looked, as a phrase.
    pub window: String,
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
