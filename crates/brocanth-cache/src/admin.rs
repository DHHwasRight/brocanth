//! The cache, as a tab in `brocanth-admin`.
//!
//! One screen, two buttons, and no numbers.
//!
//! # Why there are no numbers
//!
//! [`Cache`] has [`clear`](Cache::clear) and
//! [`purge_expired`](Cache::purge_expired), and it has no `stats()`. A panel
//! holds an `Arc<dyn Cache>` and can therefore only call what the trait
//! declares, so there is no entry count to draw, no hit rate, and no size on
//! disk.
//!
//! The screen says that out loud rather than filling the space. A dashboard
//! that invents a figure is worse than a screen that admits it has none: the
//! invented one gets believed, and the admission is a to-do item somebody can
//! act on. `docs/backoffice.md` §5 is the to-do item.
//!
//! What the screen *can* report is what the two buttons did, because both
//! operations answer with a row count. So the tab has no gauge and does have
//! an outcome, which is the honest half of a dashboard.
//!
//! # Why clearing is a declared list and not a text box
//!
//! The trait cannot enumerate namespaces either — a namespace is a prefix the
//! caller chose, not a record the cache keeps. So the application names the
//! ones this tab may empty, with [`CachePanel::namespace`], and the panel
//! refuses anything else. A free text box would be a button that empties a
//! namespace nobody admitted to owning, and would turn a typo into silence
//! rather than into an error.
//!
//! # The data contract
//!
//! The tab reads and writes through [`CacheData`] (docs/backoffice.md
//! §7.6): in this process over the cache, or over the backoffice wire
//! ([`RemoteCache`], answered by [`CacheServe`]). Reading needs
//! `<key>.read` on the data side, and both buttons `<key>.edit`. The data
//! side checks the namespace against the declared list and caps a purge at
//! the tab's limit, so a call that names another namespace is refused
//! there too.
//!
//! # Both buttons are a POST, and both are behind the policy
//!
//! Emptying a cache is a change, so the buttons are forms rather than links:
//! a crawler, a prefetching browser or a pasted URL must not be able to fire
//! them. The admin asks the policy for
//! [`Action::ActOnPanel`](brocanth_admin::policy::Action::ActOnPanel) before
//! this code runs, and a reader who may look but not act is shown the same
//! screen with no buttons on it.

use std::sync::Arc;

use async_trait::async_trait;
use brocanth_admin::admin::AdminError;
use brocanth_admin::late::Late;
use brocanth_admin::panel::remote::{PanelServe, PanelWire};
use brocanth_admin::panel::{Panel, PanelPage, PanelRequest};
use brocanth_admin::policy::{Level, Principal};
use brocanth_admin::view::render;
use serde::{Deserialize, Serialize};

use crate::cache::Cache;
use crate::error::CacheError;
use crate::key::Namespace;
use crate::plugin::CacheHandle;

/// The panel's stylesheet, built from `style/panel.css`. Inlined by the
/// screen. See `style/README.md`.
pub(crate) const STYLE: &str = include_str!("admin.css");

/// How many expired rows one press of the button removes.
///
/// Bounded, and bounded low enough to be a statement rather than a
/// maintenance window: an operator pressing a button in a browser should not
/// start a delete that holds locks for a minute. [`crate::sweep`] is the
/// thing that runs until the table is clean, and it belongs in a recurring
/// job rather than behind a button.
pub const DEFAULT_PURGE_LIMIT: u64 = 1_000;

