//! The connections, as a tab in `brocanth-admin`.
//!
//! One screen: every connection, by account then provider, with its scopes,
//! its expiry and whether a refresh token is kept. No token value is ever
//! read for this screen: the store lists [`Connection`]s, which hold none.
//! A "Revoke" button posts the account and the provider; it revokes at the
//! provider when it can and deletes the row. The admin asks its policy for
//! `ActOnPanel` before this code runs, and a reader who may not act sees no
//! button.
//!
//! The tab reads and revokes through [`ConnectionsData`]
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

use crate::web::escape;
use crate::{Connection, OauthClient};

/// The tab's key: its RBAC resource, and its name on the backoffice wire.
pub const TAB: &str = "connections";

/// Rows per page, unless [`ConnectionsPanel::per_page`] says otherwise.
pub const PER_PAGE: u32 = 50;

/// The most rows one call reads, whatever it asks for.
const MAX_PAGE: u32 = 500;

/// What the connections tab reads and does (docs/backoffice.md §7.6, P1).
///
/// In this process it runs on the [`OauthClient`]; [`RemoteConnections`]
/// sends it over the backoffice wire, and [`ConnectionsServe`] answers it
/// there. Reading needs `connections.read` on the data side, revoking
/// `connections.edit`. A [`Connection`] holds no token, so none crosses.
#[async_trait]
pub trait ConnectionsData: Send + Sync + 'static {
    /// `limit` connections from `offset`, and how many there are.
    ///
    /// # Errors
    ///
    /// The failure as text, with a `help:` line.
    async fn page(
        &self,
        who: &Principal<'_>,
        limit: u32,
        offset: u32,
    ) -> Result<ConnectionsPage, String>;

    /// Revokes `account`'s connection to `provider`.
    ///
    /// # Errors
    ///
    /// The failure as text.
    async fn revoke(
        &self,
        who: &Principal<'_>,
        account: &str,
        provider: &str,
    ) -> Result<(), String>;
}

/// One page of connections.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionsPage {
    /// How many connections there are.
    pub total: u64,
    /// This page's.
    pub rows: Vec<Connection>,
}

/// One call of [`ConnectionsData`], as the wire carries it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionsCall {
    /// [`ConnectionsData::page`]. Needs `read`.
    Page {
        /// The most rows.
        limit: u32,
        /// Where the page starts.
        offset: u32,
    },
    /// [`ConnectionsData::revoke`]. Needs `edit`.
    Revoke {
        /// The account.
        account: String,
        /// The provider id.
        provider: String,
    },
}

/// What a [`ConnectionsCall`] answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionsAnswer {
    /// To `Page`.
    Page(ConnectionsPage),
    /// To `Revoke`.
    Done,
}

/// The client in this process: empty until the first request that carries
/// it.
struct Local(Late<OauthClient>);

impl Local {
    fn client(&self) -> Result<&OauthClient, String> {
        self.0.get().ok_or_else(|| {
            "the oauth_client plugin is not registered in this application\nhelp: register it, \
             which puts the client on the request context"
                .to_string()
        })
    }
}

#[async_trait]
impl ConnectionsData for Local {
    async fn page(
        &self,
        _who: &Principal<'_>,
        limit: u32,
        offset: u32,
    ) -> Result<ConnectionsPage, String> {
        let client = self.client()?;
        Ok(ConnectionsPage {
            total: client
                .count_connections()
                .await
                .map_err(|e| e.to_string())?,
            rows: client
                .all_connections(limit.min(MAX_PAGE), offset)
                .await
                .map_err(|e| e.to_string())?,
        })
    }

    async fn revoke(
        &self,
        _who: &Principal<'_>,
        account: &str,
        provider: &str,
    ) -> Result<(), String> {
        self.client()?
            .disconnect(account, provider)
            .await
            .map(drop)
            .map_err(|e| e.to_string())
    }
}

/// The data side of the connections tab on the backoffice wire.
pub struct ConnectionsServe(Arc<dyn ConnectionsData>);

impl ConnectionsServe {
    /// Serves `data`.
    pub fn new(data: Arc<dyn ConnectionsData>) -> ConnectionsServe {
        ConnectionsServe(data)
    }

    /// As the wire server takes it.
    pub fn shared(self) -> Arc<dyn PanelServe> {
        Arc::new(self)
    }
}

