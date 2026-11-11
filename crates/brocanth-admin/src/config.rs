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
use crate::view::{DeleteCtx, DetailCtx, FormCtx, ListCtx, View, Views};

/// How many rows a list screen shows when nothing says otherwise.
pub const DEFAULT_PER_PAGE: usize = 25;

/// The most rows a list screen shows, whatever `per_page=` in the link asks
/// for, when nothing says otherwise.
///
/// A link is text anybody can write, and `per_page=10000000` would read a
/// whole table into one page. The ask is clamped, not refused, as every
/// paginated API does (`brocanth_pagination::Limits`).
pub const DEFAULT_MAX_PER_PAGE: usize = 100;

/// How many records a foreign-key picker offers when nothing says otherwise.
pub const DEFAULT_CHOICE_LIMIT: usize = 200;

/// Names a record. Supplied by the application; the default is below.
pub type LabelFn = Box<dyn Fn(&Row) -> String + Send + Sync>;

/// Names who is signed in. See [`AdminConfig::user_label`].
pub type UserLabelFn = Box<dyn Fn(&crate::policy::Principal<'_>) -> Option<String> + Send + Sync>;

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
    max_per_page: usize,
    choice_limit: usize,
    label: Option<LabelFn>,
    widgets: BTreeMap<String, Widget>,
    help: BTreeMap<String, String>,
    relations: BTreeMap<String, Relation>,
    can_create: bool,
    can_update: bool,
    can_delete: bool,
    stylesheets: Vec<String>,
    scripts: Vec<String>,
    views: ModelViews,
    actions: Vec<crate::actions::BulkAction>,
    without_actions: Vec<String>,
    inlines: Vec<crate::inline::Inline>,
    pages: Vec<ModelPage>,
}

/// Renders one request of a page under a model. See [`ModelAdmin::page`]
/// and [`ModelAdmin::page_async`].
pub type ModelPageFn =
    Box<dyn for<'a> Fn(crate::panel::PageCx<'a>) -> crate::panel::PageFuture<'a> + Send + Sync>;

/// The stored form of an async render: the closure, with its signature
/// fixed so that the compiler reads the lifetimes of a bare closure.
fn async_page<F>(render: F) -> ModelPageFn
where
    F: for<'a> Fn(crate::panel::PageCx<'a>) -> crate::panel::PageFuture<'a> + Send + Sync + 'static,
{
    Box::new(render)
}

/// The stored form of a synchronous render: a future that is ready at once.
fn sync_page<F>(render: F) -> ModelPageFn
where
    F: Fn(
            &crate::panel::PanelRequest<'_>,
            Option<&Row>,
        ) -> Result<Option<crate::panel::PanelPage>, crate::AdminError>
        + Send
        + Sync
        + 'static,
{
    async_page(move |cx| Box::pin(std::future::ready(render(cx.request(), cx.record()))))
}

/// A page of the application's own under one model, or under one record.
pub(crate) struct ModelPage {
    pub(crate) key: String,
    pub(crate) label: String,
    /// Under one record: `{base}/{model}/{key}/{page}`. Otherwise under the
    /// model: `{base}/{model}/{page}`.
    pub(crate) record: bool,
    pub(crate) render: ModelPageFn,
}

/// The screens one model replaces. `None` is the admin's own view.
#[derive(Default)]
pub(crate) struct ModelViews {
    pub(crate) list: Option<Box<dyn View<ListCtx>>>,
    pub(crate) detail: Option<Box<dyn View<DetailCtx>>>,
    pub(crate) form: Option<Box<dyn View<FormCtx>>>,
    pub(crate) confirm_delete: Option<Box<dyn View<DeleteCtx>>>,
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
            max_per_page: DEFAULT_MAX_PER_PAGE,
            choice_limit: DEFAULT_CHOICE_LIMIT,
            label: None,
            widgets: BTreeMap::new(),
            help: BTreeMap::new(),
            relations: BTreeMap::new(),
            can_create: true,
            can_update: true,
            can_delete: true,
            stylesheets: Vec::new(),
            scripts: Vec::new(),
            views: ModelViews::default(),
            actions: Vec::new(),
            without_actions: Vec::new(),
            inlines: Vec::new(),
            pages: Vec::new(),
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

