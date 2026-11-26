//! A tab the admin did not write.
//!
//! The four screens are about *a table*: list it, read a row, edit a row,
//! delete a row. Plenty of what belongs in a backoffice is not a table. A
//! job queue has a dashboard, a retry set and a kill button; a feature flag
//! has a switch; a cache has one button that empties it. None of those is a
//! row in a model and none of them should be bent into one.
//!
//! A [`Panel`] is a tab that renders its own body and is given the admin's
//! chrome for free.
//!
//! # Why the panel lives in the other crate
//!
//! This crate has four dependencies and no SQL driver, no ORM, no serde. A
//! jobs panel written here would drag the queue into every application that
//! has an admin, and a jobs crate that depended on this one would drag rsx
//! into every worker process. So the trait is here and the panel is in the
//! crate that owns the data, behind a feature that is off -- the same shape
//! as a `scaffold` module living beside the crate it scaffolds.
//!
//! # What a panel may not do
//!
//! It renders a body and returns it. It cannot replace the layout, reach
//! the policy, see other panels, or touch a model. A panel that shows the
//! values of the admin's own records -- the change history does -- asks
//! [`PanelRequest::access`] per record, and the admin answers with its own
//! policy and row rules. A panel that wants the
//! whole page is an application replacing [`Views::layout`](crate::Views),
//! which is a different and already-supported thing.
//!
//! # It is locked like everything else
//!
//! The admin ships [`DenyAll`](crate::policy::DenyAll) and a panel does not
//! get an exemption. Reading a tab is [`Action::ViewPanel`], and pressing a
//! button on one is [`Action::ActOnPanel`]; the subject of both is the
//! panel's key. Two actions and not one, because "may look at the queue" and
//! "may empty the dead set" are different answers in every organisation that
//! has ever asked the question.
//!
//! # An extra page of the application's own
//!
//! Django's `AdminSite.get_urls()` adds a view with the admin's chrome. A
//! panel is that view, so there is no second concept: an application adds
//! one with [`AdminConfig::page`](crate::AdminConfig::page), a closure
//! wrapped in [`Page`], or implements [`Panel`] when the page awaits
//! something. [`Page::hidden`] and [`Panel::in_nav`] keep it out of the
//! sidebar.
//!
//! [`Action::ViewPanel`]: crate::policy::Action::ViewPanel
//! [`Action::ActOnPanel`]: crate::policy::Action::ActOnPanel

use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;

use crate::admin::AdminError;
use crate::form::Submission;
use crate::model::{PrimaryKey, Row};
use crate::policy::Principal;

/// A tab contributed by another crate.
#[async_trait]
pub trait Panel: Send + Sync {
    /// The URL segment and the policy subject.
    ///
    /// Lower case, no slashes. A panel and a model claiming one key is a
    /// conflict [`Admin::new`](crate::Admin) refuses, naming both, rather
    /// than a silent precedence rule nobody can see.
    fn key(&self) -> &str;

    /// The tab's label.
    fn label(&self) -> String;

    /// Whether the sidebar shows a tab for it. `true` by default.
    ///
    /// `false` is a page reached by a link only: a report a list screen
    /// links to, a wizard step. The policy guards it all the same.
    fn in_nav(&self) -> bool {
        true
    }

