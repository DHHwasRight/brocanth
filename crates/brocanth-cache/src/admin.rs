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
use brocanth_admin::panel::{Panel, PanelPage, PanelRequest};
use brocanth_admin::view::render;

use crate::cache::Cache;
use crate::error::CacheError;
use crate::key::Namespace;

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

/// The cache as a tab.
///
/// ```ignore
/// AdminConfig::new().panel(CachePanel::new(cache.clone()).namespace(PAGES.clone()))
/// ```
pub struct CachePanel {
    cache: Arc<dyn Cache>,
    namespaces: Vec<Namespace>,
    purge_limit: u64,
    key: String,
    label: String,
}

impl CachePanel {
    /// The panel, at `cache`, labelled "Cache", with nothing clearable yet.
    pub fn new(cache: Arc<dyn Cache>) -> Self {
        Self {
            cache,
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

    // ------------------------------------------------------------- screen

    fn index(&self, r: &PanelRequest<'_>) -> Result<PanelPage, AdminError> {
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
            max_value_bytes: bytes(self.cache.max_value_bytes()),
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
                    .cache
                    .purge_expired(self.purge_limit)
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
                let removed = self.cache.clear(namespace).await.map_err(failed)?;
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

    async fn render(&self, r: PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError> {
        if r.post {
            return self.act(&r).await;
        }
        match r.path.is_empty() {
            true => self.index(&r).map(Some),
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
fn failed(e: CacheError) -> AdminError {
    AdminError::Render(rsx::Error::custom(format!("the cache failed: {e}")))
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
