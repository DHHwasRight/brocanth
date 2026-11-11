//! The four screens, and the routing that reaches them.
//!
//! A list with pagination, search and sortable columns; a detail view; one
//! form that both creates and edits; and a delete behind a confirmation. That
//! is what an admin is. Everything here is the arrangement of
//! [`crate::data::AdminData`], [`crate::config::ModelAdmin`] and
//! [`crate::view::Views`] into those four, with the policy asked first every
//! time.

use std::collections::BTreeMap;
use std::sync::Arc;

use brocanth_policy::{CompileError, MaskViolation};
use thiserror::Error;

use crate::actions::{ActionCx, BulkAction, Run, Selected, MAX_SELECTED};
use crate::config::{AdminConfig, ModelAdmin};
use crate::data::{
    AdminData, AdminTransaction, DataError, Equals, ListRequest, Search, SharedData,
};
use crate::discovery::discover;
use crate::form::{Choice, Form, Submission};
use crate::inline::Inline;
use crate::journal::{AdminWrite, FieldChange, Journal, RecordLinkFn, RecordRef, Written};
use crate::model::{ColumnDescriptor, ColumnKind, ModelDescriptor, PrimaryKey, Row, Value};
use crate::paging::Pager;
use crate::panel::{PageCx, Panel, PanelPage, PanelRequest};
use crate::policy::{Action, Principal, Request};
use crate::query::{Direction, ListQuery};
use crate::rows::{Gate, Guard, RowAction, RowRules, Rules, Write};
use crate::view::{
    self, ActionChoice, ActionCtx, Cell, Chrome, ColumnHeader, DeleteCtx, DetailCtx, DetailRow,
    ErrorCtx, FormCtx, IndexCtx, IndexEntry, InlineRow, InlineSet, LayoutCtx, ListCtx,
    ModelHeading, NavItem, RecordLink, RowView, SelectedRecord, Views,
};

/// How many numbered page links a pager shows.
const PAGER_WINDOW: u64 = Pager::SPAN;

/// What the admin can fail with.
///
/// A refusal and a missing record are not in here: they are screens, with a
/// status and a page, because that is what a browser needs. This is for the
/// two things that are genuinely a failure.
#[derive(Debug, Error)]
pub enum AdminError {
    /// The storage layer failed.
    #[error("admin data source failed: {0}")]
    Data(#[from] DataError),
    /// A template failed to render.
    #[error("admin failed to render: {0}")]
    Render(#[from] rsx::Error),
    /// A row policy did not compile. A programming error, never a request
    /// error.
    #[error("admin row policy did not compile: {0}")]
    Policy(#[from] CompileError),
    /// A row policy's field mask does not know a column of its model: the
    /// resource and the model describe different tables.
    #[error("admin row policy does not match its model: {0}")]
    Mask(MaskViolation),
}

/// What a screen produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Response {
    /// A page, with the status it should be served with. 200, or 403, or 404,
    /// or 422 when a submission was rejected.
    Page {
        /// The HTTP status.
        status: u16,
        /// The whole document.
        html: String,
    },
    /// A redirect after a successful write, so a reload does not repost.
    Redirect {
        /// The HTTP status: 303.
        status: u16,
        /// Where to.
        location: String,
    },
}

impl Response {
    /// The HTTP status.
    pub fn status(&self) -> u16 {
        match self {
            Response::Page { status, .. } | Response::Redirect { status, .. } => *status,
        }
    }

    /// The document, when this is a page.
    pub fn html(&self) -> Option<&str> {
        match self {
            Response::Page { html, .. } => Some(html.as_str()),
            Response::Redirect { .. } => None,
        }
    }

    /// Where to, when this is a redirect.
    pub fn location(&self) -> Option<&str> {
        match self {
            Response::Redirect { location, .. } => Some(location.as_str()),
            Response::Page { .. } => None,
        }
    }
}

/// What a child row says when its write lost a race.
const CHANGED_MEANWHILE: &str =
    "This record changed while you were working on it. Reload the page and try again.";

/// One inline, built for one request: the rows to show, and the writes a
/// POST asked for.
struct Built<'a> {
    child: &'a Entry,
    fk: &'a str,
    fk_kind: ColumnKind,
    /// The parent column the child's `fk` holds.
    parent_column: &'a str,
    set: InlineSet,
    ops: Vec<Op>,
}

/// One child write, checked and admitted, waiting for the parent's write.
enum Op {
    Update {
        row: usize,
        key: PrimaryKey,
        before: Row,
        values: Vec<(String, Value)>,
        guard: Guard,
    },
    Insert {
        row: usize,
        values: Vec<(String, Value)>,
    },
    Delete {
        row: usize,
        key: PrimaryKey,
        before: Row,
        guard: Guard,
    },
}

/// What the form says when a child row was refused and the save was undone.
const NOT_SAVED: &str = "Nothing was saved. Fix the marked row and save again.";

/// Where a save writes: straight to the data source, or into one
/// transaction when it writes inline rows too.
enum Writer<'a> {
    Direct(&'a dyn AdminData),
    Transaction(Box<dyn AdminTransaction + 'a>),
}

impl Writer<'_> {
    async fn insert(
        &mut self,
        model: &ModelDescriptor,
        values: &[(String, Value)],
    ) -> Result<PrimaryKey, DataError> {
        match self {
            Writer::Direct(data) => data.insert(model, values).await,
            Writer::Transaction(tx) => tx.insert(model, values).await,
        }
    }

    async fn update(
        &mut self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
        values: &[(String, Value)],
        scope: crate::data::Scope<'_>,
    ) -> Result<u64, DataError> {
        match self {
            Writer::Direct(data) => data.update(model, key, values, scope).await,
            Writer::Transaction(tx) => tx.update(model, key, values, scope).await,
        }
    }

    async fn delete(
        &mut self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
        scope: crate::data::Scope<'_>,
    ) -> Result<u64, DataError> {
        match self {
            Writer::Direct(data) => data.delete(model, key, scope).await,
            Writer::Transaction(tx) => tx.delete(model, key, scope).await,
        }
    }

    async fn commit(self) -> Result<(), DataError> {
        match self {
            Writer::Direct(_) => Ok(()),
            Writer::Transaction(tx) => tx.commit().await,
        }
    }

    async fn rollback(self) -> Result<(), DataError> {
        match self {
            Writer::Direct(_) => Ok(()),
            Writer::Transaction(tx) => tx.rollback().await,
        }
    }
}

/// One write the journal hears about once the save is kept.
struct Told<'e> {
    entry: &'e Entry,
    key: PrimaryKey,
    label: String,
    kind: Written,
    fields: Vec<FieldChange>,
}

/// Ends a save that failed with `error`: discards its writes, and keeps the
/// first error, which says what went wrong.
async fn abandon(writer: Writer<'_>, error: impl Into<AdminError>) -> AdminError {
    let _ = writer.rollback().await;
    error.into()
}

/// The sets to render, out of the built inlines.
fn sets(built: Vec<Built<'_>>) -> Vec<InlineSet> {
    built.into_iter().map(|b| b.set).collect()
}

/// Whether every child row passed the form checks.
fn rows_valid(built: &[Built<'_>]) -> bool {
    built
        .iter()
        .all(|b| b.set.rows.iter().all(|row| row.form.is_valid()))
}

/// Gives each row its place in the names it posts: `post-0`, `post-1`.
fn number(set: &mut InlineSet) {
    let prefix = set.model.key.clone();
    for (index, row) in set.rows.iter_mut().enumerate() {
        row.prefix = format!("{prefix}-{index}");
    }
}

/// One discovered model and the configuration that goes with it.
struct Entry {
    descriptor: ModelDescriptor,
    admin: ModelAdmin,
    /// The row policy, when a resource was registered for this model.
    rules: Option<Arc<dyn RowRules>>,
}

/// A mounted admin.
pub struct Admin {
    data: SharedData,
    title: String,
    header: String,
    index_title: String,
    site_url: Option<String>,
    stylesheets: Vec<String>,
    scripts: Vec<String>,
    user_label: Option<crate::config::UserLabelFn>,
    base_path: String,
    policy: Box<dyn crate::policy::Policy>,
    views: Views,
    entries: Vec<Entry>,
    panels: Vec<std::sync::Arc<dyn Panel>>,
    unscoped_rows: bool,
    /// Actions every model's list offers, before each model's own.
    actions: Vec<BulkAction>,
    journal: Option<Arc<dyn Journal>>,
    record_links: Vec<(String, RecordLinkFn)>,
}

impl std::fmt::Debug for Admin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Admin")
            .field("title", &self.title)
            .field("base_path", &self.base_path)
            .field("models", &self.model_keys())
            .field("panels", &self.panel_keys())
            .finish()
    }
}

impl Admin {
    /// Discovers the models and builds the admin.
    ///
    /// Discovery runs once, here: the entity registry is read out of the
    /// binary and cannot change while the process runs, so reading it per
    /// request would buy nothing.
    pub fn new(data: SharedData, mut config: AdminConfig) -> Admin {
        let descriptors = discover(data.registry(), &config.discovery);
        let mut configured = config.take_models();
        let resources = config.take_resources();
        let unscoped_rows = config.rows_unscoped();
        // Checked now rather than discovered by a request. A resource for a
        // model the admin does not show governs nothing, and a resource whose
        // table is not the model's table would compile its conditions against
        // the wrong table.
        for (index, resource) in resources.iter().enumerate() {
            let model = resource.model();
            let Some(descriptor) = descriptors.iter().find(|d| d.key == model) else {
                panic!(
                    "the admin has a row policy for `{model}`, but the data source holds no \
                     model with that key, or discovery left it out"
                );
            };
            if descriptor.table != resource.table() {
                panic!(
                    "the admin's row policy for `{model}` is written for table `{}`, but the \
                     model's table is `{}`",
                    resource.table(),
                    descriptor.table
                );
            }
            if resources[..index].iter().any(|r| r.model() == model) {
                panic!("the admin has two row policies for `{model}`");
            }
        }
        let entries: Vec<Entry> = descriptors
            .into_iter()
            .map(|descriptor| {
                let admin = configured
                    .remove(&descriptor.key)
                    .unwrap_or_else(|| ModelAdmin::new(descriptor.key.clone()));
                let rules = resources
                    .iter()
                    .find(|r| r.model() == descriptor.key)
                    .cloned();
                Entry {
                    descriptor,
                    admin,
                    rules,
                }
            })
            .filter(|entry| entry.admin.is_visible())
            .collect();
        // A `set` action names columns, and a misspelt one would write
        // nothing or fail on the first request. Named now instead.
        for entry in &entries {
            for action in entry.admin.own_actions() {
                for column in action.columns() {
                    if entry.descriptor.column(column).is_none() {
                        panic!(
                            "the admin action `{}` on `{}` sets `{column}`, which is not a column \
                             of that model",
                            action.key(),
                            entry.descriptor.key
                        );
                    }
                }
            }
        }
        // A page under a model takes a path segment the admin's own
        // screens would otherwise answer. Refused now, naming the page.
        for entry in &entries {
            let pages = entry.admin.pages();
            for (index, page) in pages.iter().enumerate() {
                check_page(&entry.descriptor.key, page);
                if pages[..index]
                    .iter()
                    .any(|other| other.record == page.record && other.key == page.key)
                {
                    panic!(
                        "the admin has two pages called `{}` under `{}`.\nhelp: give each page \
                         its own key",
                        page.key, entry.descriptor.key
                    );
                }
            }
        }
        // An inline names a child model and the column that points back.
        // Resolved now, so a misspelt model or an ambiguous column stops the
        // build of the admin and not the first edit form.
        let mut entries = entries;
        let resolved: Vec<Vec<(String, String)>> = entries
            .iter()
            .map(|entry| {
                entry
                    .admin
                    .inlines()
                    .iter()
                    .enumerate()
                    .map(|(index, inline)| {
                        if entry.admin.inlines()[..index]
                            .iter()
                            .any(|other| other.model == inline.model)
                        {
                            panic!(
                                "the admin has two inlines of `{}` on `{}`.\nhelp: keep one: an \
                                 inline's rows post under the child model's key, so two would \
                                 collide",
                                inline.model, entry.descriptor.key
                            );
                        }
                        resolve_inline(&entries, entry, inline)
                    })
                    .collect()
            })
            .collect();
        for (entry, columns) in entries.iter_mut().zip(resolved) {
            for (inline, (fk, parent_column)) in entry.admin.inlines_mut().iter_mut().zip(columns) {
                inline.fk = Some(fk);
                inline.parent_column = parent_column;
            }
        }
        let actions = std::mem::take(&mut config.actions);
        let journal = config.journal.take();
        let record_links = std::mem::take(&mut config.record_links);
        let panels = config.panels;
        // Named now rather than discovered by a request that went to the
        // wrong screen. Two modules claiming one path is an error that
        // names both, and a panel shadowing a model is the same mistake.
        if let Some((panel, model)) = collision(&panels, &entries) {
            panic!(
                "the admin has a panel and a model both called `{panel}`: the panel would \
                 shadow the model at the same URL. Rename the panel, or hide the model with \
                 `Discovery::without`. (model discovered as `{model}`)"
            );
        }
        Admin {
            unscoped_rows,
            data,
            header: config.header.unwrap_or_else(|| config.title.clone()),
            index_title: config.index_title.unwrap_or_else(|| config.title.clone()),
            title: config.title,
            site_url: config.site_url,
            stylesheets: config.stylesheets,
            scripts: config.scripts,
            user_label: config.user_label,
            base_path: config.base_path,
            policy: config.policy,
            views: config.views,
            entries,
            panels,
            actions,
            journal,
            record_links,
        }
    }

