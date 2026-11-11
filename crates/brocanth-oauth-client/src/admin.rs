//! The connections, as a tab in `brocanth-admin`.
//!
//! One screen: every connection, by account then provider, with its scopes,
//! its expiry and whether a refresh token is kept. No token value is ever
//! read for this screen: the store lists [`Connection`]s, which hold none.
//! A "Revoke" button posts the account and the provider; it revokes at the
//! provider when it can and deletes the row. The admin asks its policy for
//! `ActOnPanel` before this code runs, and a reader who may not act sees no
//! button.

use async_trait::async_trait;
use brocanth_admin::admin::AdminError;
use brocanth_admin::panel::{Panel, PanelPage, PanelRequest};
use brocanth_admin::parts;

use crate::web::escape;
use crate::{Connection, OauthClient};

/// Rows per page.
const PAGE: usize = 50;

/// The connections tab.
pub struct ConnectionsPanel {
    client: Option<OauthClient>,
}

impl ConnectionsPanel {
    /// A panel over this client, for a test or an application that builds
    /// its own.
    pub fn new(client: OauthClient) -> Self {
        Self {
            client: Some(client),
        }
    }

    /// A panel over the client the plugin installs. A generated admin mounts
    /// its routes before any plugin starts, so the panel looks when it
    /// renders.
    pub fn installed() -> Self {
        Self { client: None }
    }

    fn client(&self) -> Option<OauthClient> {
        self.client.clone().or_else(OauthClient::installed)
    }
}

fn failed(e: crate::Error) -> AdminError {
    AdminError::Data(brocanth_admin::DataError::Backend(e.to_string()))
}

#[async_trait]
impl Panel for ConnectionsPanel {
    fn key(&self) -> &str {
        "connections"
    }

    fn label(&self) -> String {
        "Connections".to_string()
    }

    async fn render(&self, r: PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError> {
        let Some(client) = self.client() else {
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
                    client.disconnect(account, provider).await.map_err(failed)?;
                }
            }
            return Ok(Some(PanelPage::redirect(r.url(""))));
        }
        if !r.path.is_empty() {
            return Ok(None);
        }
        let page = r
            .param("page")
            .and_then(|p| p.parse::<usize>().ok())
            .filter(|p| *p >= 1)
            .unwrap_or(1);
        let rows = client
            .all_connections(PAGE + 1, (page - 1) * PAGE)
            .await
            .map_err(failed)?;
        Ok(Some(PanelPage::body(
            "Connections",
            screen(&r, &rows, page),
        )))
    }
}

fn screen(r: &PanelRequest<'_>, rows: &[Connection], page: usize) -> String {
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
    for c in rows.iter().take(PAGE) {
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
    if rows.len() > PAGE {
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
