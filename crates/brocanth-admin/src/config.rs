//! Per-model configuration, and the admin's own.
//!
//! Django's `ModelAdmin` is the reference and the surface is kept small on
//! purpose: which columns the list shows, which are searchable, which are
//! sortable, which are read-only, how one record is named, and whether the
//! model appears at all. Everything past that is a template, which is the
//! point of [`crate::view`].
//!
//! Nothing here is required. A model nobody configured gets a default
//! [`ModelAdmin`], which shows every column and searches every text column —
//! an admin should be useful before it is configured, and configurable before
//! it is forked.

use std::collections::BTreeMap;

use crate::discovery::Discovery;
use crate::form::Widget;
use crate::model::{humanise, ColumnDescriptor, ColumnKind, ModelDescriptor, Relation, Row};
use crate::policy::{DenyAll, Policy};
use crate::view::Views;

/// How many rows a list screen shows when nothing says otherwise.
pub const DEFAULT_PER_PAGE: usize = 25;

/// How many records a foreign-key picker offers when nothing says otherwise.
pub const DEFAULT_CHOICE_LIMIT: usize = 200;

/// Names a record. Supplied by the application; the default is below.
pub type LabelFn = Box<dyn Fn(&Row) -> String + Send + Sync>;

/// One model's configuration.
pub struct ModelAdmin {
    key: String,
    visible: bool,
    display_name: Option<String>,
    plural_name: Option<String>,
    list_columns: Vec<String>,
    search_columns: Vec<String>,
    sort_columns: Vec<String>,
    readonly_columns: Vec<String>,
    excluded_columns: Vec<String>,
    per_page: usize,
    choice_limit: usize,
    label: Option<LabelFn>,
    widgets: BTreeMap<String, Widget>,
    help: BTreeMap<String, String>,
    relations: BTreeMap<String, Relation>,
    can_create: bool,
    can_update: bool,
    can_delete: bool,
}

impl std::fmt::Debug for ModelAdmin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelAdmin")
            .field("key", &self.key)
            .field("visible", &self.visible)
            .field("list_columns", &self.list_columns)
            .field("search_columns", &self.search_columns)
            .field("readonly_columns", &self.readonly_columns)
            .field("labelled", &self.label.is_some())
            .finish()
    }
}

impl ModelAdmin {
    /// A configuration for `key`, with every default in place.
    pub fn new(key: impl Into<String>) -> Self {
        ModelAdmin {
            key: key.into(),
            visible: true,
            display_name: None,
            plural_name: None,
            list_columns: Vec::new(),
            search_columns: Vec::new(),
            sort_columns: Vec::new(),
            readonly_columns: Vec::new(),
            excluded_columns: Vec::new(),
            per_page: DEFAULT_PER_PAGE,
            choice_limit: DEFAULT_CHOICE_LIMIT,
            label: None,
            widgets: BTreeMap::new(),
            help: BTreeMap::new(),
            relations: BTreeMap::new(),
            can_create: true,
            can_update: true,
            can_delete: true,
        }
    }

    /// The model key.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Hides the model. It disappears from the index and every screen of it
    /// answers 404 — a hidden model is not a model with an unlinked URL.
    pub fn hidden(mut self) -> Self {
        self.visible = false;
        self
    }