    /// The keys of every panel the admin shows.
    pub fn panel_keys(&self) -> Vec<&str> {
        self.panels.iter().map(|p| p.key()).collect()
    }

    /// The keys of every model the admin shows.
    pub fn model_keys(&self) -> Vec<&str> {
        self.entries
            .iter()
            .map(|e| e.descriptor.key.as_str())
            .collect()
    }

    /// One model's descriptor, when the admin shows it.
    pub fn descriptor(&self, key: &str) -> Option<&ModelDescriptor> {
        self.entry(key).map(|e| &e.descriptor)
    }

    /// Where the admin is mounted.
    pub fn base_path(&self) -> &str {
        &self.base_path
    }

    // ---------------------------------------------------------------- screens

    /// The index: every model this person may list.
    pub async fn index(&self, principal: Principal<'_>) -> Result<Response, AdminError> {
        if !self.allows(principal, Request::new(Action::ViewIndex, "")) {
            return self.refuse(principal, Action::ViewIndex);
        }
        let models = self
            .entries
            .iter()
            .filter(|e| self.lists(principal, e))
            .map(|e| {
                let can_create = e.admin.allows_create()
                    && self.allows(principal, Request::new(Action::Create, &e.descriptor.key))
                    && self.creates(principal, e);
                IndexEntry {
                    label: e.admin.plural(&e.descriptor),
                    href: self.list_url(&e.descriptor.key),
                    module_path: e.descriptor.module_path.clone(),
                    columns: e.descriptor.columns.len(),
                    create_href: self.new_url(&e.descriptor.key),
                    key: e.descriptor.key.clone(),
                    can_create,
                }
            })
            .collect();
        let context = IndexCtx {
            chrome: self.chrome(principal, ""),
            heading: self.index_title.clone(),
            models,
        };
        let body = view::render(self.views.index.as_ref(), &context)?;
        self.page(200, principal, "", self.index_title.clone(), body)
    }

    /// One model's records.
    pub async fn list(
        &self,
        principal: Principal<'_>,
        model: &str,
        query: ListQuery,
    ) -> Result<Response, AdminError> {
        self.list_with(principal, model, query, None).await
    }

    /// The list, with the line about the action that just ran: its key and
    /// how many records it acted on, as the redirect after it says.
    async fn list_with(
        &self,
        principal: Principal<'_>,
        model: &str,
        query: ListQuery,
        done: Option<(String, usize)>,
    ) -> Result<Response, AdminError> {
        let Some(entry) = self.entry(model) else {
            return self.missing(principal);
        };
        if !self.allows(principal, Request::new(Action::ViewList, model)) {
            return self.refuse(principal, Action::ViewList);
        }
        // The list rule goes into the query, for the count and for the page
        // alike. Nothing below filters a row it has read, so the total and
        // the pages describe one set (PLAN.md §9).
        let gate = self.gate(principal, entry, RowAction::List)?;
        let Some(scope) = gate.scope() else {
            return self.refuse(principal, Action::ViewList);
        };
        let descriptor = &entry.descriptor;
        let admin = &entry.admin;
        let sortable = admin.sort_for(descriptor);
        let query = query.validated(descriptor, &sortable);
        let search_columns = admin.search_for(descriptor);
        let search = Search {
            term: &query.search,
            columns: &search_columns,
        };
        let total = self.data.count(descriptor, search, scope).await?;
        let page = admin
            .page_limits()
            .page(query.page, query.per_page, total as u64);
        let rows = self
            .data
            .list(ListRequest {
                model: descriptor,
                offset: usize::try_from(page.offset()).unwrap_or(usize::MAX),
                limit: usize::try_from(page.limit()).unwrap_or(usize::MAX),
                search,
                sort: query.sort.as_ref(),
                filter: None,
                scope,
            })
            .await?;

        let columns = admin.list_for(descriptor);
        let labels = self
            .foreign_labels(principal, admin, &columns, &rows)
            .await?;

        let headers = columns
            .iter()
            .map(|column| self.header(&query, &sortable, model, column))
            .collect();
        // Once per page, not per row: a rule is per subject. A row that the
        // update rule refuses still gets the link, and its edit form answers
        // 404, which is the same answer a missing row gets.
        let writes = (
            self.opens(principal, entry, RowAction::Update),
            self.opens(principal, entry, RowAction::Delete),
        );
        let row_views = rows
            .iter()
            .map(|row| self.row_view(principal, entry, &columns, row, &labels, writes))
            .collect();

        let pager = Pager::with(page, PAGER_WINDOW, |number| {
            format!("{}?{}", self.list_url(model), query.to_query_string(number))
        });
        let context = ListCtx {
            chrome: self.chrome(principal, model),
            model: self.heading(entry),
            columns: headers,
            rows: row_views,
            previous_href: pager.previous,
            next_href: pager.next,
            pages: pager.pages,
            page,
            searchable: !search_columns.is_empty(),
            can_create: admin.allows_create()
                && self.allows(principal, Request::new(Action::Create, model))
                && self.creates(principal, entry),
            create_href: self.new_url(model),
            query,
            actions: self
                .actions_for(principal, entry)
                .into_iter()
                .map(|action| ActionChoice {
                    key: action.key.clone(),
                    label: action.label.clone(),
                })
                .collect(),
            actions_href: self.actions_url(model),
            links: admin
                .pages()
                .iter()
                .filter(|page| !page.record)
                .map(|page| RecordLink {
                    label: page.label.clone(),
                    href: format!("{}/{}", self.list_url(model), page.key),
                })
                .collect(),
            notice: done.and_then(|(key, count)| {
                let action = self.offered(entry).into_iter().find(|a| a.key == key)?;
                let noun = match count {
                    1 => admin.name(descriptor).to_lowercase(),
                    _ => admin.plural(descriptor).to_lowercase(),
                };
                Some(format!("{}: {count} {noun}.", action.label))
            }),
        };
        let heading = admin.plural(descriptor);
        let body = view::render(
            entry
                .admin
                .views()
                .list
                .as_deref()
                .unwrap_or(self.views.list.as_ref()),
            &context,
        )?;
        self.page(200, principal, model, heading, body)
    }

    /// One record.
    pub async fn detail(
        &self,
        principal: Principal<'_>,
        model: &str,
        key: &str,
    ) -> Result<Response, AdminError> {
        let Some(entry) = self.entry(model) else {
            return self.missing(principal);
        };
        let Some(record) = PrimaryKey::parse(&entry.descriptor, key) else {
            return self.missing(principal);
        };
        if !self.allows(principal, Request::on(Action::ViewDetail, model, &record)) {
            return self.refuse(principal, Action::ViewDetail);
        }
        // One guarded select: `WHERE pk = ? AND <view condition>`. A row that
        // is not there and a row the rule refuses are one answer, 404.
        let gate = self.gate(principal, entry, RowAction::View)?;
        let Some(scope) = gate.scope() else {
            return self.refuse(principal, Action::ViewDetail);
        };
        let Some(row) = self.data.fetch(&entry.descriptor, &record, scope).await? else {
            return self.missing(principal);
        };
        let admin = &entry.admin;
        let columns = admin.detail_for(&entry.descriptor);
        let labels = self
            .foreign_labels(principal, admin, &columns, std::slice::from_ref(&row))
            .await?;
        let rows = columns
            .iter()
            .map(|column| {
                let cell = self.cell(admin, column, &row, &labels);
                DetailRow {
                    name: column.name.clone(),
                    label: column.label(),
                    text: cell.text,
                    href: cell.href,
                }
            })
            .collect();
        let label = admin.label_for(&entry.descriptor, &row);
        let context = DetailCtx {
            chrome: self.chrome(principal, model),
            model: self.heading(entry),
            key: record.to_segment(),
            edit_href: (admin.allows_update()
                && self.allows(principal, Request::on(Action::Update, model, &record))
                && self.opens(principal, entry, RowAction::Update))
            .then(|| self.edit_url(model, &record)),
            delete_href: (admin.allows_delete()
                && self.allows(principal, Request::on(Action::Delete, model, &record))
                && self.opens(principal, entry, RowAction::Delete))
            .then(|| self.delete_url(model, &record)),
            rows,
            links: {
                let key = record.to_segment();
                // The pages under this record first, then the links other
                // crates add, such as its history.
                let pages =
                    admin
                        .pages()
                        .iter()
                        .filter(|page| page.record)
                        .map(|page| RecordLink {
                            label: page.label.clone(),
                            href: format!("{}/{}", self.detail_url(model, &record), page.key),
                        });
                let at = RecordRef {
                    base_path: &self.base_path,
                    model,
                    table: &entry.descriptor.table,
                    key: &key,
                };
                pages
                    .chain(self.record_links.iter().filter_map(|(label, href)| {
                        href(&at).map(|href| RecordLink {
                            label: label.clone(),
                            href,
                        })
                    }))
                    .collect()
            },
            label: label.clone(),
        };
        let body = view::render(
            entry
                .admin
                .views()
                .detail
                .as_deref()
                .unwrap_or(self.views.detail.as_ref()),
            &context,
        )?;
        self.page(200, principal, model, label, body)
    }

    /// The empty create form.
    pub async fn create_form(
        &self,
        principal: Principal<'_>,
        model: &str,
    ) -> Result<Response, AdminError> {
        let Some(entry) = self.entry(model) else {
            return self.missing(principal);
        };
        if !entry.admin.allows_create()
            || !self.allows(principal, Request::new(Action::Create, model))
        {
            return self.refuse(principal, Action::Create);
        }
        if let Some(refused) = self.refuse_create(principal, entry)? {
            return Ok(refused);
        }
        let choices = self
            .choices(principal, &entry.admin, &entry.descriptor)
            .await?;
        let mut form = Form::build(&entry.descriptor, &entry.admin, None, &choices);
        self.lock(principal, entry, Write::Create, &mut form);
        let inlines = match self.inlines(principal, entry, None, None).await? {
            Ok(built) => sets(built),
            Err(refused) => return Ok(refused),
        };
        self.form_page(principal, entry, form, None, inlines, 200)
    }