    /// Renders one request.
    ///
    /// `Ok(None)` is this panel's own 404: a sub-path it does not serve.
    /// The admin turns it into the same not-found page a bad model key
    /// gets, so a panel does not have to render one.
    async fn render(&self, request: PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError>;

    /// The data side of this tab on the backoffice wire, over the data this
    /// tab reads in this process (docs/backoffice.md §7.6). `None` by
    /// default: the tab has no data contract, or it reads another process
    /// already.
    ///
    /// The application's wire server asks each tab of the admin for it
    /// ([`AdminConfig::panel_serves`](crate::AdminConfig::panel_serves)),
    /// so a tab that can move serves itself with no line per tab.
    fn serve(&self) -> Option<std::sync::Arc<dyn remote::PanelServe>> {
        None
    }

    /// This tab, reading its data through `wire` instead of in this
    /// process. `None` by default: the tab cannot cross the wire, and it
    /// keeps reading in the process that renders it.
    ///
    /// [`AdminConfig::panels_through`](crate::AdminConfig::panels_through)
    /// asks every tab once, when the admin is built.
    fn remote(
        &self,
        wire: std::sync::Arc<dyn remote::PanelWire>,
    ) -> Option<std::sync::Arc<dyn Panel>> {
        let _ = wire;
        None
    }
}

/// Whether the person on a request may read one of the admin's records.
///
/// The admin hands one to every panel on [`PanelRequest::access`]. A panel
/// that prints the values of a record of another table asks it first, so
/// the panel's own key does not grant a read of every table.
#[async_trait]
pub trait RecordAccess: Send + Sync {
    /// Whether this person may read record `key` of `table`, values
    /// included: the screen policy allows
    /// [`Action::ViewDetail`](crate::policy::Action::ViewDetail) on its
    /// model, and the model's view rule admits the row.
    ///
    /// `key` is the stored value, `,` between the parts of a composite key
    /// ([`PrimaryKey::to_text`]). `false` for a table the admin does not
    /// show, and for a row that is gone, unless the view rule admits every
    /// row.
    async fn may_read(&self, table: &str, key: &str) -> bool;
}

/// One request, as a panel sees it.
///
/// The path is already relative to the panel, because a panel should not
/// have to know where the admin is mounted or what it is called. `""` is
/// the panel's own index.
pub struct PanelRequest<'a> {
    /// `""` for the panel's index, `"retries"` for `{base}/{key}/retries`.
    pub path: &'a str,
    /// The query string, without the `?`.
    pub query: &'a str,
    /// Whether this is a POST. A panel that changes something must only do
    /// it here, and the admin has already asked the policy about
    /// [`Action::ActOnPanel`](crate::policy::Action::ActOnPanel).
    pub post: bool,
    /// Whether this person may press this panel's buttons.
    ///
    /// Told, not asked: a panel never reaches the policy. The admin
    /// answers [`Action::ActOnPanel`](crate::policy::Action::ActOnPanel)
    /// on every request, not only on a POST, because a reader who may not
    /// act should be shown a screen with no buttons on it rather than
    /// buttons that 403.
    ///
    /// It is a courtesy and not the enforcement. A panel that ignores it
    /// and draws the buttons anyway is still safe, because the POST those
    /// buttons make is refused before the panel runs.
    pub can_act: bool,
    /// The form body, for a POST.
    pub submission: &'a Submission,
    /// Where this panel is mounted, so a panel can write its own links:
    /// `{base}/{key}`, with no trailing slash.
    pub mount: &'a str,
    /// What this person may read of the admin's records. The admin always
    /// sets it; `None` is a request built by hand, such as a test, and a
    /// panel then shows no record's values.
    pub access: Option<&'a dyn RecordAccess>,
    /// Who is asking, as the application spells it: the value the policy
    /// was given. Opaque here, as it is to the admin.
    ///
    /// For a tab whose buttons depend on who presses them, past the
    /// policy's yes: the roles tab lets an owner of `jobs` give
    /// `jobs.read`, and nothing else. The admin has already asked the
    /// policy about this tab; a panel narrows that answer, never widens it.
    pub principal: Principal<'a>,
}

impl std::fmt::Debug for PanelRequest<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PanelRequest")
            .field("path", &self.path)
            .field("query", &self.query)
            .field("post", &self.post)
            .field("can_act", &self.can_act)
            .field("submission", &self.submission)
            .field("mount", &self.mount)
            .field("access", &self.access.is_some())
            .field("principal", &self.principal)
            .finish()
    }
}

impl PanelRequest<'_> {
    /// Whether this person may read record `key` of `table`. `false` when
    /// the request carries no [`RecordAccess`]. See [`RecordAccess::may_read`].
    pub async fn may_read(&self, table: &str, key: &str) -> bool {
        match self.access {
            Some(access) => access.may_read(table, key).await,
            None => false,
        }
    }

    /// A link to a sub-path of this panel.
    pub fn url(&self, path: &str) -> String {
        match path.is_empty() {
            true => self.mount.to_string(),
            false => format!("{}/{}", self.mount, path.trim_start_matches('/')),
        }
    }

    /// One query parameter, percent-decoded.
    pub fn param(&self, name: &str) -> Option<String> {
        crate::query::param(self.query, name)
    }
}

/// What a panel produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelPage {
    /// A body, wrapped in the admin's layout.
    Body {
        /// The `<h1>` and the `<title>`.
        heading: String,
        /// The markup, already escaped by whoever rendered it.
        html: String,
    },
    /// A redirect, for after a button. 303, so a reload does not repost.
    Redirect {
        /// Where to. Build it with [`PanelRequest::url`].
        location: String,
    },
}

impl PanelPage {
    /// A body page.
    pub fn body(heading: impl Into<String>, html: impl Into<String>) -> Self {
        PanelPage::Body {
            heading: heading.into(),
            html: html.into(),
        }
    }

    /// A redirect page.
    pub fn redirect(location: impl Into<String>) -> Self {
        PanelPage::Redirect {
            location: location.into(),
        }
    }
}

