//! The numbers, as a tab in `brocanth-admin`.
//!
//! One read-only screen per site: visitors and views over 7, 30 or 90
//! days or any two dates, a bar per day, and the top pages, referrers,
//! browsers, systems, devices and countries. The sites are this server's
//! own and each static site that sends beacons. Rendered on the server,
//! with no script.
//!
//! The bars are SVG `<rect>`s with `width` and `height` attributes, not
//! `style="width: …"`: a presentation attribute is not an inline style, so
//! the screen needs nothing from the Content-Security-Policy.
//!
//! Locked like every tab: reading it is `Action::ViewPanel` on `analytics`,
//! which the admin's shipped `DenyAll` refuses until the application says
//! who may.
//!
//! The tab reads through [`AnalyticsData`] (docs/backoffice.md §7.6): in
//! this process over the recorder, or over the backoffice wire
//! ([`RemoteAnalytics`], answered by [`AnalyticsServe`]). Both calls are
//! reads and need `analytics.read` on the data side, which also holds a
//! range to [`MAX_RANGE`] days.

use std::sync::Arc;

use async_trait::async_trait;
use brocanth_admin::admin::AdminError;
use brocanth_admin::late::Late;
use brocanth_admin::panel::remote::{PanelServe, PanelWire};
use brocanth_admin::panel::{Panel, PanelPage, PanelRequest};
use brocanth_admin::parts;
use brocanth_admin::policy::{Level, Principal};
use serde::{Deserialize, Serialize};

use crate::day::Day;
use crate::recorder::Recorder;
use crate::store::{Dimension, Line, Report};

/// The tab's key: its RBAC resource, and its name on the backoffice wire.
pub const TAB: &str = "analytics";

/// The ranges the screen offers, in days.
const RANGES: [u32; 3] = [7, 30, 90];

/// What the analytics tab reads (docs/backoffice.md §7.6, P1).
///
/// In this process it reads the recorder; [`RemoteAnalytics`] sends it
/// over the backoffice wire, and [`AnalyticsServe`] answers it there.
#[async_trait]
pub trait AnalyticsData: Send + Sync + 'static {
    /// The sites, today, and the recorder's counters.
    ///
    /// # Errors
    ///
    /// The failure as text, with a `help:` line.
    async fn overview(&self, who: &Principal<'_>) -> Result<AnalyticsOverview, String>;

    /// The numbers for `site` from `from` to `to`, both included.
    ///
    /// # Errors
    ///
    /// The failure as text: a range longer than [`MAX_RANGE`] days, or the
    /// store's error.
    async fn report(
        &self,
        who: &Principal<'_>,
        site: &str,
        from: Day,
        to: Day,
    ) -> Result<Report, String>;
}

/// What the screen shows around the report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalyticsOverview {
    /// This server's site.
    pub site: String,
    /// This server's site first, then every static site that sent a beacon.
    pub sites: Vec<String>,
    /// Today, by the recorder's clock.
    pub today: Day,
    /// Views written since the process started.
    pub written: u64,
    /// Views waiting to be written.
    pub pending: usize,
    /// Views dropped because the buffer was full.
    pub dropped: u64,
    /// What the last write failed with.
    pub last_error: Option<String>,
}

/// One call of [`AnalyticsData`], as the wire carries it. Both need `read`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnalyticsCall {
    /// [`AnalyticsData::overview`].
    Overview,
    /// [`AnalyticsData::report`].
    Report {
        /// The site.
        site: String,
        /// The first day.
        from: Day,
        /// The last day.
        to: Day,
    },
}

/// What an [`AnalyticsCall`] answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnalyticsAnswer {
    /// To `Overview`.
    Overview(AnalyticsOverview),
    /// To `Report`.
    Report(Report),
}

/// The recorder in this process: empty until the first request that
/// carries it.
struct Recorded(Late<Recorder>);

