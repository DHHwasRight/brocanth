//! The numbers, as a tab in `brocanth-admin`.
//!
//! One read-only screen: visitors and views over 7, 30 or 90 days, a bar
//! per day, and the top pages, referrers, browsers, systems, devices and
//! countries. Rendered on the server, with no script.
//!
//! The bars are SVG `<rect>`s with `width` and `height` attributes, not
//! `style="width: …"`: a presentation attribute is not an inline style, so
//! the screen needs nothing from the Content-Security-Policy.
//!
//! Locked like every tab: reading it is `Action::ViewPanel` on `analytics`,
//! which the admin's shipped `DenyAll` refuses until the application says
//! who may.

use async_trait::async_trait;
use brocanth_admin::admin::AdminError;
use brocanth_admin::panel::{Panel, PanelPage, PanelRequest};
use brocanth_admin::parts;

use crate::day::Day;
use crate::plugin::Analytics;
use crate::recorder::Recorder;
use crate::store::{Dimension, Line, Report};

/// The ranges the screen offers, in days.
const RANGES: [u32; 3] = [7, 30, 90];

/// The analytics tab.
pub struct AnalyticsPanel {
    source: Source,
}

enum Source {
    Recorder(Recorder),
    Installed,
}

impl AnalyticsPanel {
    /// A tab over this recorder.
    pub fn new(recorder: Recorder) -> Self {
        Self {
            source: Source::Recorder(recorder),
        }
    }

    /// A tab over [`Analytics::installed`], resolved when it renders: for a
    /// generated admin, which mounts before the plugin is built.
    pub fn installed() -> Self {
        Self {
            source: Source::Installed,
        }
    }

    fn recorder(&self) -> Option<Recorder> {
        match &self.source {
            Source::Recorder(r) => Some(r.clone()),
            Source::Installed => Analytics::installed(),
        }
    }
}

#[async_trait]
impl Panel for AnalyticsPanel {
    fn key(&self) -> &str {
        "analytics"
    }

    fn label(&self) -> String {
        "Analytics".to_string()
    }

    async fn render(&self, r: PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError> {
        if r.post || !r.path.is_empty() {
            return Ok(None);
        }
        let Some(recorder) = self.recorder() else {
            return Ok(Some(PanelPage::body(
                "Analytics",
                format!(
                    "<p class=\"{}\">The analytics plugin is not registered in this application.</p>",
                    parts::muted()
                ),
            )));
        };
        let today = recorder.today();
        let (from, to, current) = range(&r, today);
        let mut html = String::new();
        html.push_str(&ranges(&r, current, from, to));
        match recorder.report(from, to).await {
            Ok(report) => html.push_str(&screen(&report, from, to)),
            Err(e) => html.push_str(&format!(
                "<p class=\"{}\">The report failed: {}</p>",
                parts::muted(),
                escape(&e.to_string())
            )),
        }
        html.push_str(&status(&recorder));
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

/// The three presets as links, then a form for any two dates. The form is
/// a GET, so it changes nothing and needs no token.
fn ranges(r: &PanelRequest<'_>, current: Option<u32>, from: Day, to: Day) -> String {
    let links: Vec<String> = RANGES
        .iter()
        .map(|&d| {
            let label = format!("{d} days");
            if Some(d) == current {
                format!("<strong>{label}</strong>")
            } else {
                format!(
                    "<a class=\"{}\" href=\"{}?days={d}\">{label}</a>",
                    parts::action(),
                    escape(&r.url(""))
                )
            }
        })
        .collect();
    format!(
        "<p class=\"{bar}\">{links}</p>\
         <form method=\"get\" action=\"{action}\" class=\"{bar}\">\
         <label>From <input type=\"date\" name=\"from\" value=\"{from}\" class=\"{control}\" required></label> \
         <label>To <input type=\"date\" name=\"to\" value=\"{to}\" class=\"{control}\" required></label> \
         <button type=\"submit\" class=\"{button}\">Show</button></form>",
        bar = parts::bar(),
        links = links.join(" · "),
        action = escape(&r.url("")),
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

fn status(recorder: &Recorder) -> String {
    let mut lines = vec![format!(
        "{} written since start, {} waiting",
        recorder.written(),
        recorder.pending()
    )];
    if recorder.dropped() > 0 {
        lines.push(format!(
            "{} dropped: the buffer was full",
            recorder.dropped()
        ));
    }
    if let Some(e) = recorder.last_error() {
        lines.push(format!("last write failed: {}", escape(&e)));
    }
    format!("<p class=\"{}\">{}</p>", parts::muted(), lines.join(" · "))
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