    /// Takes a create.
    ///
    /// A rejected submission comes back as a 422 page holding the same form
    /// with the submitted values still in it.
    pub async fn create(
        &self,
        principal: Principal<'_>,
        model: &str,
        submission: &Submission,
    ) -> Result<Response, AdminError> {
        let Some(entry) = self.entry(model) else {
            return self.missing(principal);
        };
        if !entry.admin.allows_create()
            || !self.allows(principal, Request::new(Action::Create, model))
        {
            return self.refuse(principal, Action::Create);
        }
        if let Some(refused) = self.refuse_create(principal, entry)? {
            return Ok(refused);
        }
        if let Some(refused) = self.refuse_fields(principal, entry, Write::Create, submission)? {
            return Ok(refused);
        }
        let choices = self
            .choices(principal, &entry.admin, &entry.descriptor)
            .await?;
        let mut form = Form::build(&entry.descriptor, &entry.admin, None, &choices);
        self.lock(principal, entry, Write::Create, &mut form);
        form.accept(submission);
        let valid = form.validate();
        let mut built = match self
            .inlines(principal, entry, None, Some(submission))
            .await?
        {
            Ok(built) => built,
            Err(refused) => return Ok(refused),
        };
        if !valid || !rows_valid(&built) {
            return self.form_page(principal, entry, form, None, sets(built), 422);
        }
        let values = form.values();
        let mut writer = self.writer(&built).await?;
        let key = match writer.insert(&entry.descriptor, &values).await {
            Ok(key) => key,
            Err(DataError::Invalid(errors)) => {
                writer.rollback().await?;
                form.reject(&errors);
                return self.form_page(principal, entry, form, None, sets(built), 422);
            }
            Err(other) => return Err(abandon(writer, other).await),
        };
        let mut row = row_of(&values);
        for (column, text) in key.parts() {
            if row.get(column).is_none_or(Value::is_null) {
                row.set(column.clone(), Value::Text(text.clone()));
            }
        }
        let mut told = vec![Told {
            entry,
            key: key.clone(),
            label: entry.admin.label_for(&entry.descriptor, &row),
            kind: Written::Created,
            fields: crate::journal::changes(None, &values),
        }];
        if !self.finish(writer, &mut built, &row, &mut told).await? {
            // Nothing was kept, so the form that comes back still creates.
            form.add_error(NOT_SAVED);
            return self.form_page(principal, entry, form, None, sets(built), 422);
        }
        self.tell_all(principal, told);
        Ok(Response::Redirect {
            status: 303,
            location: self.detail_url(model, &key),
        })
    }

    /// The edit form, filled from the record.
    pub async fn edit_form(
        &self,
        principal: Principal<'_>,
        model: &str,
        key: &str,
    ) -> Result<Response, AdminError> {
        let Some(entry) = self.entry(model) else {
            return self.missing(principal);
        };
        let Some(record) = PrimaryKey::parse(&entry.descriptor, key) else {
            return self.missing(principal);
        };
        if !entry.admin.allows_update()
            || !self.allows(principal, Request::on(Action::Update, model, &record))
        {
            return self.refuse(principal, Action::Update);
        }
        // Read under the update rule: a row this person may not change has
        // no edit form, and the answer is the one a missing row gets.
        let gate = self.gate(principal, entry, RowAction::Update)?;
        let Some(scope) = gate.scope() else {
            return self.refuse(principal, Action::Update);
        };
        let Some(row) = self.data.fetch(&entry.descriptor, &record, scope).await? else {
            return self.missing(principal);
        };
        let choices = self
            .choices(principal, &entry.admin, &entry.descriptor)
            .await?;
        let mut form = Form::build(&entry.descriptor, &entry.admin, Some(&row), &choices);
        self.lock(principal, entry, Write::Update, &mut form);
        let inlines = match self.inlines(principal, entry, Some(&row), None).await? {
            Ok(built) => sets(built),
            Err(refused) => return Ok(refused),
        };
        self.form_page(principal, entry, form, Some(&record), inlines, 200)
    }

    /// Takes an edit.
    pub async fn update(
        &self,
        principal: Principal<'_>,
        model: &str,
        key: &str,
        submission: &Submission,
    ) -> Result<Response, AdminError> {
        let Some(entry) = self.entry(model) else {
            return self.missing(principal);
        };
        let Some(record) = PrimaryKey::parse(&entry.descriptor, key) else {
            return self.missing(principal);
        };
        if !entry.admin.allows_update()
            || !self.allows(principal, Request::on(Action::Update, model, &record))
        {
            return self.refuse(principal, Action::Update);
        }
        // The order is `brocanth-resource`'s and it is load-bearing:
        // 1. a Never rule is 403, and no row is read;
        // 2. the field mask is 403, and still no row is read, so the answer
        //    says nothing about whether the row exists;
        // 3. the admission: one guarded select, 404 when it finds nothing;
        // 4. the write, guarded by the same condition; zero rows is 409.
        let Some(pending) = self
            .rules(entry)
            .admission(&principal, RowAction::Update, &record)?
        else {
            return self.refuse(principal, Action::Update);
        };
        if let Some(refused) = self.refuse_fields(principal, entry, Write::Update, submission)? {
            return Ok(refused);
        }
        let found = self
            .data
            .fetch(&entry.descriptor, &record, pending.scope())
            .await?;
        let Some(row) = found else {
            return self.missing(principal);
        };
        let Ok(guard) = pending.resolve(1) else {
            return self.missing(principal);
        };
        let choices = self
            .choices(principal, &entry.admin, &entry.descriptor)
            .await?;
        let mut form = Form::build(&entry.descriptor, &entry.admin, Some(&row), &choices);
        self.lock(principal, entry, Write::Update, &mut form);
        form.accept(submission);
        let valid = form.validate();
        let mut built = match self
            .inlines(principal, entry, Some(&row), Some(submission))
            .await?
        {
            Ok(built) => built,
            Err(refused) => return Ok(refused),
        };
        if !valid || !rows_valid(&built) {
            return self.form_page(principal, entry, form, Some(&record), sets(built), 422);
        }
        let values = form.values();
        let mut writer = self.writer(&built).await?;
        let written = writer
            .update(&entry.descriptor, &record, &values, guard.scope())
            .await;
        match written.map(|rows| guard.applied(rows)) {
            Ok(Ok(_)) => {}
            Ok(Err(_lost)) => {
                writer.rollback().await?;
                return self.conflict(principal);
            }
            Err(DataError::Invalid(errors)) => {
                writer.rollback().await?;
                form.reject(&errors);
                return self.form_page(principal, entry, form, Some(&record), sets(built), 422);
            }
            Err(other) => return Err(abandon(writer, other).await),
        }
        let mut told = vec![Told {
            entry,
            key: record.clone(),
            label: entry.admin.label_for(&entry.descriptor, &row),
            kind: Written::Updated,
            fields: crate::journal::changes(Some(&row), &values),
        }];
        let mut after = row.clone();
        for (name, value) in &values {
            after.set(name.clone(), value.clone());
        }
        if !self.finish(writer, &mut built, &after, &mut told).await? {
            form.add_error(NOT_SAVED);
            return self.form_page(principal, entry, form, Some(&record), sets(built), 422);
        }
        self.tell_all(principal, told);
        Ok(Response::Redirect {
            status: 303,
            location: self.detail_url(model, &record),
        })
    }

    /// The delete confirmation.
    pub async fn delete_confirm(
        &self,
        principal: Principal<'_>,
        model: &str,
        key: &str,
    ) -> Result<Response, AdminError> {
        let Some(entry) = self.entry(model) else {
            return self.missing(principal);
        };
        let Some(record) = PrimaryKey::parse(&entry.descriptor, key) else {
            return self.missing(principal);
        };
        if !entry.admin.allows_delete()
            || !self.allows(principal, Request::on(Action::Delete, model, &record))
        {
            return self.refuse(principal, Action::Delete);
        }
        let gate = self.gate(principal, entry, RowAction::Delete)?;
        let Some(scope) = gate.scope() else {
            return self.refuse(principal, Action::Delete);
        };
        let Some(row) = self.data.fetch(&entry.descriptor, &record, scope).await? else {
            return self.missing(principal);
        };
        let label = entry.admin.label_for(&entry.descriptor, &row);
        let context = DeleteCtx {
            chrome: self.chrome(principal, model),
            model: self.heading(entry),
            key: record.to_segment(),
            action: self.delete_url(model, &record),
            cancel_href: self.detail_url(model, &record),
            label: label.clone(),
        };
        let body = view::render(
            entry
                .admin
                .views()
                .confirm_delete
                .as_deref()
                .unwrap_or(self.views.confirm_delete.as_ref()),
            &context,
        )?;
        self.page(200, principal, model, format!("Delete {label}"), body)
    }

    /// Takes a delete.
    pub async fn delete(
        &self,
        principal: Principal<'_>,
        model: &str,
        key: &str,
    ) -> Result<Response, AdminError> {
        let Some(entry) = self.entry(model) else {
            return self.missing(principal);
        };
        let Some(record) = PrimaryKey::parse(&entry.descriptor, key) else {
            return self.missing(principal);
        };
        if !entry.admin.allows_delete()
            || !self.allows(principal, Request::on(Action::Delete, model, &record))
        {
            return self.refuse(principal, Action::Delete);
        }
        let Some(pending) = self
            .rules(entry)
            .admission(&principal, RowAction::Delete, &record)?
        else {
            return self.refuse(principal, Action::Delete);
        };
        let found = self
            .data
            .fetch(&entry.descriptor, &record, pending.scope())
            .await?;
        let Ok(guard) = pending.resolve(usize::from(found.is_some())) else {
            return self.missing(principal);
        };
        let removed = self
            .data
            .delete(&entry.descriptor, &record, guard.scope())
            .await?;
        if guard.applied(removed).is_err() {
            return self.conflict(principal);
        }
        if let Some(row) = &found {
            let label = entry.admin.label_for(&entry.descriptor, row);
            let fields = crate::journal::removed(row);
            self.tell(principal, entry, &record, &label, Written::Deleted, &fields);
        }
        Ok(Response::Redirect {
            status: 303,
            location: self.list_url(model),
        })
    }

    // --------------------------------------------------------------- routing

    /// Routes one request to a screen.
    ///
    /// The core publishes no route-registration seam yet, so an application
    /// mounts the admin by calling this from one catch-all handler. The body
    /// is already-parsed form pairs: this crate does not read a request body
    /// and does not link anything that could.
    pub async fn dispatch(
        &self,
        principal: Principal<'_>,
        method: &str,
        path: &str,
        query: &str,
        submission: &Submission,
    ) -> Result<Response, AdminError> {
        if let Some(route) = self.page_route(method, path) {
            return self.model_page(principal, route, query, submission).await;
        }
        let keys = self.panel_keys();
        let Some(route) = Route::parse(&self.base_path, method, path, &keys) else {
            return self.missing(principal);
        };
        match route {
            Route::Index => self.index(principal).await,
            Route::Panel { key, rest, post } => {
                self.panel(principal, key, rest, query, post, submission)
                    .await
            }
            Route::List(model) => {
                // `done` and `n` come from the redirect after an action. The
                // line names the action by its label, so a hand-made link
                // can show a count but no text of its own.
                let done = crate::query::param(query, "done")
                    .zip(crate::query::param(query, "n").and_then(|n| n.parse::<usize>().ok()));
                self.list_with(principal, model, ListQuery::parse(query), done)
                    .await
            }
            Route::Action(model) => self.act(principal, model, submission).await,
            Route::New(model) => self.create_form(principal, model).await,
            Route::Create(model) => self.create(principal, model, submission).await,
            Route::Detail(model, key) => self.detail(principal, model, key).await,
            Route::Edit(model, key) => self.edit_form(principal, model, key).await,
            Route::Update(model, key) => self.update(principal, model, key, submission).await,
            Route::ConfirmDelete(model, key) => self.delete_confirm(principal, model, key).await,
            Route::Delete(model, key) => self.delete(principal, model, key).await,
        }
    }