/// What the cache tab reads and writes (docs/backoffice.md §7.6, P1).
///
/// In this process it runs on the cache; [`RemoteCache`] sends it over the
/// backoffice wire, and [`CacheServe`] answers it there. `who` is the
/// request's principal: the wire turns it into the claim the data side
/// checks.
#[async_trait]
pub trait CacheData: Send + Sync + 'static {
    /// What the screen shows of the cache.
    ///
    /// # Errors
    ///
    /// The failure as text, with a `help:` line.
    async fn read(&self, who: &Principal<'_>) -> Result<CacheView, String>;

    /// Empties `namespace`, one the tab declared, and answers how many
    /// entries went.
    ///
    /// # Errors
    ///
    /// The failure as text: an undeclared namespace, or the cache's error.
    async fn clear(&self, who: &Principal<'_>, namespace: &str) -> Result<u64, String>;

    /// Removes at most `limit` expired entries, and answers how many went.
    ///
    /// # Errors
    ///
    /// The failure as text.
    async fn purge(&self, who: &Principal<'_>, limit: u64) -> Result<u64, String>;
}

/// What the screen shows of the cache.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheView {
    /// The largest value the cache stores, in bytes.
    pub max_value_bytes: usize,
}

/// One call of [`CacheData`], as the wire carries it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheCall {
    /// [`CacheData::read`]. Needs `read`.
    Read,
    /// [`CacheData::clear`]. Needs `edit`.
    Clear {
        /// The namespace.
        namespace: String,
    },
    /// [`CacheData::purge`]. Needs `edit`.
    Purge {
        /// The most entries to remove.
        limit: u64,
    },
}

impl CacheCall {
    /// The level the call needs on the tab.
    pub fn level(&self) -> Level {
        match self {
            CacheCall::Read => Level::Read,
            CacheCall::Clear { .. } | CacheCall::Purge { .. } => Level::Edit,
        }
    }
}

/// What a [`CacheCall`] answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheAnswer {
    /// To `Read`.
    View(CacheView),
    /// To `Clear` and `Purge`: how many entries went.
    Removed(u64),
}

/// The cache in this process, held to what the tab declared: its
/// namespaces and its purge limit.
struct Declared {
    cache: Local,
    namespaces: Vec<Namespace>,
    purge_limit: u64,
}

/// Where a tab in this process finds its cache.
#[derive(Clone)]
enum Local {
    /// Handed over at build time.
    Given(Arc<dyn Cache>),
    /// A cell the application fills at start.
    Late(Late<CacheHandle>),
}

impl Declared {
    fn cache(&self) -> Result<Arc<dyn Cache>, String> {
        match &self.cache {
            Local::Given(cache) => Ok(Arc::clone(cache)),
            Local::Late(cell) => cell
                .get()
                .map(|handle| Arc::clone(handle.inner()))
                .ok_or_else(|| {
                    "this tab has no cache yet: no request with it on its context has come\n\
                     help: register the cache plugin, which puts its handle on the request \
                     context"
                        .to_string()
                }),
        }
    }
}

fn cache_failed(e: CacheError) -> String {
    format!("the cache failed: {e}")
}

#[async_trait]
impl CacheData for Declared {
    async fn read(&self, _who: &Principal<'_>) -> Result<CacheView, String> {
        Ok(CacheView {
            max_value_bytes: self.cache()?.max_value_bytes(),
        })
    }

    async fn clear(&self, _who: &Principal<'_>, namespace: &str) -> Result<u64, String> {
        let Some(namespace) = self.namespaces.iter().find(|n| n.as_str() == namespace) else {
            return Err(format!(
                "the cache tab declares no namespace `{namespace}`\nhelp: declare it with \
                 `CachePanel::namespace`"
            ));
        };
        self.cache()?.clear(namespace).await.map_err(cache_failed)
    }

    async fn purge(&self, _who: &Principal<'_>, limit: u64) -> Result<u64, String> {
        self.cache()?
            .purge_expired(limit.min(self.purge_limit))
            .await
            .map_err(cache_failed)
    }
}

/// The data side of the cache tab on the backoffice wire: decodes a
/// [`CacheCall`] and runs it on a [`CacheData`].
pub struct CacheServe(Arc<dyn CacheData>);

impl CacheServe {
    /// Serves `data`.
    pub fn new(data: Arc<dyn CacheData>) -> CacheServe {
        CacheServe(data)
    }

    /// As the wire server takes it.
    pub fn shared(self) -> Arc<dyn PanelServe> {
        Arc::new(self)
    }
}

impl std::fmt::Debug for CacheServe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CacheServe")
    }
}

fn decode(call: &str) -> Result<CacheCall, String> {
    serde_json::from_str(call).map_err(|e| format!("a cache call did not decode: {e}"))
}

