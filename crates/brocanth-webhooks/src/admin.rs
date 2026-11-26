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
//!
//! The tab reads and sends again through [`DeliveriesData`]
//! (docs/backoffice.md §7.6), in this process or over the backoffice wire.

use std::sync::Arc;

use async_trait::async_trait;
use brocanth_admin::admin::AdminError;
use brocanth_admin::late::Late;
use brocanth_admin::panel::remote::{PanelServe, PanelWire};
use brocanth_admin::panel::{Panel, PanelPage, PanelRequest};
use brocanth_admin::parts;
use brocanth_admin::policy::{Level, Principal};
use serde::{Deserialize, Serialize};

use crate::outgoing::Outgoing;

/// The tab's key: its RBAC resource, and its name on the backoffice wire.
pub const TAB: &str = "webhooks";

/// Rows per page.
const PAGE: i64 = 50;

/// What the webhooks tab reads and does (docs/backoffice.md §7.6, P1).
///
/// In this process it runs on [`Outgoing`]; [`RemoteDeliveries`] sends it
/// over the backoffice wire, and [`DeliveriesServe`] answers it there.
/// Reading needs `webhooks.read` on the data side, sending again
/// `webhooks.edit`. No endpoint's secret crosses: the view leaves it out.
#[async_trait]
pub trait DeliveriesData: Send + Sync + 'static {
    /// The endpoints, and `limit` deliveries from `offset`, newest first.
    ///
    /// # Errors
    ///
    /// The failure as text, with a `help:` line.
    async fn read(
        &self,
        who: &Principal<'_>,
        limit: i64,
        offset: i64,
    ) -> Result<DeliveriesView, String>;

    /// Sends delivery `id` again.
    ///
    /// # Errors
    ///
    /// The failure as text.
    async fn redeliver(&self, who: &Principal<'_>, id: i64) -> Result<(), String>;
}

/// What the screen shows.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveriesView {
    /// Every endpoint.
    pub endpoints: Vec<EndpointView>,
    /// The deliveries of the page.
    pub deliveries: Vec<DeliveryView>,
}

/// One endpoint, without its secret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointView {
    /// Where deliveries go.
    pub url: String,
    /// The events it wants.
    pub events: Vec<String>,
    /// Whether deliveries go to it.
    pub active: bool,
}

/// One delivery, without its body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryView {
    /// Its row id.
    pub id: i64,
    /// The event type.
    pub event: String,
    /// The endpoint's URL.
    pub url: String,
    /// `pending`, `retrying`, `delivered` or `failed`.
    pub state: String,
    /// Attempts made.
    pub attempts: i32,
    /// The status of the last answer.
    pub last_status: Option<i32>,
    /// What went wrong last.
    pub last_error: Option<String>,
}

/// One call of [`DeliveriesData`], as the wire carries it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeliveriesCall {
    /// [`DeliveriesData::read`]. Needs `read`.
    Read {
        /// The most deliveries.
        limit: i64,
        /// Where the page starts.
        offset: i64,
    },
    /// [`DeliveriesData::redeliver`]. Needs `edit`.
    Redeliver {
        /// The delivery.
        id: i64,
    },
}

/// What a [`DeliveriesCall`] answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeliveriesAnswer {
    /// To `Read`.
    View(DeliveriesView),
    /// To `Redeliver`.
    Done,
}

/// [`Outgoing`] in this process: empty until the first request that
/// carries it.
struct Local(Late<Outgoing>);