    /// One panel's screen.
    ///
    /// Reading asks the policy for [`Action::ViewPanel`]; a POST asks for
    /// [`Action::ActOnPanel`] as well, and is refused before the panel is
    /// called rather than after it has done the thing.
    async fn panel(
        &self,
        principal: Principal<'_>,
        key: &str,
        rest: &str,
        query: &str,
        post: bool,
        submission: &Submission,
    ) -> Result<Response, AdminError> {
        let Some(panel) = self.panels.iter().find(|p| p.key() == key) else {
            return self.missing(principal);
        };
        if !self.allows(principal, Request::new(Action::ViewPanel, key)) {
            return self.refuse(principal, Action::ViewPanel);
        }
        // Asked on every request and not only on a POST, so a panel can
        // draw a screen with no buttons for somebody who may not press
        // them. The refusal below is the enforcement; this is the courtesy.
        let can_act = self.allows(principal, Request::new(Action::ActOnPanel, key));
        if post && !can_act {
            return self.refuse(principal, Action::ActOnPanel);
        }
        let mount = self.panel_url(key);
        let request = PanelRequest {
            path: rest,
            query,
            post,
            can_act,
            submission,
            mount: &mount,
        };
        match panel.render(request).await? {
            // The panel's own 404, rendered as the admin's, so a panel does
            // not have to carry a copy of this page.
            None => self.missing(principal),
            Some(PanelPage::Redirect { location }) => Ok(Response::Redirect {
                status: 303,
                location,
            }),
            Some(PanelPage::Body { heading, html }) => {
                self.page(200, principal, key, heading, html)
            }
        }
    }