#[async_trait]
impl PanelServe for CacheServe {
    fn level(&self, call: &str) -> Result<Level, String> {
        decode(call).map(|call| call.level())
    }

    async fn serve(&self, who: &Principal<'_>, call: &str) -> Result<String, String> {
        let answer = match decode(call)? {
            CacheCall::Read => CacheAnswer::View(self.0.read(who).await?),
            CacheCall::Clear { namespace } => {
                CacheAnswer::Removed(self.0.clear(who, &namespace).await?)
            }
            CacheCall::Purge { limit } => CacheAnswer::Removed(self.0.purge(who, limit).await?),
        };
        serde_json::to_string(&answer).map_err(|e| format!("a cache answer did not encode: {e}"))
    }
}

/// The cache tab's data in another process, through the backoffice wire.
pub struct RemoteCache {
    wire: Arc<dyn PanelWire>,
    tab: String,
}

impl RemoteCache {
    /// The cache behind `wire`, for the tab at `tab`.
    pub fn new(wire: Arc<dyn PanelWire>, tab: impl Into<String>) -> RemoteCache {
        RemoteCache {
            wire,
            tab: tab.into(),
        }
    }

    async fn ask(&self, who: &Principal<'_>, call: &CacheCall) -> Result<CacheAnswer, String> {
        let text =
            serde_json::to_string(call).map_err(|e| format!("a cache call did not encode: {e}"))?;
        let answer = self.wire.send(who, &self.tab, text).await?;
        serde_json::from_str(&answer).map_err(|e| format!("a cache answer did not decode: {e}"))
    }

    async fn removed(&self, who: &Principal<'_>, call: CacheCall) -> Result<u64, String> {
        match self.ask(who, &call).await? {
            CacheAnswer::Removed(n) => Ok(n),
            CacheAnswer::View(_) => Err("the data side answered a write with a view".into()),
        }
    }
}

impl std::fmt::Debug for RemoteCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteCache")
            .field("tab", &self.tab)
            .finish()
    }
}

#[async_trait]
impl CacheData for RemoteCache {
    async fn read(&self, who: &Principal<'_>) -> Result<CacheView, String> {
        match self.ask(who, &CacheCall::Read).await? {
            CacheAnswer::View(view) => Ok(view),
            CacheAnswer::Removed(_) => Err("the data side answered a read with a count".into()),
        }
    }

    async fn clear(&self, who: &Principal<'_>, namespace: &str) -> Result<u64, String> {
        let namespace = namespace.to_string();
        self.removed(who, CacheCall::Clear { namespace }).await
    }

    async fn purge(&self, who: &Principal<'_>, limit: u64) -> Result<u64, String> {
        self.removed(who, CacheCall::Purge { limit }).await
    }
}

/// The cache as a tab.
///
/// ```ignore
/// AdminConfig::new().panel(CachePanel::new(cache.clone()).namespace(PAGES.clone()))
/// ```
pub struct CachePanel {
    source: Source,
    namespaces: Vec<Namespace>,
    purge_limit: u64,
    key: String,
    label: String,
}

/// Where the panel finds its cache.
enum Source {
    /// In this process.
    Local(Local),
    /// Any data side: another process, through the wire.
    Data(Arc<dyn CacheData>),
}

impl CachePanel {
    /// The panel, at `cache`, labelled "Cache", with nothing clearable yet.
    pub fn new(cache: Arc<dyn Cache>) -> Self {
        Self::from_source(Source::Local(Local::Given(cache)))
    }

    /// The panel over a cache that arrives later, in `cell`.
    ///
    /// For an admin built before the cache opens: a generated project
    /// builds its admin with the route table, and its cache plugin opens
    /// the cache at `before_start`, after that. The admin fills the cell
    /// from the first request that carries the [`CacheHandle`]. Until then
    /// the tab says so.
    pub fn late(cell: Late<CacheHandle>) -> Self {
        Self::from_source(Source::Local(Local::Late(cell)))
    }

    /// The panel over any [`CacheData`]: [`RemoteCache`] in the backoffice
    /// binary, which reads the cache through the wire (§7.6).
    pub fn over(data: Arc<dyn CacheData>) -> Self {
        Self::from_source(Source::Data(data))
    }

