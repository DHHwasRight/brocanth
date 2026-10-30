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

use crate::config::{AdminConfig, ModelAdmin};
use crate::data::{DataError, ListRequest, Search, SharedData};
use crate::discovery::discover;
use crate::form::{Choice, Form, Submission};
use crate::model::{ColumnDescriptor, ModelDescriptor, PrimaryKey, Row, Value};
use crate::paging::Page;
use crate::panel::{Panel, PanelPage, PanelRequest};
use crate::policy::{Action, Principal, Request};
use crate::query::{Direction, ListQuery};
use crate::rows::{Gate, RowAction, RowRules, Rules};
use crate::view::{
    self, Cell, Chrome, ColumnHeader, DeleteCtx, DetailCtx, DetailRow, ErrorCtx, FormCtx, IndexCtx,
    IndexEntry, LayoutCtx, ListCtx, ModelHeading, NavItem, PageLink, RowView, Views,
};

/// How many numbered page links a pager shows.
const PAGER_WINDOW: usize = 7;

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
    base_path: String,
    policy: Box<dyn crate::policy::Policy>,
    views: Views,
    entries: Vec<Entry>,
    panels: Vec<std::sync::Arc<dyn Panel>>,
    unscoped_rows: bool,
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
            title: config.title,
            base_path: config.base_path,
            policy: config.policy,
            views: config.views,
            entries,
            panels,
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
            models,
        };
        let body = view::render(self.views.index.as_ref(), &context)?;
        self.page(200, principal, "", self.title.clone(), body)
    }

    /// One model's records.
    pub async fn list(
        &self,
        principal: Principal<'_>,
        model: &str,
        query: ListQuery,
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
        let page = Page::new(
            query.page,
            query.per_page.unwrap_or_else(|| admin.page_size()),
            total,
        );
        let rows = self
            .data
            .list(ListRequest {
                model: descriptor,
                offset: page.offset(),
                limit: page.limit(),
                search,
                sort: query.sort.as_ref(),
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

        let pages = page
            .window(PAGER_WINDOW)
            .into_iter()
            .map(|number| PageLink {
                href: format!("{}?{}", self.list_url(model), query.to_query_string(number)),
                current: number == page.number(),
                number,
            })
            .collect();
        let link_for =
            |number: usize| format!("{}?{}", self.list_url(model), query.to_query_string(number));
        let context = ListCtx {
            chrome: self.chrome(principal, model),
            model: self.heading(entry),
            columns: headers,
            rows: row_views,
            previous_href: page.previous().map(link_for),
            next_href: page.next().map(link_for),
            pages,
            page,
            searchable: !search_columns.is_empty(),
            can_create: admin.allows_create()
                && self.allows(principal, Request::new(Action::Create, model))
                && self.creates(principal, entry),
            create_href: self.new_url(model),
            query,
        };
        let heading = admin.plural(descriptor);
        let body = view::render(self.views.list.as_ref(), &context)?;
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
            label: label.clone(),
        };
        let body = view::render(self.views.detail.as_ref(), &context)?;
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
        self.lock(principal, entry, &mut form);
        self.render_form(principal, entry, form, None).await
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
        if let Some(refused) = self.refuse_fields(principal, entry, submission)? {
            return Ok(refused);
        }
        let choices = self
            .choices(principal, &entry.admin, &entry.descriptor)
            .await?;
        let mut form = Form::build(&entry.descriptor, &entry.admin, None, &choices);
        self.lock(principal, entry, &mut form);
        form.accept(submission);
        if !form.validate() {
            return self.render_rejected(principal, entry, form, None).await;
        }
        match self.data.insert(&entry.descriptor, &form.values()).await {
            Ok(key) => Ok(Response::Redirect {
                status: 303,
                location: self.detail_url(model, &key),
            }),
            Err(DataError::Invalid(errors)) => {
                form.reject(&errors);
                self.render_rejected(principal, entry, form, None).await
            }
            Err(other) => Err(other.into()),
        }
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
        self.lock(principal, entry, &mut form);
        self.render_form(principal, entry, form, Some(&record))
            .await
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
        if let Some(refused) = self.refuse_fields(principal, entry, submission)? {
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
        self.lock(principal, entry, &mut form);
        form.accept(submission);
        if !form.validate() {
            return self
                .render_rejected(principal, entry, form, Some(&record))
                .await;
        }
        let written = self
            .data
            .update(&entry.descriptor, &record, &form.values(), guard.scope())
            .await;
        match written.map(|rows| guard.applied(rows)) {
            Ok(Ok(_)) => Ok(Response::Redirect {
                status: 303,
                location: self.detail_url(model, &record),
            }),
            Ok(Err(_lost)) => self.conflict(principal),
            Err(DataError::Invalid(errors)) => {
                form.reject(&errors);
                self.render_rejected(principal, entry, form, Some(&record))
                    .await
            }
            Err(other) => Err(other.into()),
        }
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
        let body = view::render(self.views.confirm_delete.as_ref(), &context)?;
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
            Route::List(model) => self.list(principal, model, ListQuery::parse(query)).await,
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

    /// Draws every field the row policy's mask refuses as read-only.
    fn lock(&self, principal: Principal<'_>, entry: &Entry, form: &mut Form) {
        let rules = self.rules(entry);
        form.lock(|column| rules.writable(&principal, column));
    }

    /// `Policy::writable`, checked against every column a request sent.
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
        match self.rules(entry).check(&principal, &sent) {
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
        Chrome {
            title: self.title.clone(),
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
        let context = LayoutCtx {
            chrome: self.chrome(principal, current),
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
        let layout = LayoutCtx {
            chrome,
            heading: heading.to_string(),
            body,
        };
        Ok(Response::Page {
            status,
            html: view::render(self.views.layout.as_ref(), &layout)?,
        })
    }

    async fn render_form(
        &self,
        principal: Principal<'_>,
        entry: &Entry,
        form: Form,
        record: Option<&PrimaryKey>,
    ) -> Result<Response, AdminError> {
        self.form_page(principal, entry, form, record, 200)
    }

    async fn render_rejected(
        &self,
        principal: Principal<'_>,
        entry: &Entry,
        form: Form,
        record: Option<&PrimaryKey>,
    ) -> Result<Response, AdminError> {
        // 422, and the same form value, so nothing the person typed is lost.
        self.form_page(principal, entry, form, record, 422)
    }

    fn form_page(
        &self,
        principal: Principal<'_>,
        entry: &Entry,
        form: Form,
        record: Option<&PrimaryKey>,
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
        };
        let heading = match is_new {
            true => format!("Add {}", name.to_lowercase()),
            false => format!("Edit {}", name.to_lowercase()),
        };
        let body = view::render(self.views.form.as_ref(), &context)?;
        self.page(status, principal, model, heading, body)
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
    /// than discover it.
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
            ([model, key], false) => Some(Route::Detail(model, key)),
            ([model, key], true) => Some(Route::Update(model, key)),
            ([model, key, "edit"], false) => Some(Route::Edit(model, key)),
            ([model, key, "delete"], false) => Some(Route::ConfirmDelete(model, key)),
            ([model, key, "delete"], true) => Some(Route::Delete(model, key)),
            _ => None,
        }
    }
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