    /// A path that reaches a page under a model or under a record, if it
    /// is one. Asked before [`Route::parse`], because a page key wins over
    /// a record key at the same place.
    fn page_route<'a>(&self, method: &str, path: &'a str) -> Option<PageRoute<'a>> {
        let post = method.eq_ignore_ascii_case("post");
        if !post && !method.eq_ignore_ascii_case("get") {
            return None;
        }
        let rest = match self.base_path.is_empty() {
            true => path,
            false => path.strip_prefix(self.base_path.as_str())?,
        };
        if !rest.is_empty() && !rest.starts_with('/') {
            return None;
        }
        let rest = rest.trim_matches('/');
        let mut parts = rest.splitn(3, '/');
        let model = parts.next()?;
        let second = parts.next()?;
        let after = parts.next().unwrap_or("");
        let entry = self.entry(model)?;
        let pages = entry.admin.pages();
        if pages.iter().any(|p| !p.record && p.key == second) {
            return Some(PageRoute {
                model,
                record: None,
                page: second,
                rest: after,
                post,
            });
        }
        let (page, rest) = after.split_once('/').unwrap_or((after, ""));
        pages
            .iter()
            .any(|p| p.record && p.key == page)
            .then_some(PageRoute {
                model,
                record: Some(second),
                page,
                rest,
                post,
            })
    }

    /// A page under a model or under a record, with the list's or the
    /// record's policy asked first.
    async fn model_page(
        &self,
        principal: Principal<'_>,
        route: PageRoute<'_>,
        query: &str,
        submission: &Submission,
    ) -> Result<Response, AdminError> {
        let model = route.model;
        let Some(entry) = self.entry(model) else {
            return self.missing(principal);
        };
        let Some(page) = entry
            .admin
            .pages()
            .iter()
            .find(|p| p.record == route.record.is_some() && p.key == route.page)
        else {
            return self.missing(principal);
        };
        let (mount, record, can_act) = match route.record {
            None => {
                // The list's policy: the screen and the list rule.
                if !self.allows(principal, Request::new(Action::ViewList, model)) {
                    return self.refuse(principal, Action::ViewList);
                }
                if !self.opens(principal, entry, RowAction::List) {
                    return self.refuse(principal, Action::ViewList);
                }
                let can_act = entry.admin.allows_update()
                    && self.allows(principal, Request::new(Action::Update, model));
                (
                    format!("{}/{}", self.list_url(model), page.key),
                    None,
                    can_act,
                )
            }
            Some(raw) => {
                let Some(record) = PrimaryKey::parse(&entry.descriptor, raw) else {
                    return self.missing(principal);
                };
                // The detail's policy: the screen, then one guarded select
                // under the view rule. Refused and missing are one answer.
                if !self.allows(principal, Request::on(Action::ViewDetail, model, &record)) {
                    return self.refuse(principal, Action::ViewDetail);
                }
                let gate = self.gate(principal, entry, RowAction::View)?;
                let Some(scope) = gate.scope() else {
                    return self.refuse(principal, Action::ViewDetail);
                };
                let Some(row) = self.data.fetch(&entry.descriptor, &record, scope).await? else {
                    return self.missing(principal);
                };
                let screen = entry.admin.allows_update()
                    && self.allows(principal, Request::on(Action::Update, model, &record));
                if route.post && !screen {
                    return self.refuse(principal, Action::Update);
                }
                // A record the update rule refuses has no edit form, and no
                // POST here either: the same 404.
                let rule = self
                    .admits(principal, entry, RowAction::Update, &record)
                    .await?;
                if route.post && !rule {
                    return self.missing(principal);
                }
                let mount = format!("{}/{}", self.detail_url(model, &record), page.key);
                (mount, Some((record, row)), screen && rule)
            }
        };
        if route.post && !can_act {
            return self.refuse(principal, Action::Update);
        }
        let cx = PageCx {
            admin: self,
            model,
            principal,
            request: PanelRequest {
                path: route.rest,
                query,
                post: route.post,
                can_act,
                submission,
                mount: &mount,
            },
            key: record.as_ref().map(|(key, _)| key),
            record: record.as_ref().map(|(_, row)| row),
        };
        match (page.render)(cx).await? {
            None => self.missing(principal),
            Some(PanelPage::Redirect { location }) => Ok(Response::Redirect {
                status: 303,
                location,
            }),
            Some(PanelPage::Body { heading, html }) => {
                self.page(200, principal, model, heading, html)
            }
        }
    }

    // --------------------------------------------------------------- helpers

    fn panel_url(&self, key: &str) -> String {
        format!("{}/{key}", self.base_path)
    }

    fn entry(&self, key: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.descriptor.key == key)
    }

    fn allows(&self, principal: Principal<'_>, request: Request<'_>) -> bool {
        self.policy.allows(&principal, request)
    }

    /// What governs one model's rows.
    fn rules<'a>(&self, entry: &'a Entry) -> Rules<'a> {
        match &entry.rules {
            Some(rules) => Rules::Resource(rules.as_ref()),
            None if self.unscoped_rows => Rules::Unscoped,
            None => Rules::Missing,
        }
    }

    /// One rule, compiled for this principal.
    fn gate(
        &self,
        principal: Principal<'_>,
        entry: &Entry,
        action: RowAction,
    ) -> Result<Gate, AdminError> {
        Ok(self.rules(entry).gate(&principal, action)?)
    }

    /// Whether a rule is anything but shut. For a link, never for a write:
    /// a policy that does not compile hides the link here and answers 500 on
    /// the screen itself.
    fn opens(&self, principal: Principal<'_>, entry: &Entry, action: RowAction) -> bool {
        self.rules(entry)
            .gate(&principal, action)
            .is_ok_and(|gate| !gate.is_shut())
    }

    /// Whether the index and the navigation show a model to this person.
    fn lists(&self, principal: Principal<'_>, entry: &Entry) -> bool {
        self.allows(
            principal,
            Request::new(Action::ViewList, &entry.descriptor.key),
        ) && self.opens(principal, entry, RowAction::List)
    }

    /// Whether the create rule admits every row. See [`crate::rows`] for why
    /// a conditional create rule is refused.
    fn creates(&self, principal: Principal<'_>, entry: &Entry) -> bool {
        self.rules(entry)
            .gate(&principal, RowAction::Create)
            .is_ok_and(|gate| gate.admits_everything())
    }

    /// The create rule, as a refusal page when it does not admit every row.
    fn refuse_create(
        &self,
        principal: Principal<'_>,
        entry: &Entry,
    ) -> Result<Option<Response>, AdminError> {
        match self.gate(principal, entry, RowAction::Create)? {
            gate if gate.admits_everything() => Ok(None),
            Gate::Shut => self.refuse(principal, Action::Create).map(Some),
            _ => self
                .error_page(
                    principal,
                    403,
                    "Not allowed",
                    "This record's create rule depends on the record, and the admin \
                     cannot decide such a rule yet."
                        .to_string(),
                )
                .map(Some),
        }
    }

    /// Draws every field the row policy's mask for `write` refuses as
    /// read-only.
    fn lock(&self, principal: Principal<'_>, entry: &Entry, write: Write, form: &mut Form) {
        let rules = self.rules(entry);
        form.lock(|column| rules.writable(&principal, write, column));
    }

    /// The mask for `write` (`Policy::writable_on_create` or
    /// `Policy::writable`), checked against every column a request sent.
    ///
    /// The columns *sent*, as `brocanth-resource` checks them, and before
    /// any row is read, so the 403 says nothing about whether the row
    /// exists. The admin's own form never sends a locked field — it draws one
    /// as read-only text with no `name` — so a request that sends one was not
    /// made with the form. A name that is not a column of the model is not
    /// checked: [`Form::accept`] ignores it.
    fn refuse_fields(
        &self,
        principal: Principal<'_>,
        entry: &Entry,
        write: Write,
        submission: &Submission,
    ) -> Result<Option<Response>, AdminError> {
        let mut sent: Vec<&str> = Vec::new();
        for (name, _) in submission.pairs() {
            if entry.descriptor.column(name).is_some() && !sent.contains(&name.as_str()) {
                sent.push(name);
            }
        }
        if sent.is_empty() {
            return Ok(None);
        }
        match self.rules(entry).check(&principal, write, &sent) {
            Ok(()) => Ok(None),
            Err(MaskViolation::Forbidden { fields }) => self
                .error_page(
                    principal,
                    403,
                    "Not allowed",
                    format!("You may not change: {}.", fields.join(", ")),
                )
                .map(Some),
            Err(unknown) => Err(AdminError::Mask(unknown)),
        }
    }

    /// A guarded write changed nothing: the row stopped matching its rule
    /// between the check and the write. 409, because the next move is to
    /// read the row again, and a 404 would be wrong while the row is still
    /// there.
    fn conflict(&self, principal: Principal<'_>) -> Result<Response, AdminError> {
        self.error_page(
            principal,
            409,
            "Changed meanwhile",
            "This record changed while you were working on it, and you may no longer \
             change it. Reload it and try again."
                .to_string(),
        )
    }

    fn heading(&self, entry: &Entry) -> ModelHeading {
        ModelHeading {
            key: entry.descriptor.key.clone(),
            name: entry.admin.name(&entry.descriptor),
            plural: entry.admin.plural(&entry.descriptor),
            href: self.list_url(&entry.descriptor.key),
        }
    }

    fn chrome(&self, principal: Principal<'_>, current: &str) -> Chrome {
        // The admin's stylesheets, then the shown model's, so a model's rule
        // wins over the admin's when both set one property.
        let stylesheets = self
            .stylesheets
            .iter()
            .chain(
                self.entry(current)
                    .map(|e| e.admin.stylesheets())
                    .unwrap_or_default(),
            )
            .cloned()
            .collect();
        // The same order for scripts: the admin's, then the model's.
        let scripts = self
            .scripts
            .iter()
            .chain(
                self.entry(current)
                    .map(|e| e.admin.scripts())
                    .unwrap_or_default(),
            )
            .cloned()
            .collect();
        Chrome {
            title: self.title.clone(),
            header: self.header.clone(),
            site_url: self.site_url.clone(),
            stylesheets,
            scripts,
            user: self.user_label.as_ref().and_then(|label| label(&principal)),
            base_path: self.index_url(),
            nav: self
                .entries
                .iter()
                .filter(|e| self.lists(principal, e))
                .map(|e| NavItem {
                    label: e.admin.plural(&e.descriptor),
                    href: self.list_url(&e.descriptor.key),
                    current: e.descriptor.key == current,
                    key: e.descriptor.key.clone(),
                })
                // The panels, after the models and in the order they were
                // added. Filtered by the same policy call the screen makes,
                // so a tab nobody may open is not a tab anybody is shown.
                .chain(
                    self.panels
                        .iter()
                        // A hidden page is reached by a link, not a tab.
                        .filter(|p| p.in_nav())
                        .filter(|p| {
                            self.allows(principal, Request::new(Action::ViewPanel, p.key()))
                        })
                        .map(|p| NavItem {
                            label: p.label(),
                            href: self.panel_url(p.key()),
                            current: p.key() == current,
                            key: p.key().to_string(),
                        }),
                )
                .collect(),
        }
    }

    fn page(
        &self,
        status: u16,
        principal: Principal<'_>,
        current: &str,
        heading: String,
        body: String,
    ) -> Result<Response, AdminError> {
        self.wrap(status, self.chrome(principal, current), heading, body)
    }

    /// Renders the two blocks, then the layout around `body`.
    fn wrap(
        &self,
        status: u16,
        chrome: Chrome,
        heading: String,
        body: String,
    ) -> Result<Response, AdminError> {
        let context = LayoutCtx {
            brand: view::render(self.views.brand.as_ref(), &chrome)?,
            userlinks: view::render(self.views.userlinks.as_ref(), &chrome)?,
            chrome,
            heading,
            body,
        };
        Ok(Response::Page {
            status,
            html: view::render(self.views.layout.as_ref(), &context)?,
        })
    }

    fn refuse(&self, principal: Principal<'_>, action: Action) -> Result<Response, AdminError> {
        self.error_page(
            principal,
            403,
            "Not allowed",
            format!("You do not have permission to {action}."),
        )
    }

    fn missing(&self, principal: Principal<'_>) -> Result<Response, AdminError> {
        self.error_page(
            principal,
            404,
            "Not found",
            "There is nothing at this address.".to_string(),
        )
    }

    fn error_page(
        &self,
        principal: Principal<'_>,
        status: u16,
        heading: &str,
        message: String,
    ) -> Result<Response, AdminError> {
        // The chrome is built for whoever asked, so a 403 does not leak the
        // list of models to someone the policy refused.
        let chrome = self.chrome(principal, "");
        let context = ErrorCtx {
            chrome: chrome.clone(),
            status,
            heading: heading.to_string(),
            message,
        };
        let body = view::render(self.views.error.as_ref(), &context)?;
        self.wrap(status, chrome, heading.to_string(), body)
    }

    /// The create or edit form. 200 for a fresh one; 422 for a rejected
    /// one, with the same form value, so nothing the person typed is lost.
    fn form_page(
        &self,
        principal: Principal<'_>,
        entry: &Entry,
        form: Form,
        record: Option<&PrimaryKey>,
        inlines: Vec<InlineSet>,
        status: u16,
    ) -> Result<Response, AdminError> {
        let model = &entry.descriptor.key;
        let is_new = record.is_none();
        let name = entry.admin.name(&entry.descriptor);
        let context = FormCtx {
            chrome: self.chrome(principal, model),
            model: self.heading(entry),
            action: match record {
                Some(key) => self.detail_url(model, key),
                None => self.list_url(model),
            },
            cancel_href: match record {
                Some(key) => self.detail_url(model, key),
                None => self.list_url(model),
            },
            submit_label: match is_new {
                true => format!("Create {}", name.to_lowercase()),
                false => "Save changes".to_string(),
            },
            is_new,
            form,
            inlines,
        };
        let heading = match is_new {
            true => format!("Add {}", name.to_lowercase()),
            false => format!("Edit {}", name.to_lowercase()),
        };
        let body = view::render(
            entry
                .admin
                .views()
                .form
                .as_deref()
                .unwrap_or(self.views.form.as_ref()),
            &context,
        )?;
        self.page(status, principal, model, heading, body)
    }

    // --------------------------------------------------------------- inlines

    /// Builds every inline of a form for one request: the rows to show and,
    /// for a POST, the writes the rows ask for.
    ///
    /// Every check runs here, before any write: the screen policy and the
    /// row rule per child row, the field mask, and the form checks. A
    /// refusal comes back as `Err(page)`. The writes run later, in
    /// [`Admin::write_inlines`], once the parent and every row are valid.
    async fn inlines<'a>(
        &'a self,
        principal: Principal<'_>,
        entry: &'a Entry,
        parent: Option<&Row>,
        submission: Option<&Submission>,
    ) -> Result<Result<Vec<Built<'a>>, Response>, AdminError> {
        let mut out = Vec::new();
        for inline in entry.admin.inlines() {
            let Some(child) = self.entry(&inline.model) else {
                continue;
            };
            let prefix = inline.prefix();
            let cap = inline.max + inline.extra;
            let posted = match submission {
                None => Vec::new(),
                Some(submission) => match crate::inline::posted(submission, prefix, cap) {
                    Some(rows) => rows,
                    None => {
                        return self
                            .error_page(
                                principal,
                                422,
                                "Too many rows",
                                format!(
                                    "The form takes at most {cap} rows of {}.",
                                    child.admin.plural(&child.descriptor).to_lowercase()
                                ),
                            )
                            .map(Err)
                    }
                },
            };
            // A person who may not list the children does not see the
            // inline, and a POST that sends rows for it was not made with
            // this form.
            if !self.lists(principal, child) {
                if posted.is_empty() {
                    continue;
                }
                return self.refuse(principal, Action::ViewList).map(Err);
            }
            let fk = inline.fk.as_deref().unwrap_or_default();
            let fk_kind = child
                .descriptor
                .column(fk)
                .map_or(ColumnKind::Unknown, |c| c.kind);
            let parent_value = parent
                .and_then(|row| row.get(&inline.parent_column))
                .filter(|value| !value.is_null())
                .map(|value| typed(fk_kind, &value.as_input()));
            let can_add = child.admin.allows_create()
                && self.allows(
                    principal,
                    Request::new(Action::Create, &child.descriptor.key),
                )
                && self.creates(principal, child);
            let choices = self
                .choices(principal, &child.admin, &child.descriptor)
                .await?;
            let names: Vec<String> = match inline.fields.is_empty() {
                true => child
                    .admin
                    .form_for(&child.descriptor)
                    .iter()
                    .map(|c| c.name.clone())
                    .filter(|name| name != fk)
                    .collect(),
                false => inline
                    .fields
                    .iter()
                    .filter(|name| *name != fk)
                    .cloned()
                    .collect(),
            };
            let build = |row: Option<&Row>| {
                let write = if row.is_some() {
                    Write::Update
                } else {
                    Write::Create
                };
                let mut form = Form::build(&child.descriptor, &child.admin, row, &choices);
                form.fields.retain(|f| names.contains(&f.name));
                form.fields
                    .sort_by_key(|f| names.iter().position(|n| *n == f.name));
                self.lock(principal, child, write, &mut form);
                form
            };

            // The existing children, read under the child's list rule with
            // the parent's key in the query.
            let mut existing = Vec::new();
            let mut more = false;
            if let Some(value) = &parent_value {
                let gate = self.gate(principal, child, RowAction::List)?;
                let Some(scope) = gate.scope() else {
                    return self.refuse(principal, Action::ViewList).map(Err);
                };
                let sort =
                    child
                        .descriptor
                        .key_columns()
                        .first()
                        .map(|column| crate::query::Sort {
                            column: column.name.clone(),
                            direction: Direction::Ascending,
                        });
                let mut rows = self
                    .data
                    .list(ListRequest {
                        model: &child.descriptor,
                        offset: 0,
                        limit: inline.max + 1,
                        search: Search {
                            term: "",
                            columns: &[],
                        },
                        sort: sort.as_ref(),
                        filter: Some(Equals { column: fk, value }),
                        scope,
                    })
                    .await?;
                more = rows.len() > inline.max;
                rows.truncate(inline.max);
                existing = rows;
            }

            let mut rows: Vec<InlineRow> = Vec::new();
            let mut befores: Vec<Option<(PrimaryKey, Row)>> = Vec::new();
            let mut can_update: Vec<bool> = Vec::new();
            for row in existing {
                let Some(key) = PrimaryKey::from_row(&child.descriptor, &row) else {
                    continue;
                };
                let child_key = &child.descriptor.key;
                let update = child.admin.allows_update()
                    && self.allows(principal, Request::on(Action::Update, child_key, &key))
                    && self
                        .admits(principal, child, RowAction::Update, &key)
                        .await?;
                let delete = child.admin.allows_delete()
                    && self.allows(principal, Request::on(Action::Delete, child_key, &key))
                    && self
                        .admits(principal, child, RowAction::Delete, &key)
                        .await?;
                let mut form = build(Some(&row));
                if !update {
                    form.lock(|_| false);
                }
                rows.push(InlineRow {
                    prefix: String::new(),
                    key: Some(key.to_segment()),
                    label: child.admin.label_for(&child.descriptor, &row),
                    href: Some(self.detail_url(child_key, &key)),
                    form,
                    can_delete: delete,
                    delete: false,
                });
                befores.push(Some((key, row)));
                can_update.push(update);
            }

            let mut ops = Vec::new();
            let mut added = 0;
            let mut seen: Vec<&str> = Vec::new();
            for row in &posted {
                match &row.key {
                    Some(segment) => {
                        // The key must be one of this parent's children on
                        // the form. One that is not was deleted meanwhile,
                        // moved to another parent, or never was this one's.
                        // A key posted twice was not posted by the form.
                        let Some(at) = rows.iter().position(|r| r.key.as_ref() == Some(segment))
                        else {
                            return self.conflict(principal).map(Err);
                        };
                        if seen.contains(&segment.as_str()) {
                            return self.conflict(principal).map(Err);
                        }
                        seen.push(segment);
                        let Some((key, before)) = befores[at].clone() else {
                            continue;
                        };
                        if row.delete {
                            if !rows[at].can_delete {
                                return self.refuse(principal, Action::Delete).map(Err);
                            }
                            rows[at].delete = true;
                            match self
                                .admit(
                                    principal,
                                    child,
                                    RowAction::Delete,
                                    &key,
                                    fk,
                                    parent_value.as_ref(),
                                )
                                .await?
                            {
                                Some(guard) => ops.push(Op::Delete {
                                    row: at,
                                    key,
                                    before,
                                    guard,
                                }),
                                None => return self.conflict(principal).map(Err),
                            }
                            continue;
                        }
                        if let Some(refused) =
                            self.refuse_fields(principal, child, Write::Update, &row.fields)?
                        {
                            return Ok(Err(refused));
                        }
                        let sent = row
                            .fields
                            .pairs()
                            .iter()
                            .any(|(name, _)| child.descriptor.column(name).is_some());
                        if !can_update[at] {
                            if sent {
                                return self.refuse(principal, Action::Update).map(Err);
                            }
                            continue;
                        }
                        let form = &mut rows[at].form;
                        form.accept(&row.fields);
                        if !form.validate() {
                            continue;
                        }
                        let values = form.values();
                        if !changed(&before, &values) {
                            continue;
                        }
                        match self
                            .admit(
                                principal,
                                child,
                                RowAction::Update,
                                &key,
                                fk,
                                parent_value.as_ref(),
                            )
                            .await?
                        {
                            Some(guard) => ops.push(Op::Update {
                                row: at,
                                key,
                                before,
                                values,
                                guard,
                            }),
                            None => return self.conflict(principal).map(Err),
                        }
                    }
                    None if row.is_blank() => {}
                    None => {
                        if !can_add {
                            return self.refuse(principal, Action::Create).map(Err);
                        }
                        if let Some(refused) =
                            self.refuse_fields(principal, child, Write::Create, &row.fields)?
                        {
                            return Ok(Err(refused));
                        }
                        // The admin writes the parent's key into the child,
                        // so the mask must let this person write it.
                        if let Some(refused) = self.refuse_fields(
                            principal,
                            child,
                            Write::Create,
                            &Submission::new().with(fk, ""),
                        )? {
                            return Ok(Err(refused));
                        }
                        let mut form = build(None);
                        form.accept(&row.fields);
                        let valid = form.validate();
                        let at = rows.len();
                        if valid {
                            ops.push(Op::Insert {
                                row: at,
                                values: form.values(),
                            });
                        }
                        rows.push(InlineRow {
                            prefix: String::new(),
                            key: None,
                            label: String::new(),
                            href: None,
                            form,
                            can_delete: false,
                            delete: false,
                        });
                        befores.push(None);
                        can_update.push(true);
                        added += 1;
                    }
                }
            }
            if can_add {
                for _ in added..inline.extra {
                    rows.push(InlineRow {
                        prefix: String::new(),
                        key: None,
                        label: String::new(),
                        href: None,
                        form: build(None),
                        can_delete: false,
                        delete: false,
                    });
                }
            }
            out.push(Built {
                child,
                fk,
                fk_kind,
                parent_column: &inline.parent_column,
                set: InlineSet {
                    model: self.heading(child),
                    columns: build(None).fields.iter().map(|f| f.label.clone()).collect(),
                    rows,
                    more,
                },
                ops,
            });
        }
        for built in &mut out {
            number(&mut built.set);
        }
        Ok(Ok(out))
    }

    /// Whether a row rule admits one record: no read when the rule is shut
    /// or admits every row, one guarded select otherwise.
    async fn admits(
        &self,
        principal: Principal<'_>,
        entry: &Entry,
        action: RowAction,
        key: &PrimaryKey,
    ) -> Result<bool, AdminError> {
        let gate = self.gate(principal, entry, action)?;
        if gate.admits_everything() {
            return Ok(true);
        }
        let Some(scope) = gate.scope() else {
            return Ok(false);
        };
        Ok(self
            .data
            .fetch(&entry.descriptor, key, scope)
            .await?
            .is_some())
    }

    /// The admission for one child write: the guarded select, and the
    /// check that the child still points at this parent. `None` when the
    /// rule refuses the child, it is gone, or it moved.
    async fn admit(
        &self,
        principal: Principal<'_>,
        child: &Entry,
        action: RowAction,
        key: &PrimaryKey,
        fk: &str,
        parent: Option<&Value>,
    ) -> Result<Option<Guard>, AdminError> {
        let Some(pending) = self.rules(child).admission(&principal, action, key)? else {
            return Ok(None);
        };
        let found = self
            .data
            .fetch(&child.descriptor, key, pending.scope())
            .await?;
        let belongs = found
            .as_ref()
            .is_some_and(|row| row.get(fk).map(Value::as_input) == parent.map(Value::as_input));
        if !belongs {
            return Ok(None);
        }
        Ok(pending.resolve(1).ok())
    }

    /// Where a save writes. A save with inline writes opens one
    /// transaction, so the parent and its children are kept together or not
    /// at all. A save of one row writes straight to the data source.
    async fn writer(&self, built: &[Built<'_>]) -> Result<Writer<'_>, DataError> {
        if built.iter().all(|b| b.ops.is_empty()) {
            return Ok(Writer::Direct(self.data.as_ref()));
        }
        Ok(Writer::Transaction(self.data.begin().await?))
    }

    /// Runs the inline writes after the parent's own, then keeps or
    /// discards all of them.
    ///
    /// `Ok(true)`: the writes are committed. `Ok(false)`: a row was
    /// refused, it carries the error, and nothing is kept. The journal hears
    /// nothing here; the caller tells it once the writes are kept.
    async fn finish<'e>(
        &self,
        mut writer: Writer<'_>,
        built: &mut [Built<'e>],
        parent: &Row,
        told: &mut Vec<Told<'e>>,
    ) -> Result<bool, AdminError> {
        match self.write_inlines(&mut writer, built, parent, told).await {
            Ok(true) => {
                writer.commit().await?;
                Ok(true)
            }
            Ok(false) => {
                writer.rollback().await?;
                Ok(false)
            }
            Err(error) => Err(abandon(writer, error).await),
        }
    }

    /// Runs the writes the inlines planned, in form order.
    ///
    /// `parent` is the parent row as written, for the key a new child
    /// holds. Stops at the first row the data source refuses and returns
    /// `false`: that row carries the error, and [`Admin::finish`] discards
    /// every write. The sets stay as they were posted, so the form that
    /// comes back is the one the person sent.
    async fn write_inlines<'e>(
        &self,
        writer: &mut Writer<'_>,
        built: &mut [Built<'e>],
        parent: &Row,
        told: &mut Vec<Told<'e>>,
    ) -> Result<bool, AdminError> {
        for built in built.iter_mut() {
            let child = built.child;
            let parent_value = parent
                .get(built.parent_column)
                .map(|value| typed(built.fk_kind, &value.as_input()))
                .unwrap_or(Value::Null);
            for op in std::mem::take(&mut built.ops) {
                match op {
                    Op::Delete {
                        row,
                        key,
                        before,
                        guard,
                    } => {
                        let removed = writer
                            .delete(&child.descriptor, &key, guard.scope())
                            .await?;
                        if guard.applied(removed).is_err() {
                            built.set.rows[row].form.add_error(CHANGED_MEANWHILE);
                            return Ok(false);
                        }
                        told.push(Told {
                            entry: child,
                            label: child.admin.label_for(&child.descriptor, &before),
                            fields: crate::journal::removed(&before),
                            kind: Written::Deleted,
                            key,
                        });
                    }
                    Op::Update {
                        row,
                        key,
                        before,
                        values,
                        guard,
                    } => {
                        let written = writer
                            .update(&child.descriptor, &key, &values, guard.scope())
                            .await;
                        match written.map(|rows| guard.applied(rows)) {
                            Ok(Ok(_)) => told.push(Told {
                                entry: child,
                                label: child.admin.label_for(&child.descriptor, &before),
                                fields: crate::journal::changes(Some(&before), &values),
                                kind: Written::Updated,
                                key,
                            }),
                            Ok(Err(_lost)) => {
                                built.set.rows[row].form.add_error(CHANGED_MEANWHILE);
                                return Ok(false);
                            }
                            Err(DataError::Invalid(errors)) => {
                                built.set.rows[row].form.reject(&errors);
                                return Ok(false);
                            }
                            Err(other) => return Err(other.into()),
                        }
                    }
                    Op::Insert { row, mut values } => {
                        values.push((built.fk.to_string(), parent_value.clone()));
                        match writer.insert(&child.descriptor, &values).await {
                            Ok(key) => told.push(Told {
                                entry: child,
                                label: child.admin.label_for(&child.descriptor, &row_of(&values)),
                                fields: crate::journal::changes(None, &values),
                                kind: Written::Created,
                                key,
                            }),
                            Err(DataError::Invalid(errors)) => {
                                built.set.rows[row].form.reject(&errors);
                                return Ok(false);
                            }
                            Err(other) => return Err(other.into()),
                        }
                    }
                }
            }
        }
        Ok(true)
    }

    /// Tells the journal about the writes of one save, once they are kept.
    fn tell_all(&self, principal: Principal<'_>, told: Vec<Told<'_>>) {
        for t in told {
            self.tell(principal, t.entry, &t.key, &t.label, t.kind, &t.fields);
        }
    }

    /// Reads the options for every foreign-key field on a form.
    ///
    /// The labels come from the *target* model's configuration, which is why
    /// the admin fetches them rather than the data source: naming a record is
    /// a configuration decision, and the data layer has no idea.
    ///
    /// The options are read under the **target** model's list rule, so a
    /// picker never offers a record this person may not list.
    async fn choices(
        &self,
        principal: Principal<'_>,
        admin: &ModelAdmin,
        descriptor: &ModelDescriptor,
    ) -> Result<Vec<(String, Vec<Choice>)>, AdminError> {
        let mut out = Vec::new();
        for column in admin.form_for(descriptor) {
            let Some(relation) = admin.relation_for(column) else {
                continue;
            };
            let Some(target) = self.entry(&relation.target) else {
                continue;
            };
            let gate = self.gate(principal, target, RowAction::List)?;
            let Some(scope) = gate.scope() else {
                continue;
            };
            let limit = target.admin.choices_shown();
            let rows = self
                .data
                .list(ListRequest {
                    model: &target.descriptor,
                    offset: 0,
                    limit,
                    search: Search {
                        term: "",
                        columns: &[],
                    },
                    sort: None,
                    filter: None,
                    scope,
                })
                .await?;
            let choices = rows
                .iter()
                .filter_map(|row| {
                    let value = row.get(&relation.target_column)?;
                    Some(Choice::new(
                        value.as_input(),
                        target.admin.label_for(&target.descriptor, row),
                    ))
                })
                .collect();
            out.push((column.name.clone(), choices));
        }
        Ok(out)
    }

    /// Resolves every foreign key appearing in `rows` to a label.
    ///
    /// Keyed by `(model key, value)`, so two columns pointing at the same
    /// table share the lookups.
    ///
    /// Read under the **target** model's view rule. A record this person may
    /// not view keeps its stored value as the cell text, which is what a
    /// deleted target shows too, so the two cannot be told apart.
    async fn foreign_labels(
        &self,
        principal: Principal<'_>,
        admin: &ModelAdmin,
        columns: &[&ColumnDescriptor],
        rows: &[Row],
    ) -> Result<BTreeMap<(String, String), String>, AdminError> {
        let mut out: BTreeMap<(String, String), String> = BTreeMap::new();
        for column in columns {
            let Some(relation) = admin.relation_for(column) else {
                continue;
            };
            let Some(target) = self.entry(&relation.target) else {
                continue;
            };
            let gate = self.gate(principal, target, RowAction::View)?;
            let Some(scope) = gate.scope() else {
                continue;
            };
            for row in rows {
                let Some(value) = row.get(&column.name) else {
                    continue;
                };
                if value.is_null() {
                    continue;
                }
                let text = value.as_input();
                let slot = (relation.target.clone(), text.clone());
                if out.contains_key(&slot) {
                    continue;
                }
                let key = PrimaryKey::single(&relation.target_column, &text);
                if let Some(found) = self.data.fetch(&target.descriptor, &key, scope).await? {
                    out.insert(slot, target.admin.label_for(&target.descriptor, &found));
                }
            }
        }
        Ok(out)
    }

    fn header(
        &self,
        query: &ListQuery,
        sortable: &[String],
        model: &str,
        column: &ColumnDescriptor,
    ) -> ColumnHeader {
        let can_sort = sortable.contains(&column.name);
        let current = query
            .sort
            .as_ref()
            .filter(|s| s.column == column.name)
            .map(|s| s.direction);
        let next = current.map_or(Direction::Ascending, Direction::flipped);
        let href = match can_sort {
            true => {
                let mut target = query.clone();
                target.sort = Some(crate::query::Sort {
                    column: column.name.clone(),
                    direction: next,
                });
                format!("{}?{}", self.list_url(model), target.to_query_string(1))
            }
            false => String::new(),
        };
        ColumnHeader {
            name: column.name.clone(),
            label: column.label(),
            sortable: can_sort,
            href,
            sorted: match current {
                Some(Direction::Ascending) => "asc",
                Some(Direction::Descending) => "desc",
                None => "",
            },
        }
    }

    fn row_view(
        &self,
        principal: Principal<'_>,
        entry: &Entry,
        columns: &[&ColumnDescriptor],
        row: &Row,
        labels: &BTreeMap<(String, String), String>,
        (can_update, can_delete): (bool, bool),
    ) -> RowView {
        let model = &entry.descriptor.key;
        let key = PrimaryKey::from_row(&entry.descriptor, row);
        let cells = columns
            .iter()
            .map(|column| self.cell(&entry.admin, column, row, labels))
            .collect();
        let href = key
            .as_ref()
            .map(|k| self.detail_url(model, k))
            .unwrap_or_default();
        let edit_href = key.as_ref().filter(|k| {
            can_update
                && entry.admin.allows_update()
                && self.allows(principal, Request::on(Action::Update, model, k))
        });
        let delete_href = key.as_ref().filter(|k| {
            can_delete
                && entry.admin.allows_delete()
                && self.allows(principal, Request::on(Action::Delete, model, k))
        });
        RowView {
            key: key.as_ref().map(PrimaryKey::to_segment).unwrap_or_default(),
            label: entry.admin.label_for(&entry.descriptor, row),
            edit_href: edit_href.map(|k| self.edit_url(model, k)),
            delete_href: delete_href.map(|k| self.delete_url(model, k)),
            cells,
            href,
        }
    }

    /// One cell. **A foreign key becomes the target's label and a link, never
    /// the integer that is actually stored.**
    fn cell(
        &self,
        admin: &ModelAdmin,
        column: &ColumnDescriptor,
        row: &Row,
        labels: &BTreeMap<(String, String), String>,
    ) -> Cell {
        let value = row.get(&column.name).cloned().unwrap_or(Value::Null);
        match admin.relation_for(column) {
            Some(relation) if !value.is_null() => {
                let text = value.as_input();
                let slot = (relation.target.clone(), text.clone());
                let label = labels.get(&slot).cloned();
                let href = self.entry(&relation.target).map(|target| {
                    self.detail_url(
                        &target.descriptor.key,
                        &PrimaryKey::single(&relation.target_column, &text),
                    )
                });
                Cell {
                    column: column.name.clone(),
                    // The id is the fallback, and only when the target row has
                    // gone: an admin that shows `author: 41` has failed at the
                    // one thing it is for.
                    text: label.unwrap_or(text),
                    href,
                }
            }
            _ => Cell {
                column: column.name.clone(),
                text: truncate(&value.to_string(), 120),
                href: None,
            },
        }
    }

    // --------------------------------------------------------------- actions

    /// The actions this model's list could offer, before asking who is
    /// looking: every model's, minus the ones it leaves off or replaces,
    /// then its own; only those its configuration allows.
    fn offered<'a>(&'a self, entry: &'a Entry) -> Vec<&'a BulkAction> {
        let own = entry.admin.own_actions();
        let mut out: Vec<&BulkAction> = self
            .actions
            .iter()
            .filter(|a| !entry.admin.drops_action(&a.key) && !own.iter().any(|o| o.key == a.key))
            .filter(|a| {
                a.columns()
                    .iter()
                    .all(|c| entry.descriptor.column(c).is_some())
            })
            .collect();
        out.extend(own.iter());
        out.retain(|a| match a.needs {
            RowAction::Delete => entry.admin.allows_delete(),
            _ => entry.admin.allows_update(),
        });
        out
    }

    /// The actions this person may run on this model's list. Per subject,
    /// like the edit and delete links: a record the rule refuses is skipped
    /// when the action runs.
    fn actions_for<'a>(
        &'a self,
        principal: Principal<'_>,
        entry: &'a Entry,
    ) -> Vec<&'a BulkAction> {
        self.offered(entry)
            .into_iter()
            .filter(|a| {
                let columns = a.columns();
                self.allows(
                    principal,
                    Request::new(screen_action(a.needs), &entry.descriptor.key),
                ) && self.opens(principal, entry, a.needs)
                    && (columns.is_empty()
                        || self
                            .rules(entry)
                            .check(&principal, Write::Update, &columns)
                            .is_ok())
            })
            .collect()
    }

    /// Takes an action form: the confirmation without `confirm`, the writes
    /// with it.
    async fn act(
        &self,
        principal: Principal<'_>,
        model: &str,
        submission: &Submission,
    ) -> Result<Response, AdminError> {
        let Some(entry) = self.entry(model) else {
            return self.missing(principal);
        };
        let wanted = submission.get("action").unwrap_or("");
        let Some(action) = self.offered(entry).into_iter().find(|a| a.key == wanted) else {
            return self.error_page(
                principal,
                400,
                "No such action",
                format!("This list has no action called “{wanted}”."),
            );
        };
        let screen = screen_action(action.needs);
        if !self.allows(principal, Request::new(screen, model)) {
            return self.refuse(principal, screen);
        }
        // The field mask before any row is read, as on the edit form.
        let columns = action.columns();
        if !columns.is_empty() {
            match self.rules(entry).check(&principal, Write::Update, &columns) {
                Ok(()) => {}
                Err(MaskViolation::Forbidden { fields }) => {
                    return self.error_page(
                        principal,
                        403,
                        "Not allowed",
                        format!("You may not change: {}.", fields.join(", ")),
                    )
                }
                Err(unknown) => return Err(AdminError::Mask(unknown)),
            }
        }
        let gate = self.gate(principal, entry, action.needs)?;
        let Some(scope) = gate.scope() else {
            return self.refuse(principal, screen);
        };
        let mut keys: Vec<&str> = Vec::new();
        for (name, value) in submission.pairs() {
            if name == "pk" && !keys.contains(&value.as_str()) {
                keys.push(value);
            }
        }
        if keys.is_empty() {
            return self.error_page(
                principal,
                422,
                "Nothing selected",
                "Tick at least one record on the list, then choose the action again.".to_string(),
            );
        }
        if keys.len() > MAX_SELECTED {
            return self.error_page(
                principal,
                422,
                "Too many selected",
                format!("One action takes at most {MAX_SELECTED} records."),
            );
        }
        // Each record read under the action's rule: a record the rule
        // refuses and a record that is gone are one answer, skipped.
        let mut selected = Vec::new();
        let mut skipped = 0;
        for raw in keys {
            let Some(record) = PrimaryKey::parse(&entry.descriptor, raw) else {
                skipped += 1;
                continue;
            };
            if !self.allows(principal, Request::on(screen, model, &record)) {
                skipped += 1;
                continue;
            }
            match self.data.fetch(&entry.descriptor, &record, scope).await? {
                Some(row) => selected.push(Selected {
                    label: entry.admin.label_for(&entry.descriptor, &row),
                    key: record,
                    row,
                }),
                None => skipped += 1,
            }
        }
        if selected.is_empty() {
            return self.error_page(
                principal,
                404,
                "Nothing to act on",
                "None of the selected records exist, or you may not change them.".to_string(),
            );
        }
        if submission.get("confirm") != Some("yes") {
            let context = ActionCtx {
                chrome: self.chrome(principal, model),
                model: self.heading(entry),
                key: action.key.clone(),
                label: action.label.clone(),
                destructive: action.is_destructive(),
                records: selected
                    .iter()
                    .map(|s| SelectedRecord {
                        key: s.key.to_segment(),
                        label: s.label.clone(),
                        href: self.detail_url(model, &s.key),
                    })
                    .collect(),
                skipped,
                action: self.actions_url(model),
                cancel_href: self.list_url(model),
            };
            let body = view::render(self.views.confirm_action.as_ref(), &context)?;
            return self.page(200, principal, model, action.label.clone(), body);
        }
        let cx = ActionCx {
            admin: self,
            model,
            principal,
            selected: &selected,
        };
        let done = match &action.run {
            Run::Delete => {
                let mut done = 0;
                for record in &selected {
                    if cx.delete(&record.key).await? {
                        done += 1;
                    }
                }
                done
            }
            Run::Set(values) => {
                let mut done = 0;
                for record in &selected {
                    if cx.update(&record.key, values).await? {
                        done += 1;
                    }
                }
                done
            }
            Run::Custom(run) => run(cx).await?,
        };
        Ok(Response::Redirect {
            status: 303,
            location: format!(
                "{}?done={}&n={done}",
                self.list_url(model),
                crate::query::percent_encode(&action.key)
            ),
        })
    }

    /// Tells the journal about one write that succeeded.
    fn tell(
        &self,
        principal: Principal<'_>,
        entry: &Entry,
        key: &PrimaryKey,
        label: &str,
        kind: Written,
        fields: &[FieldChange],
    ) {
        let Some(journal) = &self.journal else {
            return;
        };
        let who = self.user_label.as_ref().and_then(|name| name(&principal));
        let key = key.to_segment();
        journal.record(&AdminWrite {
            who: who.as_deref(),
            model: &entry.descriptor.key,
            table: &entry.descriptor.table,
            key: &key,
            label,
            kind,
            fields,
        });
    }

    // ------------------------------------------------------------------ urls

    /// The index.
    pub fn index_url(&self) -> String {
        match self.base_path.is_empty() {
            true => "/".to_string(),
            false => self.base_path.clone(),
        }
    }

    /// One model's list screen.
    pub fn list_url(&self, model: &str) -> String {
        format!("{}/{model}", self.base_path)
    }

    /// One model's create form.
    pub fn new_url(&self, model: &str) -> String {
        format!("{}/{model}/new", self.base_path)
    }

    /// One record.
    pub fn detail_url(&self, model: &str, key: &PrimaryKey) -> String {
        format!("{}/{model}/{}", self.base_path, key.to_segment())
    }

    /// One record's edit form.
    pub fn edit_url(&self, model: &str, key: &PrimaryKey) -> String {
        format!("{}/{model}/{}/edit", self.base_path, key.to_segment())
    }

    /// One record's delete confirmation.
    pub fn delete_url(&self, model: &str, key: &PrimaryKey) -> String {
        format!("{}/{model}/{}/delete", self.base_path, key.to_segment())
    }

    /// Where one model's action form posts.
    pub fn actions_url(&self, model: &str) -> String {
        format!("{}/{model}/actions", self.base_path)
    }
}