    /// The most rows per page that `per_page=` in a link can ask for.
    /// [`DEFAULT_MAX_PER_PAGE`] when not set. A ceiling below
    /// [`ModelAdmin::per_page`] is raised to it.
    pub fn max_per_page(mut self, max: usize) -> Self {
        self.max_per_page = max.max(1);
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

    /// Links a stylesheet on this model's screens only, after the admin's
    /// own. Django's `ModelAdmin.Media.css`.
    ///
    /// `href` is a URL the application serves; the admin mounts no asset
    /// route.
    pub fn stylesheet(mut self, href: impl Into<String>) -> Self {
        self.stylesheets.push(href.into());
        self
    }

    /// The stylesheets this model's screens link, in order.
    pub fn stylesheets(&self) -> &[String] {
        &self.stylesheets
    }

    /// Loads a script on this model's screens only, after the admin's own.
    /// Django's `ModelAdmin.Media.js`.
    ///
    /// The layout writes `<script src="…" defer>` in the `<head>`. `src` is
    /// a file the application serves: the admin mounts no asset route and
    /// takes no inline code. A same-origin file passes the default
    /// `script-src 'self'`, so the Content-Security-Policy needs no hash
    /// and no nonce. A file on another origin needs that origin in
    /// `script-src`: `Csp::allow("script-src", ["https://cdn.example.com"])`.
    ///
    /// dp-swap replaces the `<main>` region and merges the response's
    /// `<head>`, so the script loads on a full load and on a swap into
    /// this model's screen, once. See `docs/admin-customization.md`,
    /// level 3.
    pub fn script(mut self, src: impl Into<String>) -> Self {
        self.scripts.push(src.into());
        self
    }

    /// The scripts this model's screens load, in order.
    pub fn scripts(&self) -> &[String] {
        &self.scripts
    }

    /// Replaces the list screen for this model only. Django's
    /// `change_list_template`. It wins over
    /// [`Views::with_list`](crate::Views::with_list).
    pub fn list_view(mut self, view: impl View<ListCtx>) -> Self {
        self.views.list = Some(Box::new(view));
        self
    }

    /// Replaces the detail screen for this model only.
    pub fn detail_view(mut self, view: impl View<DetailCtx>) -> Self {
        self.views.detail = Some(Box::new(view));
        self
    }

    /// Replaces the create and edit form for this model only. Django's
    /// `change_form_template` and `add_form_template` in one, because here
    /// one screen serves both: [`FormCtx::is_new`] tells them apart.
    pub fn form_view(mut self, view: impl View<FormCtx>) -> Self {
        self.views.form = Some(Box::new(view));
        self
    }

    /// Replaces the delete confirmation for this model only. Django's
    /// `delete_confirmation_template`.
    pub fn confirm_delete_view(mut self, view: impl View<DeleteCtx>) -> Self {
        self.views.confirm_delete = Some(Box::new(view));
        self
    }

    /// The screens this model replaced.
    pub(crate) fn views(&self) -> &ModelViews {
        &self.views
    }

    /// Adds an action over selected rows to this model's list. Django's
    /// `ModelAdmin.actions`. See [`crate::actions`].
    ///
    /// An action with the key of one added for every model replaces it on
    /// this model.
    pub fn action(mut self, action: crate::actions::BulkAction) -> Self {
        self.actions.retain(|a| a.key != action.key);
        self.actions.push(action);
        self
    }

    /// Leaves an action added for every model off this model's list.
    pub fn without_action(mut self, key: impl Into<String>) -> Self {
        self.without_actions.push(key.into());
        self
    }

    /// This model's own actions, in the order they were added.
    pub(crate) fn own_actions(&self) -> &[crate::actions::BulkAction] {
        &self.actions
    }

    /// Whether an action added for every model is left off this one.
    pub(crate) fn drops_action(&self, key: &str) -> bool {
        self.without_actions.iter().any(|k| k == key)
    }

    /// Puts another model's records that point at this one on this model's
    /// create and edit form. Django's `ModelAdmin.inlines`. See
    /// [`crate::inline`].
    ///
    /// `Admin::new` stops when the child model is not shown, when no
    /// column of it points at this model, or when two inlines name one
    /// child model.
    pub fn inline(mut self, inline: crate::inline::Inline) -> Self {
        self.inlines.push(inline);
        self
    }

    /// The inlines, in the order they were added.
    pub fn inlines(&self) -> &[crate::inline::Inline] {
        &self.inlines
    }

    /// Adds a page of your own under this model, at `{base}/{model}/{key}`
    /// and every path below it. Django's `ModelAdmin.get_urls()`.
    ///
    /// The admin's layout wraps it, the sidebar marks this model, and the
    /// list screen links it with `label`. The policy is the list's: reading
    /// it asks `Action::ViewList` on the model and the list rule; a POST
    /// also asks `Action::Update` on the model, as an action over selected
    /// rows does. [`PanelRequest::can_act`](crate::panel::PanelRequest)
    /// says whether a POST would pass. The record argument is `None`.
    ///
    /// The closure is synchronous. A page that awaits a read, or that
    /// writes a record, uses [`ModelAdmin::page_async`].
    ///
    /// ```
    /// use brocanth_admin::panel::PanelPage;
    /// use brocanth_admin::AdminConfig;
    ///
    /// let config = AdminConfig::new().model("invoice", |m| {
    ///     m.page("overdue", "Overdue", |request, _| {
    ///         Ok(match request.path {
    ///             "" => Some(PanelPage::body("Overdue invoices", "<p>…</p>")),
    ///             _ => None,
    ///         })
    ///     })
    /// });
    /// # let _ = config;
    /// ```
    ///
    /// A key wins over a record key at the same place: a record whose key
    /// is `overdue` cannot be read. `Admin::new` refuses `new` and
    /// `actions`, which the admin uses there.
    pub fn page<F>(mut self, key: impl Into<String>, label: impl Into<String>, render: F) -> Self
    where
        F: Fn(
                &crate::panel::PanelRequest<'_>,
                Option<&Row>,
            ) -> Result<Option<crate::panel::PanelPage>, crate::AdminError>
            + Send
            + Sync
            + 'static,
    {
        self.pages.push(ModelPage {
            key: key.into(),
            label: label.into(),
            record: false,
            render: sync_page(render),
        });
        self
    }

    /// [`ModelAdmin::page`] with an async render: a page that awaits a
    /// database read, or that writes.
    ///
    /// The render gets a [`PageCx`](crate::panel::PageCx): the request, and
    /// [`PageCx::update`](crate::panel::PageCx::update) and
    /// [`PageCx::delete`](crate::panel::PageCx::delete), which write a
    /// record of this model through the checks of every admin write. The
    /// policy is the one [`ModelAdmin::page`] has.
    ///
    /// ```
    /// use brocanth_admin::panel::PanelPage;
    /// use brocanth_admin::AdminConfig;
    ///
    /// let config = AdminConfig::new().model("invoice", |m| {
    ///     m.page_async("overdue", "Overdue", |cx| {
    ///         Box::pin(async move {
    ///             // let rows = db.overdue().await?;
    ///             Ok(match cx.request().path {
    ///                 "" => Some(PanelPage::body("Overdue invoices", "<p>…</p>")),
    ///                 _ => None,
    ///             })
    ///         })
    ///     })
    /// });
    /// # let _ = config;
    /// ```
    pub fn page_async<F>(
        mut self,
        key: impl Into<String>,
        label: impl Into<String>,
        render: F,
    ) -> Self
    where
        F: for<'a> Fn(crate::panel::PageCx<'a>) -> crate::panel::PageFuture<'a>
            + Send
            + Sync
            + 'static,
    {
        self.pages.push(ModelPage {
            key: key.into(),
            label: label.into(),
            record: false,
            render: async_page(render),
        });
        self
    }