    /// Whether the model appears.
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// The singular name shown to a person.
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.display_name = Some(name.into());
        self
    }

    /// The plural name shown to a person. Defaults to the singular plus `s`.
    pub fn named_plural(mut self, name: impl Into<String>) -> Self {
        self.plural_name = Some(name.into());
        self
    }

    /// The columns the list screen shows, in this order. Empty means all.
    pub fn list_columns<I, S>(mut self, columns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.list_columns = columns.into_iter().map(Into::into).collect();
        self
    }

    /// The columns a search term is matched against. Empty means every text
    /// column, which is a useful default and a poor index plan; name them once
    /// the table is big.
    pub fn searchable<I, S>(mut self, columns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.search_columns = columns.into_iter().map(Into::into).collect();
        self
    }

    /// The columns a header may sort by. Empty means all of them.
    pub fn sortable<I, S>(mut self, columns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.sort_columns = columns.into_iter().map(Into::into).collect();
        self
    }

    /// Columns shown in the form but not editable.
    pub fn readonly<I, S>(mut self, columns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.readonly_columns = columns.into_iter().map(Into::into).collect();
        self
    }

    /// Columns kept out of the form and the detail screen entirely.
    pub fn exclude<I, S>(mut self, columns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.excluded_columns = columns.into_iter().map(Into::into).collect();
        self
    }

    /// Rows per page.
    pub fn per_page(mut self, per_page: usize) -> Self {
        self.per_page = per_page.max(1);
        self
    }

    /// How many records a foreign-key picker pointing at this model offers.
    pub fn choice_limit(mut self, limit: usize) -> Self {
        self.choice_limit = limit.max(1);
        self
    }

    /// Names one record: what the list links say, what a foreign-key picker
    /// shows, and what the delete confirmation asks about.
    pub fn display<F>(mut self, label: F) -> Self
    where
        F: Fn(&Row) -> String + Send + Sync + 'static,
    {
        self.label = Some(Box::new(label));
        self
    }

    /// Forces a widget on one column, over what the schema implies.
    pub fn widget(mut self, column: impl Into<String>, widget: Widget) -> Self {
        self.widgets.insert(column.into(), widget);
        self
    }

    /// Adds help text under one field.
    pub fn help(mut self, column: impl Into<String>, text: impl Into<String>) -> Self {
        self.help.insert(column.into(), text.into());
        self
    }

    /// Declares a foreign key the schema did not report.
    ///
    /// SeaORM reports columns and relations through two different APIs, so a
    /// data source may know the column and not the relation. This is how the
    /// application says `author_id` points at `user`.
    pub fn foreign_key(
        mut self,
        column: impl Into<String>,
        target: impl Into<String>,
        target_column: impl Into<String>,
    ) -> Self {
        self.relations
            .insert(column.into(), Relation::new(target, target_column));
        self
    }

    /// Forbids creating records of this model.
    pub fn no_create(mut self) -> Self {
        self.can_create = false;
        self
    }

    /// Forbids changing records of this model.
    pub fn no_update(mut self) -> Self {
        self.can_update = false;
        self
    }

    /// Forbids deleting records of this model.
    pub fn no_delete(mut self) -> Self {
        self.can_delete = false;
        self
    }

    /// Whether this configuration permits creating. The policy is asked as
    /// well, and both have to agree.
    pub fn allows_create(&self) -> bool {
        self.can_create
    }

    /// Whether this configuration permits changing.
    pub fn allows_update(&self) -> bool {
        self.can_update
    }

    /// Whether this configuration permits deleting.
    pub fn allows_delete(&self) -> bool {
        self.can_delete
    }

    /// Rows per page.
    pub fn page_size(&self) -> usize {
        self.per_page
    }

    /// How many choices a picker pointing here offers.
    pub fn choices_shown(&self) -> usize {
        self.choice_limit
    }

    /// The singular name.
    pub fn name(&self, model: &ModelDescriptor) -> String {
        self.display_name
            .clone()
            .unwrap_or_else(|| humanise(&model.key))
    }

    /// The plural name.
    pub fn plural(&self, model: &ModelDescriptor) -> String {
        self.plural_name
            .clone()
            .unwrap_or_else(|| format!("{}s", self.name(model)))
    }

    /// Help text for a column, or `""`.
    pub fn help_for(&self, column: &str) -> &str {
        self.help.get(column).map(String::as_str).unwrap_or("")
    }

    /// The forced widget for a column, if any.
    pub fn widget_for(&self, column: &str) -> Option<Widget> {
        self.widgets.get(column).copied()
    }

    /// Whether a column is read-only.
    pub fn is_readonly(&self, column: &str) -> bool {
        self.readonly_columns.iter().any(|c| c == column)
    }

    /// Whether a column is kept out of the forms.
    pub fn is_excluded(&self, column: &str) -> bool {
        self.excluded_columns.iter().any(|c| c == column)
    }

    /// The relation on a column: the schema's, or the one declared here.
    pub fn relation_for<'a>(&'a self, column: &'a ColumnDescriptor) -> Option<&'a Relation> {
        self.relations
            .get(&column.name)
            .or(column.relation.as_ref())
    }

    /// The columns the list screen shows.
    ///
    /// A named set is taken in the order it was named, skipping any column the
    /// model does not have — a renamed column must not take the list screen
    /// down. An unnamed set is every column that is not excluded.
    pub fn list_for<'a>(&self, model: &'a ModelDescriptor) -> Vec<&'a ColumnDescriptor> {
        if self.list_columns.is_empty() {
            return model
                .columns
                .iter()
                .filter(|c| !self.is_excluded(&c.name))
                .collect();
        }
        self.list_columns
            .iter()
            .filter_map(|name| model.column(name))
            .collect()
    }

    /// The columns the form shows, which is every column that is neither
    /// excluded nor a generated primary key.
    pub fn form_for<'a>(&self, model: &'a ModelDescriptor) -> Vec<&'a ColumnDescriptor> {
        model
            .columns
            .iter()
            .filter(|c| !self.is_excluded(&c.name))
            .filter(|c| !(c.primary_key && c.auto_generated))
            .collect()
    }

    /// The columns the detail screen shows.
    pub fn detail_for<'a>(&self, model: &'a ModelDescriptor) -> Vec<&'a ColumnDescriptor> {
        model
            .columns
            .iter()
            .filter(|c| !self.is_excluded(&c.name))
            .collect()
    }

    /// The columns a search term is matched against.
    pub fn search_for(&self, model: &ModelDescriptor) -> Vec<String> {
        if !self.search_columns.is_empty() {
            return self
                .search_columns
                .iter()
                .filter(|name| model.column(name).is_some())
                .cloned()
                .collect();
        }
        model
            .columns
            .iter()
            .filter(|c| matches!(c.kind, ColumnKind::Text | ColumnKind::LongText))
            .map(|c| c.name.clone())
            .collect()
    }

    /// The columns a header may sort by.
    pub fn sort_for(&self, model: &ModelDescriptor) -> Vec<String> {
        if !self.sort_columns.is_empty() {
            return self
                .sort_columns
                .iter()
                .filter(|name| model.column(name).is_some())
                .cloned()
                .collect();
        }
        model
            .columns
            .iter()
            .filter(|c| !matches!(c.kind, ColumnKind::Binary | ColumnKind::Json))
            .map(|c| c.name.clone())
            .collect()
    }

    /// Names one record.
    ///
    /// With no [`ModelAdmin::display`] the first non-key text column is used,
    /// falling back to the primary key. An integer is never a useful name for
    /// a thing, which is why the fallback chain exists at all.
    pub fn label_for(&self, model: &ModelDescriptor, row: &Row) -> String {
        if let Some(label) = &self.label {
            let text = label(row);
            if !text.trim().is_empty() {
                return text;
            }
        }
        let natural = model
            .columns
            .iter()
            .filter(|c| !c.primary_key)
            .filter(|c| matches!(c.kind, ColumnKind::Text | ColumnKind::LongText))
            .find_map(|c| match row.get(&c.name) {
                Some(value) if !value.is_null() && !value.as_input().trim().is_empty() => {
                    Some(value.as_input())
                }
                _ => None,
            });
        match natural {
            Some(text) => text,
            None => match crate::model::PrimaryKey::from_row(model, row) {
                Some(key) => format!("{} #{key}", self.name(model)),
                None => self.name(model),
            },
        }
    }
}