/// Which screen a path and a method reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route<'a> {
    /// `GET {base}`.
    Index,
    /// `GET {base}/{model}`.
    List(&'a str),
    /// `GET {base}/{model}/new`.
    New(&'a str),
    /// `POST {base}/{model}`.
    Create(&'a str),
    /// `GET {base}/{model}/{key}`.
    Detail(&'a str, &'a str),
    /// `GET {base}/{model}/{key}/edit`.
    Edit(&'a str, &'a str),
    /// `POST {base}/{model}/{key}`.
    Update(&'a str, &'a str),
    /// `GET {base}/{model}/{key}/delete`.
    ConfirmDelete(&'a str, &'a str),
    /// `POST {base}/{model}/{key}/delete`.
    Delete(&'a str, &'a str),
    /// `POST {base}/{model}/actions`: an action over selected rows, first
    /// its confirmation, then, with `confirm`, the writes.
    Action(&'a str),
    /// Anything under a panel's key: `{base}/{key}` and below.
    ///
    /// One variant for the whole subtree, because the admin does not know
    /// what paths a panel serves and should not have to. The panel is given
    /// what is left and answers `None` for a path it does not serve.
    Panel {
        /// The panel's key.
        key: &'a str,
        /// What followed it, with no leading or trailing slash.
        rest: &'a str,
        /// Whether this is a POST.
        post: bool,
    },
}

impl<'a> Route<'a> {
    /// Reads a route out of a method and a path.
    ///
    /// `new` is a reserved key segment, as `add` is in Django: a record whose
    /// primary key is the literal text `new` cannot be reached. Say so rather
    /// than discover it. `actions` is reserved for a POST: a record whose key
    /// is the text `actions` can be read but not changed.
    /// `panels` is the set of keys that are tabs rather than models. It is
    /// a parameter and not a lookup because this function is pure and the
    /// test suite calls it directly; `Admin::dispatch` passes its own.
    pub fn parse(base: &str, method: &str, path: &'a str, panels: &[&str]) -> Option<Route<'a>> {
        // A path outside the mount is not this admin's, which matters when the
        // application routes by a pattern wider than the mount point.
        let rest = match base.is_empty() {
            true => path,
            false => path.strip_prefix(base)?,
        };
        if !rest.is_empty() && !rest.starts_with('/') {
            return None;
        }
        let rest = rest.trim_matches('/');
        let post = method.eq_ignore_ascii_case("post");
        let get = method.eq_ignore_ascii_case("get");
        if !get && !post {
            return None;
        }
        let parts: Vec<&str> = match rest.is_empty() {
            true => Vec::new(),
            false => rest.split('/').collect(),
        };
        // A panel owns everything under its key, so it is matched before
        // the model shapes and on any depth. `Admin::new` has already
        // refused a key that is both.
        if let Some(first) = parts.first() {
            if panels.contains(first) {
                let rest = rest[first.len()..].trim_matches('/');
                return Some(Route::Panel {
                    key: first,
                    rest,
                    post,
                });
            }
        }
        match (parts.as_slice(), post) {
            ([], false) => Some(Route::Index),
            ([model], false) => Some(Route::List(model)),
            ([model], true) => Some(Route::Create(model)),
            ([model, "new"], false) => Some(Route::New(model)),
            ([model, "actions"], true) => Some(Route::Action(model)),
            ([model, key], false) => Some(Route::Detail(model, key)),
            ([model, key], true) => Some(Route::Update(model, key)),
            ([model, key, "edit"], false) => Some(Route::Edit(model, key)),
            ([model, key, "delete"], false) => Some(Route::ConfirmDelete(model, key)),
            ([model, key, "delete"], true) => Some(Route::Delete(model, key)),
            _ => None,
        }
    }
}

impl Admin {
    /// Sets `values` on one record, under the screen policy, the field mask
    /// and the update rule, and tells the journal. The write behind
    /// [`ActionCx::update`] and [`PageCx::update`].
    async fn guarded_update(
        &self,
        principal: Principal<'_>,
        model: &str,
        key: &PrimaryKey,
        values: &[(String, Value)],
    ) -> Result<bool, AdminError> {
        let Some(entry) = self.entry(model) else {
            return Ok(false);
        };
        if !entry.admin.allows_update()
            || !self.allows(principal, Request::on(Action::Update, model, key))
        {
            return Ok(false);
        }
        let names: Vec<&str> = values.iter().map(|(name, _)| name.as_str()).collect();
        match self.rules(entry).check(&principal, Write::Update, &names) {
            Ok(()) => {}
            Err(MaskViolation::Forbidden { .. }) => return Ok(false),
            Err(unknown) => return Err(AdminError::Mask(unknown)),
        }
        let Some(pending) = self
            .rules(entry)
            .admission(&principal, RowAction::Update, key)?
        else {
            return Ok(false);
        };
        let Some(row) = self
            .data
            .fetch(&entry.descriptor, key, pending.scope())
            .await?
        else {
            return Ok(false);
        };
        let Ok(guard) = pending.resolve(1) else {
            return Ok(false);
        };
        let rows = match self
            .data
            .update(&entry.descriptor, key, values, guard.scope())
            .await
        {
            Ok(rows) => rows,
            Err(DataError::Invalid(_)) => return Ok(false),
            Err(other) => return Err(other.into()),
        };
        if guard.applied(rows).is_err() {
            return Ok(false);
        }
        let label = entry.admin.label_for(&entry.descriptor, &row);
        let fields = crate::journal::changes(Some(&row), values);
        self.tell(principal, entry, key, &label, Written::Updated, &fields);
        Ok(true)
    }

    /// Deletes one record, under the screen policy and the delete rule, and
    /// tells the journal. The write behind [`ActionCx::delete`] and
    /// [`PageCx::delete`].
    async fn guarded_delete(
        &self,
        principal: Principal<'_>,
        model: &str,
        key: &PrimaryKey,
    ) -> Result<bool, AdminError> {
        let Some(entry) = self.entry(model) else {
            return Ok(false);
        };
        if !entry.admin.allows_delete()
            || !self.allows(principal, Request::on(Action::Delete, model, key))
        {
            return Ok(false);
        }
        let Some(pending) = self
            .rules(entry)
            .admission(&principal, RowAction::Delete, key)?
        else {
            return Ok(false);
        };
        let found = self
            .data
            .fetch(&entry.descriptor, key, pending.scope())
            .await?;
        let Some(row) = found else {
            return Ok(false);
        };
        let Ok(guard) = pending.resolve(1) else {
            return Ok(false);
        };
        let removed = self
            .data
            .delete(&entry.descriptor, key, guard.scope())
            .await?;
        if guard.applied(removed).is_err() {
            return Ok(false);
        }
        let label = entry.admin.label_for(&entry.descriptor, &row);
        let fields = crate::journal::removed(&row);
        self.tell(principal, entry, key, &label, Written::Deleted, &fields);
        Ok(true)
    }
}

impl ActionCx<'_> {
    /// Sets `values` on one record, under the update rule, and tells the
    /// journal. `Ok(false)` when the rules refuse the record, the record is
    /// gone, it changed meanwhile, or the data source refused the values.
    ///
    /// # Errors
    ///
    /// The data source failed, or a row policy did not compile.
    pub async fn update(
        &self,
        key: &PrimaryKey,
        values: &[(String, Value)],
    ) -> Result<bool, AdminError> {
        self.admin
            .guarded_update(self.principal, self.model, key, values)
            .await
    }

    /// Deletes one record, under the delete rule, and tells the journal.
    /// `Ok(false)` when the rules refuse it or it is gone.
    ///
    /// # Errors
    ///
    /// The data source failed, or a row policy did not compile.
    pub async fn delete(&self, key: &PrimaryKey) -> Result<bool, AdminError> {
        self.admin
            .guarded_delete(self.principal, self.model, key)
            .await
    }
}

impl PageCx<'_> {
    /// Sets `values` on one record of this page's model, through the
    /// checks of every admin write: `Action::Update` on the record, the
    /// field mask, the update rule, and a write guarded by that rule. Tells
    /// the journal.
    ///
    /// `Ok(false)` on a GET, which writes nothing, and when the rules
    /// refuse the record, the record is gone, it changed meanwhile, or the
    /// data source refused the values.
    ///
    /// # Errors
    ///
    /// The data source failed, or a row policy did not compile.
    pub async fn update(
        &self,
        key: &PrimaryKey,
        values: &[(String, Value)],
    ) -> Result<bool, AdminError> {
        if !self.request.post {
            return Ok(false);
        }
        self.admin
            .guarded_update(self.principal, self.model, key, values)
            .await
    }

    /// Deletes one record of this page's model, through `Action::Delete`
    /// and the delete rule, and tells the journal. `Ok(false)` on a GET,
    /// and when the rules refuse the record or it is gone.
    ///
    /// # Errors
    ///
    /// The data source failed, or a row policy did not compile.
    pub async fn delete(&self, key: &PrimaryKey) -> Result<bool, AdminError> {
        if !self.request.post {
            return Ok(false);
        }
        self.admin
            .guarded_delete(self.principal, self.model, key)
            .await
    }
}

/// The screen policy's word for a row rule an action needs.
fn screen_action(needs: RowAction) -> Action {
    match needs {
        RowAction::Delete => Action::Delete,
        _ => Action::Update,
    }
}

/// The submitted values as a row, to name a record that was just created.
fn row_of(values: &[(String, Value)]) -> Row {
    let mut row = Row::new();
    for (name, value) in values {
        row.set(name.clone(), value.clone());
    }
    row
}

/// A path that reaches a page under a model (`record` is `None`) or under
/// one record.
#[derive(Clone, Copy, Debug)]
struct PageRoute<'a> {
    model: &'a str,
    record: Option<&'a str>,
    page: &'a str,
    rest: &'a str,
    post: bool,
}

/// Refuses a page key that is empty, holds a slash, or is a segment the
/// admin's own screens use at that place.
fn check_page(model: &str, page: &crate::config::ModelPage) {
    let key = page.key.as_str();
    let (method, reserved): (&str, &[&str]) = match page.record {
        false => ("page", &["new", "actions"]),
        true => ("record_page", &["edit", "delete"]),
    };
    if key.is_empty() || key.contains('/') {
        panic!(
            "the admin's page `{key}` under `{model}` is not one path segment.\nhelp: give \
             `ModelAdmin::{method}` a key with no slash, such as `report`"
        );
    }
    if reserved.contains(&key) {
        panic!(
            "the admin's page `{key}` under `{model}` takes a path the admin uses.\nhelp: \
             `ModelAdmin::{method}` cannot use {}; choose another key",
            reserved.join(" or ")
        );
    }
}

/// The child column an inline reads and the parent column it holds.
///
/// Panics with a `help:` line when the child is not shown or no single
/// column points at the parent.
fn resolve_inline(entries: &[Entry], parent: &Entry, inline: &Inline) -> (String, String) {
    let parent_key = &parent.descriptor.key;
    let Some(child) = entries.iter().find(|e| e.descriptor.key == inline.model) else {
        panic!(
            "the admin's inline `{}` on `{parent_key}` names no model the admin shows.\nhelp: \
             use the key of a discovered model that is not hidden",
            inline.model
        );
    };
    let points_here = |column: &ColumnDescriptor| {
        child
            .admin
            .relation_for(column)
            .filter(|relation| relation.target == *parent_key)
            .map(|relation| relation.target_column.clone())
    };
    match &inline.fk {
        Some(name) => {
            let Some(column) = child.descriptor.column(name) else {
                panic!(
                    "the admin's inline `{}` on `{parent_key}` names `{name}`, which is not a \
                     column of `{}`.\nhelp: name the column of `{}` that holds the `{parent_key}` \
                     key",
                    inline.model, inline.model, inline.model
                );
            };
            // A column the schema does not relate holds the parent's one
            // key column.
            let parent_column = points_here(column).or_else(|| {
                match (
                    child.admin.relation_for(column),
                    parent.descriptor.key_columns().as_slice(),
                ) {
                    (None, [only]) => Some(only.name.clone()),
                    _ => None,
                }
            });
            let Some(parent_column) = parent_column else {
                panic!(
                    "the admin's inline `{}` on `{parent_key}` reads `{name}`, which points at \
                     another model.\nhelp: name the column of `{}` that holds the `{parent_key}` \
                     key",
                    inline.model, inline.model
                );
            };
            (name.clone(), parent_column)
        }
        None => {
            let found: Vec<(String, String)> = child
                .descriptor
                .columns
                .iter()
                .filter_map(|column| {
                    points_here(column).map(|target| (column.name.clone(), target))
                })
                .collect();
            match found.as_slice() {
                [one] => one.clone(),
                [] => panic!(
                    "the admin's inline `{}` on `{parent_key}` found no column of `{}` that \
                     points at `{parent_key}`.\nhelp: name it with `Inline::fk`, or declare the \
                     relation with `ModelAdmin::foreign_key`",
                    inline.model, inline.model
                ),
                _ => panic!(
                    "the admin's inline `{}` on `{parent_key}` found more than one column of `{}` \
                     that points at `{parent_key}`.\nhelp: name the one to use with `Inline::fk`",
                    inline.model, inline.model
                ),
            }
        }
    }
}

/// A key's text as the value a column of `kind` stores.
fn typed(kind: ColumnKind, text: &str) -> Value {
    match kind {
        ColumnKind::Integer => text
            .parse::<i64>()
            .map_or_else(|_| Value::Text(text.to_string()), Value::Int),
        _ => Value::Text(text.to_string()),
    }
}

/// Whether a child form's values differ from the row it was built from.
fn changed(before: &Row, values: &[(String, Value)]) -> bool {
    values.iter().any(|(name, value)| {
        before.get(name).map(Value::as_input).unwrap_or_default() != value.as_input()
    })
}

/// A panel key that is also a model key, if there is one.
fn collision(panels: &[std::sync::Arc<dyn Panel>], entries: &[Entry]) -> Option<(String, String)> {
    panels.iter().find_map(|p| {
        entries
            .iter()
            .find(|e| e.descriptor.key == p.key())
            .map(|e| (p.key().to_string(), e.descriptor.table.clone()))
    })
}

/// Shortens a cell so one long column cannot push the table off the screen.
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}
