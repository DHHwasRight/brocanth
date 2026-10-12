//! The override seam, and the context types the screens are rendered from.
//!
//! **This is the point of the crate.** Every screen is a [`View`], the admin
//! holds one per screen in [`Views`], and the shipped rsx templates are
//! nothing more than the default value of that struct. The extension path is
//! therefore the path the defaults themselves take, which is the only way to
//! know it works.
//!
//! A view is a function from an owned context to markup:
//!
//! ```ignore
//! // src/views/admin.rsx
//! use brocanth_admin::view::ListCtx;
//!
//! pub fn PostList(ctx: &ListCtx) -> Element {
//!     <section class="posts">
//!         <h1>{&ctx.model.plural}</h1>
//!         for row in &ctx.rows {
//!             <a href={&row.href}>{&row.label}</a>
//!         }
//!     </section>
//! }
//! ```
//!
//! ```ignore
//! use brocanth_admin::{AdminConfig, Views};
//! use rsx::Template;
//!
//! mod views { rsx::load_dir!("src/views"); }
//!
//! let config = AdminConfig::new().with_views(
//!     Views::default().with_list(|ctx: &ListCtx, w: &mut rsx::Writer<'_>| {
//!         views::admin::PostList { ctx }.render(w)
//!     }),
//! );
//! ```
//!
//! The contexts own their data rather than borrowing it. That is deliberate:
//! a borrowed context would put a lifetime in `Box<dyn View<ListCtx<'_>>>`,
//! which needs a higher-ranked trait object and makes the one type an
//! application has to write unreadable. An admin renders a page per request;
//! the allocations are not the budget.

use rsx::{Result as RsxResult, Writer};

use crate::form::Form;
use crate::paging::Page;
use crate::query::ListQuery;
use crate::templates;

/// One screen's renderer.
///
/// Implemented for any `Fn(&C, &mut Writer) -> rsx::Result`, so an override is
/// a closure around an rsx component and nothing else.
pub trait View<C: ?Sized>: Send + Sync + 'static {
    /// Writes the screen into `w`.
    fn render(&self, context: &C, w: &mut Writer<'_>) -> RsxResult;
}

impl<C: ?Sized, F> View<C> for F
where
    F: Fn(&C, &mut Writer<'_>) -> RsxResult + Send + Sync + 'static,
{
    fn render(&self, context: &C, w: &mut Writer<'_>) -> RsxResult {
        self(context, w)
    }
}

/// Renders a view to a string, which is what the screens do with it.
pub fn render<C: ?Sized + 'static>(view: &dyn View<C>, context: &C) -> RsxResult<String> {
    let mut w = Writer::new();
    view.render(context, &mut w)?;
    w.into_string()
}

/// The six screens, each replaceable.
///
/// [`Views::default`] is the shipped set. Replace one and the rest stay as
/// they are.
pub struct Views {
    /// Wraps every screen. Receives the inner HTML already rendered.
    pub layout: Box<dyn View<LayoutCtx>>,
    /// The list of models.
    pub index: Box<dyn View<IndexCtx>>,
    /// One model's records: pagination, search and sortable headers.
    pub list: Box<dyn View<ListCtx>>,
    /// One record.
    pub detail: Box<dyn View<DetailCtx>>,
    /// The create and edit form.
    pub form: Box<dyn View<FormCtx>>,
    /// The delete confirmation.
    pub confirm_delete: Box<dyn View<DeleteCtx>>,
    /// Anything that went wrong: 403, 404.
    pub error: Box<dyn View<ErrorCtx>>,
}

impl std::fmt::Debug for Views {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Views { .. }")
    }
}