impl std::fmt::Debug for ConnectionsServe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConnectionsServe")
    }
}

fn decode(call: &str) -> Result<ConnectionsCall, String> {
    serde_json::from_str(call).map_err(|e| format!("a connections call did not decode: {e}"))
}

#[async_trait]
impl PanelServe for ConnectionsServe {
    fn level(&self, call: &str) -> Result<Level, String> {
        decode(call).map(|call| match call {
            ConnectionsCall::Page { .. } => Level::Read,
            ConnectionsCall::Revoke { .. } => Level::Edit,
        })
    }

    async fn serve(&self, who: &Principal<'_>, call: &str) -> Result<String, String> {
        let answer = match decode(call)? {
            ConnectionsCall::Page { limit, offset } => {
                ConnectionsAnswer::Page(self.0.page(who, limit, offset).await?)
            }
            ConnectionsCall::Revoke { account, provider } => {
                self.0.revoke(who, &account, &provider).await?;
                ConnectionsAnswer::Done
            }
        };
        serde_json::to_string(&answer)
            .map_err(|e| format!("a connections answer did not encode: {e}"))
    }
}

/// The connections tab's data in another process, through the backoffice
/// wire.
pub struct RemoteConnections(Arc<dyn PanelWire>);

impl RemoteConnections {
    /// The connections behind `wire`.
    pub fn new(wire: Arc<dyn PanelWire>) -> RemoteConnections {
        RemoteConnections(wire)
    }

    async fn ask(
        &self,
        who: &Principal<'_>,
        call: &ConnectionsCall,
    ) -> Result<ConnectionsAnswer, String> {
        let text = serde_json::to_string(call)
            .map_err(|e| format!("a connections call did not encode: {e}"))?;
        let answer = self.0.send(who, TAB, text).await?;
        serde_json::from_str(&answer)
            .map_err(|e| format!("a connections answer did not decode: {e}"))
    }
}

impl std::fmt::Debug for RemoteConnections {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RemoteConnections")
    }
}

#[async_trait]
impl ConnectionsData for RemoteConnections {
    async fn page(
        &self,
        who: &Principal<'_>,
        limit: u32,
        offset: u32,
    ) -> Result<ConnectionsPage, String> {
        match self
            .ask(who, &ConnectionsCall::Page { limit, offset })
            .await?
        {
            ConnectionsAnswer::Page(page) => Ok(page),
            ConnectionsAnswer::Done => Err("the data side answered a read with no rows".into()),
        }
    }

    async fn revoke(
        &self,
        who: &Principal<'_>,
        account: &str,
        provider: &str,
    ) -> Result<(), String> {
        let call = ConnectionsCall::Revoke {
            account: account.to_string(),
            provider: provider.to_string(),
        };
        match self.ask(who, &call).await? {
            ConnectionsAnswer::Done => Ok(()),
            ConnectionsAnswer::Page(_) => Err("the data side answered a write with rows".into()),
        }
    }
}

/// Where the tab reads.
enum Source {
    /// The client, which arrives with the first request.
    Local(Late<OauthClient>),
    /// Any data side: another process, through the wire.
    Data(Arc<dyn ConnectionsData>),
}

/// The connections tab.
pub struct ConnectionsPanel {
    source: Source,
    per_page: u32,
}

impl ConnectionsPanel {
    /// A panel over this client, for a test or an application that builds
    /// its own.
    pub fn new(client: OauthClient) -> Self {
        Self::late(Late::ready(client))
    }

    /// A panel over a client that arrives later, in `cell`. A generated
    /// admin mounts its routes before any plugin starts, and fills the cell
    /// from the first request that carries the client.
    pub fn late(cell: Late<OauthClient>) -> Self {
        Self {
            source: Source::Local(cell),
            per_page: PER_PAGE,
        }
    }

    /// A panel over any [`ConnectionsData`]: [`RemoteConnections`] in the
    /// backoffice binary, which reads the connections through the wire
    /// (§7.6).
    pub fn over(data: Arc<dyn ConnectionsData>) -> Self {
        Self {
            source: Source::Data(data),
            per_page: PER_PAGE,
        }
    }

    /// How many connections a page shows. Zero is treated as one. The same
    /// builder as `brocanth-auth`'s `AccountsPanel::per_page`.
    #[must_use]
    pub fn per_page(mut self, per_page: u32) -> Self {
        self.per_page = per_page.max(1);
        self
    }