#[async_trait]
impl DeliveriesData for Local {
    async fn read(
        &self,
        _who: &Principal<'_>,
        limit: i64,
        offset: i64,
    ) -> Result<DeliveriesView, String> {
        let outgoing = self.outgoing()?;
        let endpoints = outgoing.endpoints().await.map_err(|e| e.0)?;
        let deliveries = outgoing
            .deliveries(limit.clamp(0, PAGE + 1), offset.max(0))
            .await
            .map_err(|e| e.0)?;
        Ok(DeliveriesView {
            endpoints: endpoints
                .into_iter()
                .map(|e| EndpointView {
                    url: e.url,
                    events: e.events,
                    active: e.active,
                })
                .collect(),
            deliveries: deliveries
                .into_iter()
                .map(|row| DeliveryView {
                    id: row.id,
                    event: row.event,
                    url: row.url,
                    state: row.state,
                    attempts: row.attempts,
                    last_status: row.last_status,
                    last_error: row.last_error,
                })
                .collect(),
        })
    }

    async fn redeliver(&self, _who: &Principal<'_>, id: i64) -> Result<(), String> {
        self.outgoing()?
            .redeliver(id)
            .await
            .map(drop)
            .map_err(|e| e.0)
    }
}

impl Local {
    fn outgoing(&self) -> Result<&Outgoing, String> {
        self.0.get().ok_or_else(|| {
            "the webhooks_out plugin is not registered in this application\nhelp: register it, \
             which puts `Outgoing` on the request context"
                .to_string()
        })
    }
}

/// The data side of the webhooks tab on the backoffice wire.
pub struct DeliveriesServe(Arc<dyn DeliveriesData>);

impl DeliveriesServe {
    /// Serves `data`.
    pub fn new(data: Arc<dyn DeliveriesData>) -> DeliveriesServe {
        DeliveriesServe(data)
    }

    /// As the wire server takes it.
    pub fn shared(self) -> Arc<dyn PanelServe> {
        Arc::new(self)
    }
}

impl std::fmt::Debug for DeliveriesServe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DeliveriesServe")
    }
}

fn decode(call: &str) -> Result<DeliveriesCall, String> {
    serde_json::from_str(call).map_err(|e| format!("a webhooks call did not decode: {e}"))
}

#[async_trait]
impl PanelServe for DeliveriesServe {
    fn level(&self, call: &str) -> Result<Level, String> {
        decode(call).map(|call| match call {
            DeliveriesCall::Read { .. } => Level::Read,
            DeliveriesCall::Redeliver { .. } => Level::Edit,
        })
    }

    async fn serve(&self, who: &Principal<'_>, call: &str) -> Result<String, String> {
        let answer = match decode(call)? {
            DeliveriesCall::Read { limit, offset } => {
                DeliveriesAnswer::View(self.0.read(who, limit, offset).await?)
            }
            DeliveriesCall::Redeliver { id } => {
                self.0.redeliver(who, id).await?;
                DeliveriesAnswer::Done
            }
        };
        serde_json::to_string(&answer).map_err(|e| format!("a webhooks answer did not encode: {e}"))
    }
}

/// The webhooks tab's data in another process, through the backoffice
/// wire.
pub struct RemoteDeliveries(Arc<dyn PanelWire>);

impl RemoteDeliveries {
    /// The deliveries behind `wire`.
    pub fn new(wire: Arc<dyn PanelWire>) -> RemoteDeliveries {
        RemoteDeliveries(wire)
    }

    async fn ask(
        &self,
        who: &Principal<'_>,
        call: &DeliveriesCall,
    ) -> Result<DeliveriesAnswer, String> {
        let text = serde_json::to_string(call)
            .map_err(|e| format!("a webhooks call did not encode: {e}"))?;
        let answer = self.0.send(who, TAB, text).await?;
        serde_json::from_str(&answer).map_err(|e| format!("a webhooks answer did not decode: {e}"))
    }
}

impl std::fmt::Debug for RemoteDeliveries {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RemoteDeliveries")
    }
}

#[async_trait]
impl DeliveriesData for RemoteDeliveries {
    async fn read(
        &self,
        who: &Principal<'_>,
        limit: i64,
        offset: i64,
    ) -> Result<DeliveriesView, String> {
        match self
            .ask(who, &DeliveriesCall::Read { limit, offset })
            .await?
        {
            DeliveriesAnswer::View(view) => Ok(view),
            DeliveriesAnswer::Done => Err("the data side answered a read with no rows".into()),
        }
    }