impl Default for Views {
    fn default() -> Self {
        use rsx::Template as _;
        Views {
            layout: Box::new(|ctx: &LayoutCtx, w: &mut Writer<'_>| {
                templates::layout::Layout { ctx }.render(w)
            }),
            index: Box::new(|ctx: &IndexCtx, w: &mut Writer<'_>| {
                templates::index::Index { ctx }.render(w)
            }),
            list: Box::new(|ctx: &ListCtx, w: &mut Writer<'_>| {
                templates::list::List { ctx }.render(w)
            }),
            detail: Box::new(|ctx: &DetailCtx, w: &mut Writer<'_>| {
                templates::detail::Detail { ctx }.render(w)
            }),
            form: Box::new(|ctx: &FormCtx, w: &mut Writer<'_>| {
                templates::form::FormPage { ctx }.render(w)
            }),
            confirm_delete: Box::new(|ctx: &DeleteCtx, w: &mut Writer<'_>| {
                templates::delete::Delete { ctx }.render(w)
            }),
            error: Box::new(|ctx: &ErrorCtx, w: &mut Writer<'_>| {
                templates::error::ErrorPage { ctx }.render(w)
            }),
        }
    }
}

impl Views {
    /// Replaces the layout.
    pub fn with_layout(mut self, view: impl View<LayoutCtx>) -> Self {
        self.layout = Box::new(view);
        self
    }

    /// Replaces the index.
    pub fn with_index(mut self, view: impl View<IndexCtx>) -> Self {
        self.index = Box::new(view);
        self
    }

    /// Replaces the list screen.
    pub fn with_list(mut self, view: impl View<ListCtx>) -> Self {
        self.list = Box::new(view);
        self
    }

    /// Replaces the detail screen.
    pub fn with_detail(mut self, view: impl View<DetailCtx>) -> Self {
        self.detail = Box::new(view);
        self
    }

    /// Replaces the form screen, which serves both create and edit.
    pub fn with_form(mut self, view: impl View<FormCtx>) -> Self {
        self.form = Box::new(view);
        self
    }

    /// Replaces the delete confirmation.
    pub fn with_confirm_delete(mut self, view: impl View<DeleteCtx>) -> Self {
        self.confirm_delete = Box::new(view);
        self
    }

    /// Replaces the error page.
    pub fn with_error(mut self, view: impl View<ErrorCtx>) -> Self {
        self.error = Box::new(view);
        self
    }
}

/// One entry in the sidebar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavItem {
    /// The model key.
    pub key: String,
    /// What the link says.
    pub label: String,
    /// Where it goes.
    pub href: String,
    /// Whether this is the screen being shown.
    pub current: bool,
}

/// What every screen shares: the title, the mount path and the sidebar.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Chrome {
    /// The admin's title.
    pub title: String,
    /// Where the admin is mounted.
    pub base_path: String,
    /// The sidebar.
    pub nav: Vec<NavItem>,
}

/// The layout's context: the chrome, and the screen already rendered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LayoutCtx {
    /// The chrome.
    pub chrome: Chrome,
    /// The `<title>` and the heading of this particular screen.
    pub heading: String,
    /// The screen's markup. Written out unescaped, because the view that
    /// produced it already escaped what needed escaping.
    pub body: String,
}

/// One model on the index screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexEntry {
    /// The model key.
    pub key: String,
    /// The plural name.
    pub label: String,
    /// The list screen.
    pub href: String,
    /// The Rust module the model was discovered in.
    pub module_path: String,
    /// How many columns it has.
    pub columns: usize,
    /// Whether this person may create one.
    pub can_create: bool,
    /// Where creating starts.
    pub create_href: String,
}

/// The index screen's context.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IndexCtx {
    /// The chrome.
    pub chrome: Chrome,
    /// The models.
    pub models: Vec<IndexEntry>,
}

/// The model being shown, for a heading and a breadcrumb.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelHeading {
    /// The model key.
    pub key: String,
    /// The singular name.
    pub name: String,
    /// The plural name.
    pub plural: String,
    /// The list screen.
    pub href: String,
}

