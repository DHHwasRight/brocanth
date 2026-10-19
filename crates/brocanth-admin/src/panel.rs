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
//! the policy, see other panels, or touch a model. A panel that wants the
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
//! [`Action::ViewPanel`]: crate::policy::Action::ViewPanel
//! [`Action::ActOnPanel`]: crate::policy::Action::ActOnPanel

use async_trait::async_trait;

use crate::admin::AdminError;
use crate::form::Submission;

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

    /// Renders one request.
    ///
    /// `Ok(None)` is this panel's own 404: a sub-path it does not serve.
    /// The admin turns it into the same not-found page a bad model key
    /// gets, so a panel does not have to render one.
    async fn render(&self, request: PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError>;
}

/// One request, as a panel sees it.
///
/// The path is already relative to the panel, because a panel should not
/// have to know where the admin is mounted or what it is called. `""` is
/// the panel's own index.
#[derive(Debug)]
pub struct PanelRequest<'a> {
    /// `""` for the panel's index, `"retries"` for `{base}/{key}/retries`.
    pub path: &'a str,
    /// The query string, without the `?`.
    pub query: &'a str,
    /// Whether this is a POST. A panel that changes something must only do
    /// it here, and the admin has already asked the policy about
    /// [`Action::ActOnPanel`](crate::policy::Action::ActOnPanel).
    pub post: bool,
    /// The form body, for a POST.
    pub submission: &'a Submission,
    /// Where this panel is mounted, so a panel can write its own links:
    /// `{base}/{key}`, with no trailing slash.
    pub mount: &'a str,
}

impl PanelRequest<'_> {
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