impl Recorded {
    fn recorder(&self) -> Result<&Recorder, String> {
        self.0.get().ok_or_else(|| {
            "the analytics plugin is not registered in this application\nhelp: register it, \
             which puts the recorder on the request context"
                .to_string()
        })
    }
}

#[async_trait]
impl AnalyticsData for Recorded {
    async fn overview(&self, _who: &Principal<'_>) -> Result<AnalyticsOverview, String> {
        let recorder = self.recorder()?;
        Ok(AnalyticsOverview {
            site: recorder.site().to_string(),
            sites: recorder
                .sites()
                .await
                .unwrap_or_else(|_| vec![recorder.site().to_string()]),
            today: recorder.today(),
            written: recorder.written(),
            pending: recorder.pending(),
            dropped: recorder.dropped(),
            last_error: recorder.last_error(),
        })
    }

    async fn report(
        &self,
        _who: &Principal<'_>,
        site: &str,
        from: Day,
        to: Day,
    ) -> Result<Report, String> {
        if from > to || to.0 - from.0 >= MAX_RANGE as i32 {
            return Err(format!(
                "a report covers one to {MAX_RANGE} days, not {from} to {to}\nhelp: ask for a \
                 shorter range"
            ));
        }
        self.recorder()?
            .report_for(site, from, to)
            .await
            .map_err(|e| e.to_string())
    }
}

/// The data side of the analytics tab on the backoffice wire: decodes an
/// [`AnalyticsCall`] and runs it on an [`AnalyticsData`].
pub struct AnalyticsServe(Arc<dyn AnalyticsData>);

impl AnalyticsServe {
    /// Serves `data`.
    pub fn new(data: Arc<dyn AnalyticsData>) -> AnalyticsServe {
        AnalyticsServe(data)
    }

    /// As the wire server takes it.
    pub fn shared(self) -> Arc<dyn PanelServe> {
        Arc::new(self)
    }
}

impl std::fmt::Debug for AnalyticsServe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AnalyticsServe")
    }
}

fn decode(call: &str) -> Result<AnalyticsCall, String> {
    serde_json::from_str(call).map_err(|e| format!("an analytics call did not decode: {e}"))
}

#[async_trait]
impl PanelServe for AnalyticsServe {
    fn level(&self, call: &str) -> Result<Level, String> {
        decode(call).map(|_| Level::Read)
    }

    async fn serve(&self, who: &Principal<'_>, call: &str) -> Result<String, String> {
        let answer = match decode(call)? {
            AnalyticsCall::Overview => AnalyticsAnswer::Overview(self.0.overview(who).await?),
            AnalyticsCall::Report { site, from, to } => {
                AnalyticsAnswer::Report(self.0.report(who, &site, from, to).await?)
            }
        };
        serde_json::to_string(&answer)
            .map_err(|e| format!("an analytics answer did not encode: {e}"))
    }
}

/// The analytics tab's data in another process, through the backoffice
/// wire.
pub struct RemoteAnalytics(Arc<dyn PanelWire>);

impl RemoteAnalytics {
    /// The analytics behind `wire`.
    pub fn new(wire: Arc<dyn PanelWire>) -> RemoteAnalytics {
        RemoteAnalytics(wire)
    }

    async fn ask(
        &self,
        who: &Principal<'_>,
        call: &AnalyticsCall,
    ) -> Result<AnalyticsAnswer, String> {
        let text = serde_json::to_string(call)
            .map_err(|e| format!("an analytics call did not encode: {e}"))?;
        let answer = self.0.send(who, TAB, text).await?;
        serde_json::from_str(&answer)
            .map_err(|e| format!("an analytics answer did not decode: {e}"))
    }
}

impl std::fmt::Debug for RemoteAnalytics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RemoteAnalytics")
    }
}

#[async_trait]
impl AnalyticsData for RemoteAnalytics {
    async fn overview(&self, who: &Principal<'_>) -> Result<AnalyticsOverview, String> {
        match self.ask(who, &AnalyticsCall::Overview).await? {
            AnalyticsAnswer::Overview(overview) => Ok(overview),
            AnalyticsAnswer::Report(_) => Err("the data side answered with a report".into()),
        }
    }

