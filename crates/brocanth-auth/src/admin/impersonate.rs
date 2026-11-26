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
//!
//! The list reads through [`SubjectsData`] (docs/backoffice.md §7.6), in
//! this process or over the backoffice wire. The start itself never
//! crosses it: it is the application's route, on the application's host.

use std::sync::Arc;

use async_trait::async_trait;
use brocanth_admin::admin::AdminError;
use brocanth_admin::late::Late;
use brocanth_admin::panel::remote::{PanelServe, PanelWire};
use brocanth_admin::panel::{Panel, PanelPage, PanelRequest};
use brocanth_admin::policy::{Level, Principal};
use brocanth_admin::view::render;
use serde::{Deserialize, Serialize};

use crate::storage::AccountDirectory;

/// The tab's key and RBAC resource.
pub const IMPERSONATE_KEY: &str = "impersonate";

const PER_PAGE: u32 = 50;

/// What the impersonate tab reads (docs/backoffice.md §7.6, P1): one page
/// of accounts. In this process it reads the store; [`RemoteSubjects`]
/// sends it over the backoffice wire, and [`SubjectsServe`] answers it
/// there with `impersonate.read`.
#[async_trait]
pub trait SubjectsData: Send + Sync + 'static {
    /// The accounts from `offset`, at most `limit`, and how many there are.
    ///
    /// # Errors
    ///
    /// The failure as text, with a `help:` line.
    async fn page(&self, who: &Principal<'_>, limit: u32, offset: u32) -> Result<Subjects, String>;
}

/// One page of accounts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subjects {
    /// Whether the accounts stay when the process stops.
    pub durable: bool,
    /// How many accounts there are.
    pub total: u64,
    /// This page's accounts: the id, and the address or the words that say
    /// there is none.
    pub accounts: Vec<(String, String)>,
}

/// One call of [`SubjectsData`], as the wire carries it. Needs `read`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubjectsCall {
    /// The most accounts.
    pub limit: u32,
    /// Where the page starts.
    pub offset: u32,
}

/// The store in this process: empty until the first request that carries
/// it.
struct Directory<S>(Late<Arc<S>>);

#[async_trait]
impl<S: AccountDirectory> SubjectsData for Directory<S> {
    async fn page(
        &self,
        _who: &Principal<'_>,
        limit: u32,
        offset: u32,
    ) -> Result<Subjects, String> {
        let store = self.0.get().ok_or_else(|| {
            "this tab has no accounts store yet: no request with it on its context has come\n\
             help: register the auth plugin, which puts its store on the request context"
                .to_string()
        })?;
        let failed = |e: crate::storage::StoreError| format!("the account store failed: {e}");
        let total = store.count_accounts().await.map_err(failed)?;
        let accounts = store
            .accounts(limit.min(PER_PAGE), offset)
            .await
            .map_err(failed)?;
        Ok(Subjects {
            durable: store.is_durable(),
            total,
            accounts: accounts
                .iter()
                .map(|a| (a.id.to_string(), super::address_of(a)))
                .collect(),
        })
    }
}

/// The data side of the impersonate tab on the backoffice wire.
pub struct SubjectsServe(Arc<dyn SubjectsData>);

impl SubjectsServe {
    /// Serves `data`.
    pub fn new(data: Arc<dyn SubjectsData>) -> SubjectsServe {
        SubjectsServe(data)
    }

    /// As the wire server takes it.
    pub fn shared(self) -> Arc<dyn PanelServe> {
        Arc::new(self)
    }
}

impl std::fmt::Debug for SubjectsServe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SubjectsServe")
    }
}

fn decode(call: &str) -> Result<SubjectsCall, String> {
    serde_json::from_str(call).map_err(|e| format!("an impersonate call did not decode: {e}"))
}

#[async_trait]
impl PanelServe for SubjectsServe {
    fn level(&self, call: &str) -> Result<Level, String> {
        decode(call).map(|_| Level::Read)
    }

    async fn serve(&self, who: &Principal<'_>, call: &str) -> Result<String, String> {
        let SubjectsCall { limit, offset } = decode(call)?;
        let page = self.0.page(who, limit, offset).await?;
        serde_json::to_string(&page)
            .map_err(|e| format!("an impersonate answer did not encode: {e}"))
    }
}

/// The impersonate tab's data in another process, through the backoffice
/// wire.
pub struct RemoteSubjects(Arc<dyn PanelWire>);

impl RemoteSubjects {
    /// The accounts behind `wire`.
    pub fn new(wire: Arc<dyn PanelWire>) -> RemoteSubjects {
        RemoteSubjects(wire)
    }
}

impl std::fmt::Debug for RemoteSubjects {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RemoteSubjects")
    }
}

#[async_trait]
impl SubjectsData for RemoteSubjects {
    async fn page(&self, who: &Principal<'_>, limit: u32, offset: u32) -> Result<Subjects, String> {
        let call = serde_json::to_string(&SubjectsCall { limit, offset })
            .map_err(|e| format!("an impersonate call did not encode: {e}"))?;
        let answer = self.0.send(who, IMPERSONATE_KEY, call).await?;
        serde_json::from_str(&answer)
            .map_err(|e| format!("an impersonate answer did not decode: {e}"))
    }
}

/// Where the tab reads its accounts.
enum Source<S> {
    /// The store, which arrives with the first request.
    Local(Late<Arc<S>>),
    /// Any data side: another process, through the wire.
    Data(Arc<dyn SubjectsData>),
}

/// Impersonation as a tab.
pub struct ImpersonatePanel<S: AccountDirectory> {
    source: Source<S>,
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
            source: Source::Local(cell),
            start: "/impersonate".to_string(),
        }
    }

    /// The tab over any [`SubjectsData`]: [`RemoteSubjects`] in the
    /// backoffice binary, which reads the accounts through the wire (§7.6).
    /// The forms still post to the application's route.
    pub fn over(data: Arc<dyn SubjectsData>) -> Self {
        Self {
            source: Source::Data(data),
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

    fn data(&self) -> Arc<dyn SubjectsData> {
        match &self.source {
            Source::Local(cell) => Arc::new(Directory(cell.clone())),
            Source::Data(data) => Arc::clone(data),
        }
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

    fn serve(&self) -> Option<Arc<dyn PanelServe>> {
        match &self.source {
            Source::Local(_) => Some(SubjectsServe::new(self.data()).shared()),
            Source::Data(_) => None,
        }
    }

    fn remote(&self, wire: Arc<dyn PanelWire>) -> Option<Arc<dyn Panel>> {
        Some(Arc::new(
            ImpersonatePanel::<S>::over(Arc::new(RemoteSubjects::new(wire)))
                .starting_at(self.start.clone()),
        ))
    }

    async fn render(&self, r: PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError> {
        if r.post || !r.path.is_empty() {
            return Ok(None);
        }
        let page = r
            .param("page")
            .and_then(|p| p.parse::<u32>().ok())
            .unwrap_or(1)
            .max(1);
        let offset = (page - 1).saturating_mul(PER_PAGE);
        let subjects = self
            .data()
            .page(&r.principal, PER_PAGE, offset)
            .await
            .map_err(|e| AdminError::Render(rsx::Error::custom(e)))?;
        let last = subjects.total.div_ceil(u64::from(PER_PAGE)).max(1);
        let ctx = ImpersonateCtx {
            volatile: !subjects.durable,
            can_act: r.can_act,
            start: self.start.clone(),
            rows: subjects
                .accounts
                .into_iter()
                .map(|(id, email)| SubjectRow { id, email })
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