    fn from_source(source: Source) -> Self {
        Self {
            source,
            namespaces: Vec::new(),
            purge_limit: DEFAULT_PURGE_LIMIT,
            key: "cache".to_string(),
            label: "Cache".to_string(),
        }
    }

    /// A namespace this tab may empty.
    ///
    /// Name every namespace the application owns. One that is not named has
    /// no button and cannot be cleared through the admin even by hand, which
    /// is the point: the trait cannot list namespaces, so this list is the
    /// only statement anywhere about which ones exist.
    #[must_use]
    pub fn namespace(mut self, namespace: Namespace) -> Self {
        if !self.namespaces.contains(&namespace) {
            self.namespaces.push(namespace);
        }
        self
    }

    /// Several namespaces at once.
    #[must_use]
    pub fn namespaces(mut self, namespaces: impl IntoIterator<Item = Namespace>) -> Self {
        for namespace in namespaces {
            self = self.namespace(namespace);
        }
        self
    }

    /// How many expired rows one press removes. See [`DEFAULT_PURGE_LIMIT`].
    #[must_use]
    pub fn purge_limit(mut self, limit: u64) -> Self {
        self.purge_limit = limit;
        self
    }

    /// A different key, for an application that already has a `cache` model.
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

    /// What the tab reads and writes through.
    fn data(&self) -> Arc<dyn CacheData> {
        match &self.source {
            Source::Local(cache) => Arc::new(Declared {
                cache: cache.clone(),
                namespaces: self.namespaces.clone(),
                purge_limit: self.purge_limit,
            }),
            Source::Data(data) => Arc::clone(data),
        }
    }

    // ------------------------------------------------------------- screen

    async fn index(&self, r: &PanelRequest<'_>) -> Result<PanelPage, AdminError> {
        let view = self.data().read(&r.principal).await.map_err(failed)?;
        let ctx = IndexCtx {
            can_act: r.can_act,
            notice: self.notice(r),
            rows: self
                .namespaces
                .iter()
                .map(|namespace| NamespaceRow {
                    name: namespace.as_str().to_string(),
                    clear_href: r.url(&format!("clear/{}", namespace.as_str())),
                })
                .collect(),
            purge_href: r.url("purge"),
            purge_limit: self.purge_limit,
            max_value_bytes: bytes(view.max_value_bytes),
        };
        let html = render(
            &|ctx: &IndexCtx, w: &mut rsx::Writer<'_>| {
                use rsx::Template as _;
                crate::admin_templates::index::Index { ctx }.render(w)
            },
            &ctx,
        )?;
        Ok(PanelPage::body("Cache", html))
    }

    /// What the last button did, read back off the redirect.
    ///
    /// The admin has no flash and a panel has no session, so the outcome
    /// travels in the query string. Every part of it is checked before it
    /// reaches the page: the namespace must be one this panel declared, and
    /// the count must parse as a number. A panel that echoed its own query
    /// string would be putting whatever a link said in front of an operator.
    fn notice(&self, r: &PanelRequest<'_>) -> String {
        let rows = r.param("rows").and_then(|n| n.parse::<u64>().ok());
        if let (Some(name), Some(rows)) = (r.param("cleared"), rows) {
            let known = self.namespaces.iter().any(|n| n.as_str() == name);
            if known {
                return format!("Cleared `{name}`: {} removed.", entries(rows));
            }
            return String::new();
        }
        match r.param("purged").and_then(|n| n.parse::<u64>().ok()) {
            None => String::new(),
            Some(removed) if removed >= self.purge_limit => format!(
                "Purged {}, which is the whole batch. There may be more: press it again.",
                entries(removed)
            ),
            Some(removed) => format!("Purged {}. Nothing expired is left.", entries(removed)),
        }
    }

    // ------------------------------------------------------------ buttons

    /// Every POST. The admin has already asked the policy whether this
    /// person may press a button here, so what is left is which button.
    async fn act(&self, r: &PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError> {
        match r.path.split_once('/') {
            None if r.path == "purge" => {
                let removed = self
                    .data()
                    .purge(&r.principal, self.purge_limit)
                    .await
                    .map_err(failed)?;
                Ok(Some(PanelPage::redirect(format!(
                    "{}?purged={removed}",
                    r.url("")
                ))))
            }
            Some(("clear", name)) => {
                // Only a namespace the application declared. The admin
                // refuses a POST from somebody who may not act; this
                // refuses one naming a namespace nobody owns, which a
                // policy cannot express and a text box would invite.
                let Some(namespace) = self.namespaces.iter().find(|n| n.as_str() == name) else {
                    return Ok(None);
                };
                let removed = self
                    .data()
                    .clear(&r.principal, namespace.as_str())
                    .await
                    .map_err(failed)?;
                Ok(Some(PanelPage::redirect(format!(
                    "{}?cleared={name}&rows={removed}",
                    r.url("")
                ))))
            }
            _ => Ok(None),
        }
    }
}