    async fn redeliver(&self, who: &Principal<'_>, id: i64) -> Result<(), String> {
        match self.ask(who, &DeliveriesCall::Redeliver { id }).await? {
            DeliveriesAnswer::Done => Ok(()),
            DeliveriesAnswer::View(_) => Err("the data side answered a write with rows".into()),
        }
    }
}

/// Where the tab reads.
enum Source {
    /// [`Outgoing`], which arrives with the first request.
    Local(Late<Outgoing>),
    /// Any data side: another process, through the wire.
    Data(Arc<dyn DeliveriesData>),
}

/// The deliveries tab.
pub struct DeliveriesPanel {
    source: Source,
}

impl DeliveriesPanel {
    /// A panel over this `Outgoing`, for a test or an application that
    /// builds its own.
    pub fn new(outgoing: Outgoing) -> Self {
        Self::late(Late::ready(outgoing))
    }

    /// A panel over an `Outgoing` that arrives later, in `cell`. A
    /// generated admin mounts its routes before any plugin starts, and
    /// fills the cell from the first request that carries the `Outgoing`.
    pub fn late(cell: Late<Outgoing>) -> Self {
        Self {
            source: Source::Local(cell),
        }
    }

    /// A panel over any [`DeliveriesData`]: [`RemoteDeliveries`] in the
    /// backoffice binary, which reads the deliveries through the wire
    /// (§7.6).
    pub fn over(data: Arc<dyn DeliveriesData>) -> Self {
        Self {
            source: Source::Data(data),
        }
    }

    /// What the tab reads through, or `None` before `Outgoing` arrived.
    fn data(&self) -> Option<Arc<dyn DeliveriesData>> {
        match &self.source {
            Source::Local(cell) => cell
                .get()
                .is_some()
                .then(|| Arc::new(Local(cell.clone())) as Arc<dyn DeliveriesData>),
            Source::Data(data) => Some(Arc::clone(data)),
        }
    }
}

fn failed(e: String) -> AdminError {
    AdminError::Data(brocanth_admin::DataError::Backend(e))
}

#[async_trait]
impl Panel for DeliveriesPanel {
    fn key(&self) -> &str {
        TAB
    }

    fn label(&self) -> String {
        "Webhooks".to_string()
    }

    fn serve(&self) -> Option<Arc<dyn PanelServe>> {
        match &self.source {
            Source::Local(cell) => {
                Some(DeliveriesServe::new(Arc::new(Local(cell.clone()))).shared())
            }
            Source::Data(_) => None,
        }
    }

    fn remote(&self, wire: Arc<dyn PanelWire>) -> Option<Arc<dyn Panel>> {
        Some(Arc::new(DeliveriesPanel::over(Arc::new(
            RemoteDeliveries::new(wire),
        ))))
    }

    async fn render(&self, r: PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError> {
        let Some(data) = self.data() else {
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
                data.redeliver(&r.principal, id).await.map_err(failed)?;
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
        // A page past the end of i64 is past the end of the log: an empty
        // page, not an overflow.
        let view = match (page - 1).checked_mul(PAGE) {
            Some(offset) => data
                .read(&r.principal, PAGE + 1, offset)
                .await
                .map_err(failed)?,
            None => {
                let mut view = data.read(&r.principal, 0, 0).await.map_err(failed)?;
                view.deliveries.clear();
                view
            }
        };
        Ok(Some(PanelPage::body(
            "Webhooks",
            screen(&r, &view.endpoints, &view.deliveries, page),
        )))
    }
}

fn screen(
    r: &PanelRequest<'_>,
    endpoints: &[EndpointView],
    rows: &[DeliveryView],
    page: i64,
) -> String {
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