/// What an async page under a model returns. Build one with `Box::pin`.
pub type PageFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<PanelPage>, AdminError>> + Send + 'a>>;

/// One request of a page under a model or a record, as an async page sees
/// it: the request, the record, and the writes that keep the admin's rules.
///
/// See [`ModelAdmin::page_async`](crate::ModelAdmin::page_async). The admin
/// asked the policy before it built this; [`PageCx::update`] and
/// [`PageCx::delete`] ask it again for the record they write, as an action
/// over selected rows does.
pub struct PageCx<'a> {
    pub(crate) admin: &'a crate::admin::Admin,
    pub(crate) model: &'a str,
    pub(crate) principal: crate::policy::Principal<'a>,
    pub(crate) request: PanelRequest<'a>,
    pub(crate) key: Option<&'a PrimaryKey>,
    pub(crate) record: Option<&'a Row>,
}

impl<'a> PageCx<'a> {
    /// The request: the path below the page, the query, the POST and its
    /// form, `can_act` and `mount`.
    pub fn request(&self) -> &PanelRequest<'a> {
        &self.request
    }

    /// The record, read under the view rule, on a page under a record.
    /// `None` on a page under the model.
    pub fn record(&self) -> Option<&'a Row> {
        self.record
    }

    /// The record's key, on a page under a record.
    pub fn key(&self) -> Option<&'a PrimaryKey> {
        self.key
    }

    /// The model key.
    pub fn model(&self) -> &str {
        self.model
    }
}

/// Renders one request of a [`Page`].
pub type PageFn =
    Box<dyn Fn(&PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError> + Send + Sync>;

/// A panel from a closure: the application's own page with the admin's
/// chrome. [`AdminConfig::page`](crate::AdminConfig::page) builds one.
///
/// The closure is synchronous. It renders from what it captured and from
/// the request. A page that reads a database implements [`Panel`], whose
/// `render` is async.
pub struct Page {
    key: String,
    label: String,
    in_nav: bool,
    render: PageFn,
}

impl std::fmt::Debug for Page {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Page")
            .field("key", &self.key)
            .field("label", &self.label)
            .field("in_nav", &self.in_nav)
            .finish()
    }
}

impl Page {
    /// A page at `{base}/{key}` and every path under it, with a tab named
    /// `label`.
    pub fn new<F>(key: impl Into<String>, label: impl Into<String>, render: F) -> Self
    where
        F: Fn(&PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError> + Send + Sync + 'static,
    {
        Page {
            key: key.into(),
            label: label.into(),
            in_nav: true,
            render: Box::new(render),
        }
    }

    /// Keeps the page out of the sidebar. A link reaches it.
    pub fn hidden(mut self) -> Self {
        self.in_nav = false;
        self
    }
}

#[async_trait]
impl Panel for Page {
    fn key(&self) -> &str {
        &self.key
    }

    fn label(&self) -> String {
        self.label.clone()
    }

    fn in_nav(&self) -> bool {
        self.in_nav
    }

    async fn render(&self, request: PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError> {
        (self.render)(&request)
    }
}

/// A panel's data, sent to another process as JSON text
/// (docs/backoffice.md §7.6, P2).
///
/// A panel crate that can move to the backoffice service puts its data
/// behind a trait of its own, with owned serde types. For the wire it
/// implements [`PanelServe`](remote::PanelServe) over that trait (the data
/// side), and that trait over any [`PanelWire`](remote::PanelWire) (the
/// backoffice's end). `brocanth-backoffice` carries the text between the
/// two. The text is JSON, so this crate needs no serde.
pub mod remote {
    use std::collections::HashMap;
    use std::sync::Arc;

    use async_trait::async_trait;

    use crate::policy::{Level, Principal};

    /// What a caller holds, as the data side checks it: the superuser flag
    /// and the permission strings (`jobs.read`). The backoffice wire builds
    /// it from the call's claim.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Grants {
        /// Holds every level on every tab.
        pub superuser: bool,
        /// Permission strings, `<resource>.<level>`.
        pub permissions: Vec<String>,
    }

    impl Grants {
        /// Whether this holds `level` on `tab`: a superuser holds every
        /// level, and a higher level holds the lower ones.
        pub fn holds(&self, tab: &str, level: Level) -> bool {
            self.superuser
                || Level::ALL
                    .into_iter()
                    .filter(|held| *held >= level)
                    .any(|held| self.permissions.iter().any(|p| *p == held.on(tab)))
        }

        /// The resources this owns (`jobs` for `jobs.owner`).
        pub fn owned(&self) -> impl Iterator<Item = &str> {
            self.permissions
                .iter()
                .filter_map(|p| p.strip_suffix(".owner"))
        }
    }

