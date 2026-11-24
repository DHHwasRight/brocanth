//! The **Impersonate** tab: every account, with a form that starts acting
//! as it.
//!
//! The tab only lists. The form posts to a route of the application
//! (`/impersonate` by default), because only a page can set the session
//! cookie and a panel answers with a body or a redirect. That route asks
//! [`Staff::impersonate`](crate::Staff::impersonate), which checks the
//! permission, refuses a staff subject and needs an elevated session.
//!
//! The tab's RBAC resource is `impersonate`: `impersonate.read` shows it,
//! `impersonate.edit` draws the forms, and the route asks for the same
//! `impersonate.edit` ([`Staff::IMPERSONATE`](crate::Staff::IMPERSONATE)).

use std::sync::Arc;

use async_trait::async_trait;
use brocanth_admin::admin::AdminError;
use brocanth_admin::late::Late;
use brocanth_admin::panel::{Panel, PanelPage, PanelRequest};
use brocanth_admin::view::render;

use crate::storage::AccountDirectory;

/// The tab's key and RBAC resource.
pub const IMPERSONATE_KEY: &str = "impersonate";

const PER_PAGE: u32 = 50;

/// Impersonation as a tab.
pub struct ImpersonatePanel<S: AccountDirectory> {
    store: Late<Arc<S>>,
    start: String,
}

impl<S: AccountDirectory> ImpersonatePanel<S> {
    /// The tab over a store the application already holds, posting to
    /// `/impersonate`.
    pub fn new(store: Arc<S>) -> Self {
        Self::late(Late::ready(store))
    }

    /// The tab over a store that arrives later, in `cell`.
    pub fn late(cell: Late<Arc<S>>) -> Self {
        Self {
            store: cell,
            start: "/impersonate".to_string(),
        }
    }

    /// Where the forms post: the application's route that starts an
    /// impersonation.
    #[must_use]
    pub fn starting_at(mut self, path: impl Into<String>) -> Self {
        self.start = path.into();
        self
    }
}

#[async_trait]
impl<S: AccountDirectory> Panel for ImpersonatePanel<S> {
    fn key(&self) -> &str {
        IMPERSONATE_KEY
    }

    fn label(&self) -> String {
        "Impersonate".to_string()
    }

    async fn render(&self, r: PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError> {
        if r.post || !r.path.is_empty() {
            return Ok(None);
        }
        let store = self.store.require("accounts store")?;
        let total = store.count_accounts().await.map_err(super::failed)?;
        let page = r
            .param("page")
            .and_then(|p| p.parse::<u32>().ok())
            .unwrap_or(1)
            .max(1);
        let offset = (page - 1).saturating_mul(PER_PAGE);
        let accounts = store
            .accounts(PER_PAGE, offset)
            .await
            .map_err(super::failed)?;
        let last = total.div_ceil(u64::from(PER_PAGE)).max(1);
        let ctx = ImpersonateCtx {
            volatile: !store.is_durable(),
            can_act: r.can_act,
            start: self.start.clone(),
            rows: accounts
                .iter()
                .map(|a| SubjectRow {
                    id: a.id.to_string(),
                    email: super::address_of(a),
                })
                .collect(),
            summary: format!("Page {page} of {last}"),
            previous: (page > 1).then(|| format!("{}?page={}", r.url(""), page - 1)),
            next: (u64::from(page) < last).then(|| format!("{}?page={}", r.url(""), page + 1)),
        };
        let view = |ctx: &ImpersonateCtx, w: &mut rsx::Writer<'_>| {
            use rsx::Template as _;
            crate::admin_templates::impersonate::Impersonate { ctx }.render(w)
        };
        Ok(Some(PanelPage::body("Impersonate", render(&view, &ctx)?)))
    }
}

/// One account the tab lists.
pub struct SubjectRow {
    /// The account's identifier, the form's `id`.
    pub id: String,
    /// The address, or the words that say there is not one.
    pub email: String,
}

/// What the template is given.
pub struct ImpersonateCtx {
    /// Whether these accounts go when the process does.
    pub volatile: bool,
    /// Whether to draw the forms.
    pub can_act: bool,
    /// Where the forms post.
    pub start: String,
    /// The accounts of this page.
    pub rows: Vec<SubjectRow>,
    /// "Page 2 of 9".
    pub summary: String,
    /// The page before, when there is one.
    pub previous: Option<String>,
    /// The page after, when there is one.
    pub next: Option<String>,
}