/// One column header on the list screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnHeader {
    /// The column name.
    pub name: String,
    /// What the header says.
    pub label: String,
    /// Whether the header is a sort link.
    pub sortable: bool,
    /// Where the sort link goes, or `""`.
    pub href: String,
    /// `"asc"`, `"desc"`, or `""` when this column is not the current sort.
    pub sorted: &'static str,
}

/// One cell on the list screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    /// The column this cell belongs to.
    pub column: String,
    /// What it says. **For a foreign key this is the target record's label,
    /// not its id.**
    pub text: String,
    /// Where it links, for a foreign key.
    pub href: Option<String>,
}

/// One row on the list screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowView {
    /// The record's key, in its URL form.
    pub key: String,
    /// What the record is called.
    pub label: String,
    /// The detail screen.
    pub href: String,
    /// The edit form, when this person may change it.
    pub edit_href: Option<String>,
    /// The delete confirmation, when this person may remove it.
    pub delete_href: Option<String>,
    /// The cells, in header order.
    pub cells: Vec<Cell>,
}

/// One page link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageLink {
    /// The page number.
    pub number: usize,
    /// Where it goes.
    pub href: String,
    /// Whether it is the page being shown.
    pub current: bool,
}

/// The list screen's context.
#[derive(Clone, Debug, PartialEq)]
pub struct ListCtx {
    /// The chrome.
    pub chrome: Chrome,
    /// The model.
    pub model: ModelHeading,
    /// The headers, in display order.
    pub columns: Vec<ColumnHeader>,
    /// The rows.
    pub rows: Vec<RowView>,
    /// Where the page sits in the whole.
    pub page: Page,
    /// What was asked for.
    pub query: ListQuery,
    /// The numbered page links.
    pub pages: Vec<PageLink>,
    /// The previous page, when there is one.
    pub previous_href: Option<String>,
    /// The next page, when there is one.
    pub next_href: Option<String>,
    /// Whether a search box is shown at all.
    pub searchable: bool,
    /// Whether this person may create a record.
    pub can_create: bool,
    /// Where creating starts.
    pub create_href: String,
}

/// One row on the detail screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetailRow {
    /// The column name.
    pub name: String,
    /// What the label says.
    pub label: String,
    /// What the value says, labelled rather than numbered for a foreign key.
    pub text: String,
    /// Where it links, for a foreign key.
    pub href: Option<String>,
}

/// The detail screen's context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetailCtx {
    /// The chrome.
    pub chrome: Chrome,
    /// The model.
    pub model: ModelHeading,
    /// The record's key, in its URL form.
    pub key: String,
    /// What the record is called.
    pub label: String,
    /// The values.
    pub rows: Vec<DetailRow>,
    /// The edit form, when this person may change it.
    pub edit_href: Option<String>,
    /// The delete confirmation, when this person may remove it.
    pub delete_href: Option<String>,
}

/// The form screen's context, for both create and edit.
#[derive(Clone, Debug, PartialEq)]
pub struct FormCtx {
    /// The chrome.
    pub chrome: Chrome,
    /// The model.
    pub model: ModelHeading,
    /// The form, with the submitted values and the errors already in it.
    pub form: Form,
    /// Where the form posts to.
    pub action: String,
    /// Where cancelling goes.
    pub cancel_href: String,
    /// Whether this is a create.
    pub is_new: bool,
    /// What the submit button says.
    pub submit_label: String,
}

/// The delete confirmation's context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeleteCtx {
    /// The chrome.
    pub chrome: Chrome,
    /// The model.
    pub model: ModelHeading,
    /// The record's key, in its URL form.
    pub key: String,
    /// What the record is called.
    pub label: String,
    /// Where the confirmation posts to.
    pub action: String,
    /// Where cancelling goes.
    pub cancel_href: String,
}

/// The error screen's context.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ErrorCtx {
    /// The chrome.
    pub chrome: Chrome,
    /// The HTTP status.
    pub status: u16,
    /// The heading.
    pub heading: String,
    /// One line about what happened, safe to show a stranger.
    pub message: String,
}