    async fn report(
        &self,
        who: &Principal<'_>,
        site: &str,
        from: Day,
        to: Day,
    ) -> Result<Report, String> {
        let site = site.to_string();
        match self
            .ask(who, &AnalyticsCall::Report { site, from, to })
            .await?
        {
            AnalyticsAnswer::Report(report) => Ok(report),
            AnalyticsAnswer::Overview(_) => Err("the data side answered with no report".into()),
        }
    }
}

/// Where the tab reads.
enum Source {
    /// The recorder, which arrives with the first request.
    Local(Late<Recorder>),
    /// Any data side: another process, through the wire.
    Data(Arc<dyn AnalyticsData>),
}

/// The analytics tab.
pub struct AnalyticsPanel {
    source: Source,
}

impl AnalyticsPanel {
    /// A tab over this recorder.
    pub fn new(recorder: Recorder) -> Self {
        Self::late(Late::ready(recorder))
    }

    /// A tab over a recorder that arrives later, in `cell`: for a generated
    /// admin, which mounts before the plugin is built. The admin fills the
    /// cell from the first request that carries the [`Recorder`].
    pub fn late(cell: Late<Recorder>) -> Self {
        Self {
            source: Source::Local(cell),
        }
    }

    /// A tab over any [`AnalyticsData`]: [`RemoteAnalytics`] in the
    /// backoffice binary, which reads the numbers through the wire (§7.6).
    pub fn over(data: Arc<dyn AnalyticsData>) -> Self {
        Self {
            source: Source::Data(data),
        }
    }

    /// What the tab reads through, or `None` before the recorder arrived.
    fn data(&self) -> Option<Arc<dyn AnalyticsData>> {
        match &self.source {
            Source::Local(cell) => cell
                .get()
                .is_some()
                .then(|| Arc::new(Recorded(cell.clone())) as Arc<dyn AnalyticsData>),
            Source::Data(data) => Some(Arc::clone(data)),
        }
    }
}

#[async_trait]
impl Panel for AnalyticsPanel {
    fn key(&self) -> &str {
        TAB
    }

    fn label(&self) -> String {
        "Analytics".to_string()
    }

    fn serve(&self) -> Option<Arc<dyn PanelServe>> {
        match &self.source {
            Source::Local(cell) => {
                Some(AnalyticsServe::new(Arc::new(Recorded(cell.clone()))).shared())
            }
            Source::Data(_) => None,
        }
    }

    fn remote(&self, wire: Arc<dyn PanelWire>) -> Option<Arc<dyn Panel>> {
        Some(Arc::new(AnalyticsPanel::over(Arc::new(
            RemoteAnalytics::new(wire),
        ))))
    }

