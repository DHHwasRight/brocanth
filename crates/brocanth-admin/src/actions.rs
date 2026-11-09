//! Actions over selected rows: a checkbox per row on the list, a menu of
//! actions, a confirmation page, then the writes.
//!
//! Django's `ModelAdmin.actions`. Three kinds:
//!
//! - [`BulkAction::delete_selected`]: Django's "Delete selected", which an
//!   application turns on for every model with
//!   [`AdminConfig::action`](crate::AdminConfig::action).
//! - [`BulkAction::set`]: set columns to fixed values, such as
//!   `status = "published"`. The admin checks at start that the model has
//!   the columns, so a typo stops `Admin::new` rather than a request.
//! - [`BulkAction::custom`]: an async function over the selected records,
//!   which writes through [`ActionCx::update`] and [`ActionCx::delete`].
//!
//! ```
//! use brocanth_admin::actions::BulkAction;
//! use brocanth_admin::{AdminConfig, RowAction};
//!
//! let config = AdminConfig::new()
//!     // "Delete selected" on every model that allows a delete.
//!     .action(BulkAction::delete_selected())
//!     .model("post", |m| {
//!         m.action(BulkAction::set("publish", "Publish selected", [("status", "published")]))
//!             .action(BulkAction::custom(
//!                 "archive",
//!                 "Archive selected",
//!                 RowAction::Update,
//!                 |cx| {
//!                     Box::pin(async move {
//!                         let mut done = 0;
//!                         for record in cx.selected() {
//!                             let values = [("archived".to_string(), true.into())];
//!                             if cx.update(&record.key, &values).await? {
//!                                 done += 1;
//!                             }
//!                         }
//!                         Ok(done)
//!                     })
//!                 },
//!             ))
//!     });
//! # let _ = config;
//! ```
//!
//! # The rules every action goes through
//!
//! An action names the row rule it needs: [`RowAction::Update`] or
//! [`RowAction::Delete`]. The screen policy is asked for that action on the
//! model and on each selected record, and the row policy's condition guards
//! every read and every write, exactly as on the single-record screens. A
//! record the rules refuse is not acted on and is counted as skipped; the
//! confirmation page says how many. A [`BulkAction::set`] whose columns the
//! field mask refuses is a 403 before any row is read.
//!
//! The confirmation page is a POST form, and so is the action. Nothing
//! happens on a GET, and the page works with no script: the checkboxes join
//! the action form through the HTML `form` attribute.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::admin::AdminError;
use crate::model::{PrimaryKey, Row, Value};
use crate::rows::RowAction;

/// The most records one action takes. A list page shows at most a few
/// hundred; a POST with more keys was not made from the list.
pub const MAX_SELECTED: usize = 1_000;

/// What a custom action returns: how many records it acted on.
pub type ActionFuture<'a> = Pin<Box<dyn Future<Output = Result<usize, AdminError>> + Send + 'a>>;

type CustomFn = dyn for<'a> Fn(ActionCx<'a>) -> ActionFuture<'a> + Send + Sync;

/// One action over selected rows.
#[derive(Clone)]
pub struct BulkAction {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) needs: RowAction,
    pub(crate) run: Run,
}

#[derive(Clone)]
pub(crate) enum Run {
    Delete,
    Set(Vec<(String, Value)>),
    Custom(Arc<CustomFn>),
}

impl std::fmt::Debug for BulkAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BulkAction")
            .field("key", &self.key)
            .field("label", &self.label)
            .field("needs", &self.needs)
            .finish()
    }
}

impl BulkAction {
    /// "Delete selected": deletes each selected record the delete rule
    /// admits. Shown only on a model that allows a delete.
    pub fn delete_selected() -> BulkAction {
        BulkAction {
            key: "delete_selected".to_string(),
            label: "Delete selected".to_string(),
            needs: RowAction::Delete,
            run: Run::Delete,
        }
    }

    /// Sets `values` on each selected record the update rule admits.
    ///
    /// On a model, [`Admin::new`](crate::Admin::new) panics when a column
    /// is not one of the model's. Added to every model with
    /// [`AdminConfig::action`](crate::AdminConfig::action), it is shown only
    /// on the models that have every column.
    pub fn set<I, K, V>(key: impl Into<String>, label: impl Into<String>, values: I) -> BulkAction
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<Value>,
    {
        BulkAction {
            key: key.into(),
            label: label.into(),
            needs: RowAction::Update,
            run: Run::Set(
                values
                    .into_iter()
                    .map(|(k, v)| (k.into(), v.into()))
                    .collect(),
            ),
        }
    }

    /// Runs `action` over the selected records the `needs` rule admits.
    ///
    /// `needs` is [`RowAction::Update`] or [`RowAction::Delete`]; any other
    /// rule is read as `Update`. The function writes through
    /// [`ActionCx::update`] and [`ActionCx::delete`], which apply the row
    /// policy and tell the journal, and returns how many records it acted
    /// on.
    pub fn custom<F>(
        key: impl Into<String>,
        label: impl Into<String>,
        needs: RowAction,
        action: F,
    ) -> BulkAction
    where
        F: for<'a> Fn(ActionCx<'a>) -> ActionFuture<'a> + Send + Sync + 'static,
    {
        BulkAction {
            key: key.into(),
            label: label.into(),
            needs: match needs {
                RowAction::Delete => RowAction::Delete,
                _ => RowAction::Update,
            },
            run: Run::Custom(Arc::new(action)),
        }
    }

    /// The key the form posts: `delete_selected`, `publish`.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// What the menu says.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The row rule it needs.
    pub fn needs(&self) -> RowAction {
        self.needs
    }

    /// Whether it deletes, which makes its button red.
    pub fn is_destructive(&self) -> bool {
        self.needs == RowAction::Delete
    }

    /// The columns a [`BulkAction::set`] writes. Empty for the other kinds.
    pub fn columns(&self) -> Vec<&str> {
        match &self.run {
            Run::Set(values) => values.iter().map(|(name, _)| name.as_str()).collect(),
            _ => Vec::new(),
        }
    }
}

/// One selected record, as the rules let this person read it.
#[derive(Clone, Debug)]
pub struct Selected {
    /// The record's key.
    pub key: PrimaryKey,
    /// What the record is called.
    pub label: String,
    /// The row, read under the action's rule.
    pub row: Row,
}

/// What a custom action is handed: the selected records, and the two writes
/// that keep the rules.
pub struct ActionCx<'a> {
    pub(crate) admin: &'a crate::admin::Admin,
    pub(crate) model: &'a str,
    pub(crate) principal: crate::policy::Principal<'a>,
    pub(crate) selected: &'a [Selected],
}

impl ActionCx<'_> {
    /// The records the rules admitted, in the order they were selected.
    pub fn selected(&self) -> &[Selected] {
        self.selected
    }

    /// The model key.
    pub fn model(&self) -> &str {
        self.model
    }
}
