//! The outgoing deliveries, as a tab in `brocanth-admin`.
//!
//! One screen: the endpoints, then the newest deliveries with their state,
//! attempts and last answer. A delivery that failed or is waiting has a
//! "Send again" button, which is a POST: a crawler or a pasted link must not
//! send a webhook. The admin asks its policy for `ActOnPanel` before this
//! code runs, and a reader who may not act sees no buttons.
//!
//! Lives here, not in the admin, for the reason the jobs panel does: the
//! admin has no SQL driver, and the crate that owns the rows owns the
//! screen, behind an `admin` feature that is off.

use async_trait::async_trait;
use brocanth_admin::admin::AdminError;
use brocanth_admin::panel::{Panel, PanelPage, PanelRequest};
use brocanth_admin::parts;

use crate::outgoing::{DeliveryRow, Endpoint, Outgoing};

/// Rows per page.
const PAGE: i64 = 50;

/// The deliveries tab.
pub struct DeliveriesPanel {
    outgoing: Option<Outgoing>,
}

impl DeliveriesPanel {
    /// A panel over this `Outgoing`, for a test or an application that
    /// builds its own.
    pub fn new(outgoing: Outgoing) -> Self {
        Self {
            outgoing: Some(outgoing),
        }
    }

    /// A panel over the `Outgoing` the plugin installs. A generated admin
    /// mounts its routes before any plugin starts, so the panel looks when
    /// it renders.
    pub fn installed() -> Self {
        Self { outgoing: None }
    }

    fn outgoing(&self) -> Option<Outgoing> {
        self.outgoing.clone().or_else(Outgoing::installed)
    }
}

#[async_trait]
impl Panel for DeliveriesPanel {
    fn key(&self) -> &str {
        "webhooks"
    }

    fn label(&self) -> String {
        "Webhooks".to_string()
    }

    async fn render(&self, r: PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError> {
        let Some(outgoing) = self.outgoing() else {
            return Ok(Some(PanelPage::body(
                "Webhooks",
                format!(
                    "<p class=\"{}\">The webhooks_out plugin is not registered in this application.</p>",
                    parts::muted()
                ),
            )));
        };
        if r.post {
            let Some(id) = r
                .path
                .strip_prefix("redeliver/")
                .and_then(|id| id.parse::<i64>().ok())
            else {
                return Ok(None);
            };
            if r.can_act {
                outgoing
                    .redeliver(id)
                    .await
                    .map_err(|e| AdminError::Data(brocanth_admin::DataError::Backend(e.0)))?;
            }
            return Ok(Some(PanelPage::redirect(r.url(""))));
        }
        if !r.path.is_empty() {
            return Ok(None);
        }
        let page = r
            .param("page")
            .and_then(|p| p.parse::<i64>().ok())
            .filter(|p| *p >= 1)
            .unwrap_or(1);
        let failed = |e: crate::OutError| AdminError::Data(brocanth_admin::DataError::Backend(e.0));
        let endpoints = outgoing.endpoints().await.map_err(failed)?;
        let rows = outgoing
            .deliveries(PAGE + 1, (page - 1) * PAGE)
            .await
            .map_err(failed)?;
        Ok(Some(PanelPage::body(
            "Webhooks",
            screen(&r, &endpoints, &rows, page),
        )))
    }
}

fn screen(r: &PanelRequest<'_>, endpoints: &[Endpoint], rows: &[DeliveryRow], page: i64) -> String {
    let mut html = String::new();
    html.push_str(&format!(
        "<h2 class=\"{}\">Endpoints</h2>",
        parts::heading()
    ));
    if endpoints.is_empty() {
        html.push_str(&format!(
            "<p class=\"{}\">No endpoint yet. Add one with <code>Outgoing::add_endpoint</code>.</p>",
            parts::empty()
        ));
    } else {
        html.push_str(&format!(
            "<table class=\"{}\"><thead><tr><th class=\"{th}\">URL</th><th class=\"{th}\">Events</th><th class=\"{th}\">State</th></tr></thead><tbody>",
            parts::table(),
            th = parts::th()
        ));
        for e in endpoints {
            html.push_str(&format!(
                "<tr><td class=\"{td}\">{}</td><td class=\"{td}\">{}</td><td class=\"{td}\">{}</td></tr>",
                escape(&e.url),
                escape(&e.events.join(", ")),
                if e.active { "on" } else { "off" },
                td = parts::td()
            ));
        }
        html.push_str("</tbody></table>");
    }
    html.push_str(&format!(
        "<h2 class=\"{}\">Deliveries</h2>",
        parts::heading()
    ));
    if rows.is_empty() {
        html.push_str(&format!(
            "<p class=\"{}\">Nothing was sent yet.</p>",
            parts::empty()
        ));
        return html;
    }
    html.push_str(&format!(
        "<table class=\"{}\"><thead><tr><th class=\"{th}\">#</th><th class=\"{th}\">Event</th><th class=\"{th}\">To</th><th class=\"{th}\">State</th><th class=\"{th}\">Attempts</th><th class=\"{th}\">Last answer</th><th class=\"{th}\"></th></tr></thead><tbody>",
        parts::table(),
        th = parts::th()
    ));
    for row in rows.iter().take(PAGE as usize) {
        let last = match (&row.last_status, &row.last_error) {
            (_, Some(error)) => escape(error),
            (Some(status), None) => status.to_string(),
            (None, None) => String::new(),
        };
        let button = if r.can_act && row.state != "delivered" {
            format!(
                "<form method=\"post\" action=\"{}\"><button type=\"submit\">Send again</button></form>",
                escape(&r.url(&format!("redeliver/{}", row.id)))
            )
        } else {
            String::new()
        };
        html.push_str(&format!(
            "<tr><td class=\"{td}\">{}</td><td class=\"{td}\">{}</td><td class=\"{td}\">{}</td><td class=\"{td}\">{}</td><td class=\"{td}\">{}</td><td class=\"{td}\">{}</td><td class=\"{td}\">{}</td></tr>",
            row.id,
            escape(&row.event),
            escape(&row.url),
            escape(&row.state),
            row.attempts,
            last,
            button,
            td = parts::td()
        ));
    }
    html.push_str("</tbody></table>");
    let mut links = Vec::new();
    if page > 1 {
        links.push(format!(
            "<a href=\"{}?page={}\">Newer</a>",
            escape(&r.url("")),
            page - 1
        ));
    }
    if rows.len() as i64 > PAGE {
        links.push(format!(
            "<a href=\"{}?page={}\">Older</a>",
            escape(&r.url("")),
            page + 1
        ));
    }
    if !links.is_empty() {
        html.push_str(&format!(
            "<p class=\"{}\">{}</p>",
            parts::bar(),
            links.join(" ")
        ));
    }
    html
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