    async fn render(&self, r: PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError> {
        if r.post || !r.path.is_empty() {
            return Ok(None);
        }
        let Some(data) = self.data() else {
            return Ok(Some(PanelPage::body(
                "Analytics",
                format!(
                    "<p class=\"{}\">The analytics plugin is not registered in this application.</p>",
                    parts::muted()
                ),
            )));
        };
        let who = &r.principal;
        let overview = match data.overview(who).await {
            Ok(overview) => overview,
            Err(e) => {
                return Ok(Some(PanelPage::body(
                    "Analytics",
                    format!(
                        "<p role=\"alert\" class=\"{}\">The numbers could not be read: {}</p>",
                        parts::muted(),
                        escape(&e)
                    ),
                )))
            }
        };
        let (from, to, current) = range(&r, overview.today);
        // This server's site first, then every static site that sent a
        // beacon. A `site` that is not one of them is this server's: the
        // parameter picks from the list, it never names a new one.
        let sites = &overview.sites;
        let site = r
            .param("site")
            .filter(|s| sites.contains(s))
            .unwrap_or_else(|| overview.site.clone());
        let mut html = String::new();
        html.push_str(&ranges(&r, current, from, to, sites, &site));
        match data.report(who, &site, from, to).await {
            Ok(report) => html.push_str(&screen(&report, from, to)),
            Err(e) => html.push_str(&format!(
                "<p class=\"{}\">The report failed: {}</p>",
                parts::muted(),
                escape(&e)
            )),
        }
        html.push_str(&status(&overview));
        Ok(Some(PanelPage::body("Analytics", html)))
    }
}

/// The longest custom range, in days: two years, so one request cannot
/// ask the database for a decade.
pub const MAX_RANGE: u32 = 731;

/// The range asked for: `from` and `to` as dates when both read and make a
/// range of at most [`MAX_RANGE`] days, else `days` (7, 30 or 90) ending
/// today, else 30 days. The third value is the preset in use, if any.
fn range(r: &PanelRequest<'_>, today: Day) -> (Day, Day, Option<u32>) {
    let custom = r
        .param("from")
        .and_then(|f| Day::parse(&f))
        .zip(r.param("to").and_then(|t| Day::parse(&t)));
    if let Some((from, to)) = custom {
        if from <= to && to.0 - from.0 < MAX_RANGE as i32 {
            return (from, to, None);
        }
    }
    let days = r
        .param("days")
        .and_then(|d| d.parse::<u32>().ok())
        .filter(|d| RANGES.contains(d))
        .unwrap_or(30);
    (today.minus(days - 1), today, Some(days))
}

/// The site picker, then the three presets as links, then a form for any
/// two dates. Every form is a GET, so it changes nothing and needs no
/// token. The site travels with the range, and the range with the site.
fn ranges(
    r: &PanelRequest<'_>,
    current: Option<u32>,
    from: Day,
    to: Day,
    sites: &[String],
    site: &str,
) -> String {
    let action = escape(&r.url(""));
    let site_query = format!("site={}", encode(site));
    let links: Vec<String> = RANGES
        .iter()
        .map(|&d| {
            let label = format!("{d} days");
            if Some(d) == current {
                format!("<strong>{label}</strong>")
            } else {
                format!(
                    "<a class=\"{}\" href=\"{action}?days={d}&amp;{}\">{label}</a>",
                    parts::action(),
                    escape(&site_query),
                )
            }
        })
        .collect();
    let options: String = sites
        .iter()
        .map(|s| {
            format!(
                "<option value=\"{value}\"{selected}>{value}</option>",
                value = escape(s),
                selected = if s == site { " selected" } else { "" },
            )
        })
        .collect();
    // The site form keeps the range on screen: a preset as `days`, a
    // custom range as `from` and `to`.
    let kept = match current {
        Some(d) => format!("<input type=\"hidden\" name=\"days\" value=\"{d}\">"),
        None => format!(
            "<input type=\"hidden\" name=\"from\" value=\"{from}\">\
             <input type=\"hidden\" name=\"to\" value=\"{to}\">"
        ),
    };
    format!(
        "<form method=\"get\" action=\"{action}\" class=\"{bar}\">\
         <label>Site <select name=\"site\" class=\"{control}\">{options}</select></label>{kept} \
         <button type=\"submit\" class=\"{button}\">Show</button></form>\
         <p class=\"{bar}\">{links}</p>\
         <form method=\"get\" action=\"{action}\" class=\"{bar}\">\
         <input type=\"hidden\" name=\"site\" value=\"{site}\">\
         <label>From <input type=\"date\" name=\"from\" value=\"{from}\" class=\"{control}\" required></label> \
         <label>To <input type=\"date\" name=\"to\" value=\"{to}\" class=\"{control}\" required></label> \
         <button type=\"submit\" class=\"{button}\">Show</button></form>",
        bar = parts::bar(),
        links = links.join(" · "),
        site = escape(site),
        control = parts::control(),
        button = parts::button(),
    )
}

fn screen(report: &Report, from: Day, to: Day) -> String {
    let mut html = format!(
        "<section class=\"{card}\"><p class=\"{muted}\">{from} to {to}, UTC</p>\
         <p><strong>{visitors}</strong> visitors · <strong>{views}</strong> page views</p>{chart}</section>",
        card = parts::card(),
        muted = parts::muted(),
        visitors = report.visitors,
        views = report.views,
        chart = chart(report, from, to),
    );
    for dimension in Dimension::ALL {
        html.push_str(&breakdown(dimension, report.breakdown(dimension)));
    }
    html
}

/// One bar per day of the range, a day with no view drawn as an empty
/// slot so the gaps show.
fn chart(report: &Report, from: Day, to: Day) -> String {
    let span = usize::try_from(to.0 - from.0 + 1).unwrap_or(1).max(1);
    let max = report
        .days
        .iter()
        .map(|(_, v, _)| *v)
        .max()
        .unwrap_or(0)
        .max(1);
    // At least one unit per day, so a two-year range still draws a bar for
    // each; the SVG scales to the card either way.
    let (width, height) = (600usize.max(span), 120usize);
    let slot = (width / span).max(1);
    let bar = slot.saturating_sub(1).max(1);
    let mut rects = String::new();
    for (day, views, visitors) in &report.days {
        let i = usize::try_from(day.0 - from.0).unwrap_or(0);
        let h = ((*views as f64 / max as f64) * height as f64)
            .round()
            .max(1.0) as usize;
        rects.push_str(&format!(
            "<rect x=\"{x}\" y=\"{y}\" width=\"{bar}\" height=\"{h}\" fill=\"currentColor\">\
             <title>{day}: {views} views, {visitors} visitors</title></rect>",
            x = i * slot,
            y = height - h,
        ));
    }
    format!(
        "<svg role=\"img\" aria-label=\"Page views per day\" viewBox=\"0 0 {width} {height}\" \
         width=\"100%\" height=\"{height}\" preserveAspectRatio=\"none\">{rects}</svg>"
    )
}

fn breakdown(dimension: Dimension, lines: &[Line]) -> String {
    let title = match dimension {
        Dimension::Path => "Pages",
        Dimension::Referrer => "Referrers",
        Dimension::Browser => "Browsers",
        Dimension::Os => "Systems",
        Dimension::Device => "Devices",
        Dimension::Country => "Countries",
    };
    if lines.is_empty() {
        return format!(
            "<section class=\"{}\"><h2>{title}</h2><p class=\"{}\">None in this range.</p></section>",
            parts::card(),
            parts::empty()
        );
    }
    let rows: String = lines
        .iter()
        .map(|line| {
            format!(
                "<tr><td class=\"{td}\">{label}</td><td class=\"{td}\">{visitors}</td><td class=\"{td}\">{views}</td></tr>",
                td = parts::td(),
                label = escape(&line.label),
                visitors = line.visitors,
                views = line.views,
            )
        })
        .collect();
    format!(
        "<section class=\"{card}\"><h2>{title}</h2><table class=\"{table}\"><thead><tr>\
         <th class=\"{th}\">{title}</th><th class=\"{th}\">Visitors</th><th class=\"{th}\">Views</th>\
         </tr></thead><tbody>{rows}</tbody></table></section>",
        card = parts::card(),
        table = parts::table(),
        th = parts::th(),
    )
}

fn status(recorder: &AnalyticsOverview) -> String {
    let mut lines = vec![format!(
        "{} written since start, {} waiting",
        recorder.written, recorder.pending
    )];
    if recorder.dropped > 0 {
        lines.push(format!("{} dropped: the buffer was full", recorder.dropped));
    }
    if let Some(e) = &recorder.last_error {
        lines.push(format!("last write failed: {}", escape(e)));
    }
    format!("<p class=\"{}\">{}</p>", parts::muted(), lines.join(" · "))
}

/// Percent-encodes a query value: every byte but an unreserved one.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            byte => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}