/// The admin as a whole.
pub struct AdminConfig {
    /// The heading, the page title, and what the index calls itself.
    pub title: String,
    /// The path the admin is mounted at, without a trailing slash.
    pub base_path: String,
    /// How the model list is narrowed.
    pub discovery: Discovery,
    /// Who may do what. [`DenyAll`] until the application says otherwise.
    pub policy: Box<dyn Policy>,
    /// The screens. Replace any of them with your own rsx component.
    pub views: Views,
    /// Tabs other crates contributed, in the order they were added.
    ///
    /// Not autowired. A crate that can draw a panel does not get to decide
    /// that this application shows one, so adding it is a line in the
    /// application -- the same rule `JobsPlugin` follows about starting a
    /// worker.
    pub panels: Vec<std::sync::Arc<dyn crate::panel::Panel>>,
    models: BTreeMap<String, ModelAdmin>,
}

impl std::fmt::Debug for AdminConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminConfig")
            .field("title", &self.title)
            .field("base_path", &self.base_path)
            .field("discovery", &self.discovery)
            .field("models", &self.models.keys().collect::<Vec<_>>())
            .field(
                "panels",
                &self.panels.iter().map(|p| p.key()).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl Default for AdminConfig {
    fn default() -> Self {
        AdminConfig::new()
    }
}

impl AdminConfig {
    /// An admin at `/admin`, titled "Admin", that denies every request.
    pub fn new() -> Self {
        AdminConfig {
            title: "Admin".to_string(),
            base_path: "/admin".to_string(),
            discovery: Discovery::new(),
            policy: Box::new(DenyAll),
            views: Views::default(),
            panels: Vec::new(),
            models: BTreeMap::new(),
        }
    }

    /// Adds a tab another crate contributed.
    ///
    /// ```ignore
    /// AdminConfig::new().panel(brocanth_jobs::admin::JobsPanel::new(queue))
    /// ```
    ///
    /// Panels appear after the models, in the order they were added. A
    /// panel whose key collides with a model's is refused by
    /// [`Admin::new`](crate::Admin), naming both.
    pub fn panel(mut self, panel: impl crate::panel::Panel + 'static) -> Self {
        self.panels.push(std::sync::Arc::new(panel));
        self
    }

    /// Sets the title.
    pub fn titled(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Sets the mount path. A trailing slash is dropped.
    pub fn at(mut self, base_path: impl Into<String>) -> Self {
        let path = base_path.into();
        self.base_path = path.trim_end_matches('/').to_string();
        self
    }

    /// Narrows the models the admin discovers.
    pub fn discovering(mut self, discovery: Discovery) -> Self {
        self.discovery = discovery;
        self
    }

    /// Installs the policy. Without this the admin denies everything.
    pub fn with_policy(mut self, policy: impl Policy) -> Self {
        self.policy = Box::new(policy);
        self
    }

    /// Replaces one or more screens.
    pub fn with_views(mut self, views: Views) -> Self {
        self.views = views;
        self
    }

    /// Configures one model.
    ///
    /// ```
    /// use brocanth_admin::AdminConfig;
    ///
    /// let config = AdminConfig::new().model("post", |m| {
    ///     m.list_columns(["id", "title", "author_id", "published"])
    ///         .searchable(["title", "body"])
    ///         .readonly(["created_at"])
    ///         .foreign_key("author_id", "user", "id")
    ///         .display(|row| row.get("title").map(ToString::to_string).unwrap_or_default())
    /// });
    /// # let _ = config;
    /// ```
    pub fn model<F>(mut self, key: impl Into<String>, build: F) -> Self
    where
        F: FnOnce(ModelAdmin) -> ModelAdmin,
    {
        let key = key.into();
        let existing = self
            .models
            .remove(&key)
            .unwrap_or_else(|| ModelAdmin::new(key.clone()));
        self.models.insert(key, build(existing));
        self
    }

    /// Takes the configured models out, leaving the admin's own settings.
    ///
    /// [`Admin::new`] calls this once: a [`ModelAdmin`] holds a boxed closure
    /// and so cannot be cloned, and the admin needs to own one per discovered
    /// model whether it was configured or not.
    ///
    /// [`Admin::new`]: crate::admin::Admin::new
    pub(crate) fn take_models(&mut self) -> BTreeMap<String, ModelAdmin> {
        std::mem::take(&mut self.models)
    }

    /// One model's configuration, when there is one.
    pub fn configured(&self, key: &str) -> Option<&ModelAdmin> {
        self.models.get(key)
    }

    /// The keys that were configured.
    pub fn configured_keys(&self) -> Vec<&str> {
        self.models.keys().map(String::as_str).collect()
    }
}