    /// Checks that `grants` may run `call` on `tab`'s data, before the data
    /// side runs it. The wire server and [`Loopback`] both call it, so a
    /// test over the loopback checks what the wire checks.
    ///
    /// # Errors
    ///
    /// The refusal, or a call that does not decode, as text with a `help:`
    /// line.
    pub fn authorize(
        serve: &dyn PanelServe,
        tab: &str,
        call: &str,
        grants: &Grants,
    ) -> Result<Level, String> {
        let level = serve.level(call)?;
        if serve.allows(tab, call, grants)? {
            Ok(level)
        } else {
            Err(format!(
                "the data side refused a call on `{tab}` that needs `{}`\nhelp: give the \
                 member of staff a role that holds it",
                level.on(tab)
            ))
        }
    }

    /// The backoffice's end: sends one call of a panel's data contract to
    /// the process that holds the data.
    #[async_trait]
    pub trait PanelWire: Send + Sync + 'static {
        /// Sends `call`, the JSON text of `tab`'s own call type, for `who`.
        /// The answer is the JSON text of `tab`'s answer type.
        ///
        /// # Errors
        ///
        /// What failed, as text with a `help:` line: the transport, the
        /// data side's grant check, or the data side itself.
        async fn send(
            &self,
            who: &Principal<'_>,
            tab: &str,
            call: String,
        ) -> Result<String, String>;
    }

    /// The data side of one panel: decodes a call, names the level it
    /// needs, and runs it.
    #[async_trait]
    pub trait PanelServe: Send + Sync + 'static {
        /// The level `call` needs on the panel's tab: `read` for a read,
        /// `edit` for a write. The server checks the caller's grant with
        /// it before [`PanelServe::serve`].
        ///
        /// # Errors
        ///
        /// When `call` does not decode.
        fn level(&self, call: &str) -> Result<Level, String>;

        /// Whether `grants` may run `call` on `tab`. By default, whether
        /// they hold [`PanelServe::level`] on `tab`. A tab whose RBAC is
        /// not one level on itself says else: the roles tab lets the owner
        /// of `jobs` give `jobs.read`.
        ///
        /// # Errors
        ///
        /// When `call` does not decode.
        fn allows(&self, tab: &str, call: &str, grants: &Grants) -> Result<bool, String> {
            Ok(grants.holds(tab, self.level(call)?))
        }

        /// Runs `call` for `who`, and answers the JSON text of the answer.
        ///
        /// # Errors
        ///
        /// When `call` does not decode, or the data side fails.
        async fn serve(&self, who: &Principal<'_>, call: &str) -> Result<String, String>;
    }

    /// Who a principal is, as the data side checks it.
    pub type GrantsFn = Arc<dyn for<'p> Fn(&Principal<'p>) -> Grants + Send + Sync>;

    /// A [`PanelWire`] in one process: each call is JSON text, checked with
    /// [`authorize`] and run by the [`PanelServe`] of its tab, as the
    /// backoffice wire runs it. For a panel crate's tests, which must not
    /// depend on `brocanth-backoffice`.
    #[derive(Clone)]
    pub struct Loopback {
        panels: HashMap<String, Arc<dyn PanelServe>>,
        grants: GrantsFn,
    }

    impl std::fmt::Debug for Loopback {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Loopback")
                .field("panels", &self.panels.keys().collect::<Vec<_>>())
                .finish_non_exhaustive()
        }
    }

    impl Loopback {
        /// No tab yet. `grants` says what each principal holds.
        pub fn new(
            grants: impl for<'p> Fn(&Principal<'p>) -> Grants + Send + Sync + 'static,
        ) -> Loopback {
            Loopback {
                panels: HashMap::new(),
                grants: Arc::new(grants),
            }
        }

        /// Serves `tab` with `serve`.
        pub fn panel(mut self, tab: impl Into<String>, serve: Arc<dyn PanelServe>) -> Loopback {
            self.panels.insert(tab.into(), serve);
            self
        }

        /// As a panel takes it.
        pub fn shared(self) -> Arc<dyn PanelWire> {
            Arc::new(self)
        }
    }

    #[async_trait]
    impl PanelWire for Loopback {
        async fn send(
            &self,
            who: &Principal<'_>,
            tab: &str,
            call: String,
        ) -> Result<String, String> {
            let serve = self.panels.get(tab).ok_or_else(|| {
                format!("no panel data for the tab `{tab}`\nhelp: serve it on the loopback")
            })?;
            authorize(serve.as_ref(), tab, &call, &(self.grants)(who))?;
            serve.serve(who, &call).await
        }
    }
}