    /// What the tab reads through, or `None` before the client arrived.
    fn data(&self) -> Option<Arc<dyn ConnectionsData>> {
        match &self.source {
            Source::Local(cell) => cell
                .get()
                .is_some()
                .then(|| Arc::new(Local(cell.clone())) as Arc<dyn ConnectionsData>),
            Source::Data(data) => Some(Arc::clone(data)),
        }
    }
}

fn failed(e: String) -> AdminError {
    AdminError::Data(brocanth_admin::DataError::Backend(e))
}

#[async_trait]
impl Panel for ConnectionsPanel {
    fn key(&self) -> &str {
        TAB
    }

    fn label(&self) -> String {
        "Connections".to_string()
    }

    fn serve(&self) -> Option<Arc<dyn PanelServe>> {
        match &self.source {
            Source::Local(cell) => {
                Some(ConnectionsServe::new(Arc::new(Local(cell.clone()))).shared())
            }
            Source::Data(_) => None,
        }
    }

    fn remote(&self, wire: Arc<dyn PanelWire>) -> Option<Arc<dyn Panel>> {
        Some(Arc::new(
            ConnectionsPanel::over(Arc::new(RemoteConnections::new(wire))).per_page(self.per_page),
        ))
    }

    async fn render(&self, r: PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError> {
        let Some(data) = self.data() else {
            return Ok(Some(PanelPage::body(
                "Connections",
                format!(
                    "<p class=\"{}\">The oauth_client plugin is not registered in this application.</p>",
                    parts::muted()
                ),
            )));
        };
        if r.post {
            if r.path != "revoke" {
                return Ok(None);
            }
            if r.can_act {
                if let (Some(account), Some(provider)) =
                    (r.submission.get("account"), r.submission.get("provider"))
                {
                    data.revoke(&r.principal, account, provider)
                        .await
                        .map_err(failed)?;
                }
            }
            return Ok(Some(PanelPage::redirect(r.url(""))));
        }
        if !r.path.is_empty() {
            return Ok(None);
        }
        let page = r
            .param("page")
            .and_then(|p| p.parse::<u32>().ok())
            .unwrap_or(1)
            .max(1);
        let found = data
            .page(
                &r.principal,
                self.per_page,
                (page - 1).saturating_mul(self.per_page),
            )
            .await
            .map_err(failed)?;
        let last = found.total.div_ceil(u64::from(self.per_page)).max(1);
        Ok(Some(PanelPage::body(
            "Connections",
            screen(&r, &found.rows, page, u64::from(page) < last),
        )))
    }
}

fn screen(r: &PanelRequest<'_>, rows: &[Connection], page: u32, more: bool) -> String {
    if rows.is_empty() && page == 1 {
        return format!(
            "<p class=\"{}\">Nobody has connected an account yet.</p>",
            parts::empty()
        );
    }
    let mut html = format!(
        "<table class=\"{}\"><thead><tr><th class=\"{th}\">Account</th><th class=\"{th}\">Provider</th><th class=\"{th}\">Scopes</th><th class=\"{th}\">Expires</th><th class=\"{th}\">Refresh</th><th class=\"{th}\"></th></tr></thead><tbody>",
        parts::table(),
        th = parts::th()
    );
    for c in rows {
        let button = if r.can_act {
            format!(
                "<form method=\"post\" action=\"{}\"><input type=\"hidden\" name=\"account\" value=\"{}\"><input type=\"hidden\" name=\"provider\" value=\"{}\"><button type=\"submit\">Revoke</button></form>",
                escape(&r.url("revoke")),
                escape(&c.account),
                escape(&c.provider)
            )
        } else {
            String::new()
        };
        html.push_str(&format!(
            "<tr><td class=\"{td}\">{}</td><td class=\"{td}\">{}</td><td class=\"{td}\">{}</td><td class=\"{td}\">{}</td><td class=\"{td}\">{}</td><td class=\"{td}\">{}</td></tr>",
            escape(&c.account),
            escape(&c.provider),
            escape(&c.scopes.join(" ")),
            c.expires_at.map_or_else(|| "never".to_string(), |at| at.to_string()),
            if c.has_refresh { "yes" } else { "no" },
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
    if more {
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