    /// [`ModelAdmin::record_page`] with an async render. The render gets a
    /// [`PageCx`](crate::panel::PageCx) with the record and its key; see
    /// [`ModelAdmin::page_async`].
    pub fn record_page_async<F>(
        mut self,
        key: impl Into<String>,
        label: impl Into<String>,
        render: F,
    ) -> Self
    where
        F: for<'a> Fn(crate::panel::PageCx<'a>) -> crate::panel::PageFuture<'a>
            + Send
            + Sync
            + 'static,
    {
        self.pages.push(ModelPage {
            key: key.into(),
            label: label.into(),
            record: true,
            render: async_page(render),
        });
        self
    }

    /// Adds a page of your own under each record of this model, at
    /// `{base}/{model}/{record}/{key}` and every path below it: "Send" on
    /// an invoice.
    ///
    /// The detail screen links it with `label`. The policy is the
    /// record's: reading it asks `Action::ViewDetail` on the record and
    /// reads the record under the view rule, so a record the rule refuses
    /// is a 404. A POST also asks `Action::Update` on the record and its
    /// update rule. The render function gets the record.
    ///
    /// `Admin::new` refuses `edit` and `delete`, which the admin uses
    /// there.
    pub fn record_page<F>(
        mut self,
        key: impl Into<String>,
        label: impl Into<String>,
        render: F,
    ) -> Self
    where
        F: Fn(
                &crate::panel::PanelRequest<'_>,
                Option<&Row>,
            ) -> Result<Option<crate::panel::PanelPage>, crate::AdminError>
            + Send
            + Sync
            + 'static,
    {
        self.pages.push(ModelPage {
            key: key.into(),
            label: label.into(),
            record: true,
            render: sync_page(render),
        });
        self
    }

    /// The pages under this model and under its records.
    pub(crate) fn pages(&self) -> &[ModelPage] {
        &self.pages
    }

    /// The inlines, for `Admin::new` to resolve their columns.
    pub(crate) fn inlines_mut(&mut self) -> &mut [crate::inline::Inline] {
        &mut self.inlines
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

    /// The page size, and the ceiling on what a link may ask for.
    pub fn page_limits(&self) -> brocanth_pagination::Limits {
        brocanth_pagination::Limits::new(self.per_page as u64, self.max_per_page as u64)
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
    /// The end of every `<title>`, and the sidebar header and the index
    /// heading when those were not set. Django's `site_title`.
    pub title: String,
    /// What the top of the sidebar says. Django's `site_header`.
    pub header: Option<String>,
    /// The index's `<h1>`. Django's `index_title`.
    pub index_title: Option<String>,
    /// The application's front page, for a "View site" link. Django's
    /// `site_url`, except that the default here is no link.
    pub site_url: Option<String>,
    /// Stylesheets every screen links after the shipped one.
    pub stylesheets: Vec<String>,
    /// Scripts every screen loads with `defer`.
    pub scripts: Vec<String>,
    /// Names who is signed in, for [`Chrome::user`](crate::view::Chrome).
    pub user_label: Option<UserLabelFn>,
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
    resources: Vec<std::sync::Arc<dyn crate::rows::RowRules>>,
    unscoped_rows: bool,
    pub(crate) actions: Vec<crate::actions::BulkAction>,
    pub(crate) journal: Option<std::sync::Arc<dyn crate::journal::Journal>>,
    pub(crate) record_links: Vec<(String, crate::journal::RecordLinkFn)>,
}

impl std::fmt::Debug for AdminConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminConfig")
            .field("title", &self.title)
            .field("base_path", &self.base_path)
            .field("discovery", &self.discovery)
            .field("models", &self.models.keys().collect::<Vec<_>>())
            .field(
                "resources",
                &self.resources.iter().map(|r| r.model()).collect::<Vec<_>>(),
            )
            .field("unscoped_rows", &self.unscoped_rows)
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
            header: None,
            index_title: None,
            site_url: None,
            stylesheets: Vec::new(),
            scripts: Vec::new(),
            user_label: None,
            base_path: "/admin".to_string(),
            discovery: Discovery::new(),
            policy: Box::new(DenyAll),
            views: Views::default(),
            panels: Vec::new(),
            models: BTreeMap::new(),
            resources: Vec::new(),
            unscoped_rows: false,
            actions: Vec::new(),
            journal: None,
            record_links: Vec::new(),
        }
    }

    /// Puts one model's rows under its entity's policy.
    ///
    /// Every screen of that model then goes through the policy: the list and
    /// its count filter in the query, a record is read with a guarded select,
    /// a write is guarded by the condition it was checked against, and a form
    /// accepts only the columns `Policy::writable` allows. See
    /// [`crate::rows`].
    ///
    /// A discovered model with no resource answers `403` to every screen,
    /// unless [`AdminConfig::unscoped_rows_unsafe_for_development`] is on.
    ///
    /// [`Admin::new`](crate::Admin::new) panics when the resource names a
    /// model the data source does not hold, when its entity's table is not
    /// that model's table, or when two resources name one model.
    pub fn resource<R: crate::rows::AdminResource>(mut self, resource: R) -> Self {
        self.resources.push(crate::rows::erase(resource));
        self
    }

    /// Lets a model with no [`AdminResource`](crate::rows::AdminResource)
    /// show and change every row, limited only by the screen policy.
    ///
    /// For a development build or a test with an in-memory data source, and
    /// for nothing else. Named so that it cannot appear in a diff without
    /// being noticed, like [`AllowAllUnsafeForDevelopment`](crate::policy::AllowAllUnsafeForDevelopment).
    pub fn unscoped_rows_unsafe_for_development(mut self) -> Self {
        self.unscoped_rows = true;
        self
    }

    /// Takes the registered resources out. [`Admin::new`](crate::Admin::new)
    /// calls this once.
    pub(crate) fn take_resources(&mut self) -> Vec<std::sync::Arc<dyn crate::rows::RowRules>> {
        std::mem::take(&mut self.resources)
    }

    /// Whether a model with no resource is unscoped rather than denied.
    pub(crate) fn rows_unscoped(&self) -> bool {
        self.unscoped_rows
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

    /// Sets what the top of the sidebar says, when it is not the title.
    pub fn headed(mut self, header: impl Into<String>) -> Self {
        self.header = Some(header.into());
        self
    }

    /// Sets the index's heading, when it is not the title.
    pub fn index_titled(mut self, heading: impl Into<String>) -> Self {
        self.index_title = Some(heading.into());
        self
    }

    /// Adds a "View site" link to the sidebar, pointing at `href`.
    pub fn site_url(mut self, href: impl Into<String>) -> Self {
        self.site_url = Some(href.into());
        self
    }

    /// Links a stylesheet on every screen, after the shipped one.
    ///
    /// The shipped stylesheet reads its colours, fonts and radii from CSS
    /// custom properties (`--color-accent`, `--font-sans`, ...), so a
    /// stylesheet of a few lines rethemes the admin:
    ///
    /// ```css
    /// :root { --color-accent: #0f766e; --color-accent-dark: #115e59; }
    /// ```
    ///
    /// `href` is a URL the application serves; the admin mounts no asset
    /// route. A same-origin file passes the default Content-Security-Policy.
    pub fn stylesheet(mut self, href: impl Into<String>) -> Self {
        self.stylesheets.push(href.into());
        self
    }

    /// Loads a script on every screen, with `defer`, after the admin's own.
    ///
    /// The same rules as [`ModelAdmin::script`]: a file the application
    /// serves, no inline code. A script loaded here stays in the page
    /// across dp-swap navigation. To act on each screen a swap shows, listen
    /// for the `dp:afterSwap` event on `document`.
    pub fn script(mut self, src: impl Into<String>) -> Self {
        self.scripts.push(src.into());
        self
    }

    /// Names who is signed in. The user links block shows the name.
    ///
    /// The principal is the application's own identity value, so only the
    /// application can name it. Return `None` for nobody.
    ///
    /// ```
    /// use brocanth_admin::AdminConfig;
    ///
    /// struct User { email: String }
    ///
    /// let config = AdminConfig::new()
    ///     .user_label(|who| who.downcast::<User>().map(|u| u.email.clone()));
    /// # let _ = config;
    /// ```
    pub fn user_label<F>(mut self, label: F) -> Self
    where
        F: Fn(&crate::policy::Principal<'_>) -> Option<String> + Send + Sync + 'static,
    {
        self.user_label = Some(Box::new(label));
        self
    }

    /// Adds a page with the admin's chrome: a tab in the sidebar, behind
    /// the policy as [`Action::ViewPanel`](crate::policy::Action::ViewPanel),
    /// at `{base}/{key}` and every path under it. Django's
    /// `AdminSite.get_urls()`.
    ///
    /// A shorthand for [`panel`](Self::panel) with a
    /// [`Page`](crate::panel::Page): the closure renders one request. A
    /// page that awaits something implements [`Panel`](crate::panel::Panel)
    /// instead.
    ///
    /// ```
    /// use brocanth_admin::panel::PanelPage;
    /// use brocanth_admin::AdminConfig;
    ///
    /// let config = AdminConfig::new().page("reports", "Reports", |request| {
    ///     Ok(match request.path {
    ///         "" => Some(PanelPage::body("Reports", "<p>Nothing to report.</p>")),
    ///         _ => None,
    ///     })
    /// });
    /// # let _ = config;
    /// ```
    pub fn page<F>(self, key: impl Into<String>, label: impl Into<String>, render: F) -> Self
    where
        F: Fn(
                &crate::panel::PanelRequest<'_>,
            ) -> Result<Option<crate::panel::PanelPage>, crate::admin::AdminError>
            + Send
            + Sync
            + 'static,
    {
        self.panel(crate::panel::Page::new(key, label, render))
    }

    /// Adds an action over selected rows to every model's list. Django's
    /// `AdminSite.add_action`. A model leaves it off with
    /// [`ModelAdmin::without_action`], or replaces it with an action of the
    /// same key. See [`crate::actions`].
    ///
    /// ```
    /// use brocanth_admin::actions::BulkAction;
    /// use brocanth_admin::AdminConfig;
    ///
    /// let config = AdminConfig::new().action(BulkAction::delete_selected());
    /// # let _ = config;
    /// ```
    pub fn action(mut self, action: crate::actions::BulkAction) -> Self {
        self.actions.retain(|a| a.key != action.key);
        self.actions.push(action);
        self
    }

    /// Tells `journal` about every write the admin makes: who, which
    /// record, and the values before and after. See [`crate::journal`].
    ///
    /// One journal. `brocanth-audit` installs one that keeps the change
    /// history.
    ///
    /// ```
    /// use brocanth_admin::journal::AdminWrite;
    /// use brocanth_admin::AdminConfig;
    ///
    /// let config = AdminConfig::new().journal(|write: &AdminWrite<'_>| {
    ///     eprintln!("{} {} {}", write.kind.as_str(), write.model, write.key);
    /// });
    /// # let _ = config;
    /// ```
    pub fn journal(mut self, journal: impl crate::journal::Journal + 'static) -> Self {
        self.journal = Some(std::sync::Arc::new(journal));
        self
    }

    /// Adds a link to every record's detail screen. `href` is asked per
    /// record; `None` draws no link on that record.
    ///
    /// Django's History button is one: `brocanth-audit` adds a "History"
    /// link to its panel.
    ///
    /// ```
    /// use brocanth_admin::AdminConfig;
    ///
    /// let config = AdminConfig::new().record_link("History", |r| {
    ///     Some(format!("{}/history?table={}&row={}", r.base_path, r.table, r.key))
    /// });
    /// # let _ = config;
    /// ```
    pub fn record_link<F>(mut self, label: impl Into<String>, href: F) -> Self
    where
        F: Fn(&crate::journal::RecordRef<'_>) -> Option<String> + Send + Sync + 'static,
    {
        self.record_links.push((label.into(), Box::new(href)));
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

    /// Changes the screens this config already holds.
    ///
    /// [`with_views`](Self::with_views) replaces the whole set. This keeps
    /// what an earlier call installed and changes one screen, so two pieces
    /// of code can each replace a screen without undoing the other. The
    /// file `brocanth admin eject` writes uses it.
    ///
    /// ```
    /// use brocanth_admin::view::ErrorCtx;
    /// use brocanth_admin::AdminConfig;
    ///
    /// let config = AdminConfig::new().map_views(|views| {
    ///     views.with_error(|ctx: &ErrorCtx, w: &mut brocanth_admin::rsx::Writer<'_>| {
    ///         w.text(&ctx.message)
    ///     })
    /// });
    /// # let _ = config;
    /// ```
    pub fn map_views(mut self, change: impl FnOnce(Views) -> Views) -> Self {
        self.views = change(std::mem::take(&mut self.views));
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