#[async_trait]
impl Panel for CachePanel {
    fn key(&self) -> &str {
        &self.key
    }

    fn label(&self) -> String {
        self.label.clone()
    }

    fn serve(&self) -> Option<Arc<dyn PanelServe>> {
        match &self.source {
            Source::Local(_) => Some(CacheServe::new(self.data()).shared()),
            Source::Data(_) => None,
        }
    }

    fn remote(&self, wire: Arc<dyn PanelWire>) -> Option<Arc<dyn Panel>> {
        let data = Arc::new(RemoteCache::new(wire, self.key.clone()));
        Some(Arc::new(Self {
            source: Source::Data(data),
            namespaces: self.namespaces.clone(),
            purge_limit: self.purge_limit,
            key: self.key.clone(),
            label: self.label.clone(),
        }))
    }

    async fn render(&self, r: PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError> {
        if r.post {
            return self.act(&r).await;
        }
        match r.path.is_empty() {
            true => self.index(&r).await.map(Some),
            false => Ok(None),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The cache's errors are not the admin's, and the admin's error type has no
/// arm for them. They arrive as a render failure carrying the text, which is
/// what a screen can actually show somebody.
fn failed(e: String) -> AdminError {
    AdminError::Render(rsx::Error::custom(e))
}

/// "1 entry" or "12 entries". A sentence, not a column.
fn entries(n: u64) -> String {
    match n {
        1 => "1 entry".to_string(),
        n => format!("{n} entries"),
    }
}

/// "1 MiB", for the one figure the trait does hand over.
fn bytes(n: usize) -> String {
    const MIB: usize = 1024 * 1024;
    const KIB: usize = 1024;
    match n {
        n if n >= MIB && n % MIB == 0 => format!("{} MiB", n / MIB),
        n if n >= KIB && n % KIB == 0 => format!("{} KiB", n / KIB),
        n => format!("{n} bytes"),
    }
}

// ---------------------------------------------------------------------------
// What the template is given
// ---------------------------------------------------------------------------

/// One namespace the application declared.
pub struct NamespaceRow {
    /// Its name.
    pub name: String,
    /// Where the Clear button posts.
    pub clear_href: String,
}

/// The screen.
pub struct IndexCtx {
    /// Whether to draw the buttons.
    pub can_act: bool,
    /// What the last button did, or empty.
    pub notice: String,
    /// One row per declared namespace.
    pub rows: Vec<NamespaceRow>,
    /// Where the Purge button posts.
    pub purge_href: String,
    /// How many expired rows one press removes.
    pub purge_limit: u64,
    /// The largest value this cache stores, as "1 MiB".
    pub max_value_bytes: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_count_is_said_in_the_right_singular() {
        assert_eq!(entries(0), "0 entries");
        assert_eq!(entries(1), "1 entry");
        assert_eq!(entries(12), "12 entries");
    }

    #[test]
    fn the_value_cap_is_said_in_the_unit_it_was_set_in() {
        assert_eq!(bytes(1024 * 1024), "1 MiB");
        assert_eq!(bytes(64 * 1024), "64 KiB");
        assert_eq!(bytes(1500), "1500 bytes");
    }
}
