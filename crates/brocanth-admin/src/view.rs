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

/// The screens and the two blocks of the layout, each replaceable.
///
/// [`Views::default`] is the shipped set. Replace one and the rest stay as
/// they are. A model can replace its own four screens on top of these, with
/// [`ModelAdmin::list_view`](crate::ModelAdmin::list_view) and its siblings:
/// the model's view wins, then this set.
///
/// An override that only adds to a shipped screen keeps the shipped one and
/// calls it. That is Django's `{{ block.super }}`:
///
/// ```
/// use brocanth_admin::view::ListCtx;
/// use brocanth_admin::{AdminConfig, Views};
///
/// let shipped = Views::default().list;
/// let config = AdminConfig::new().with_views(Views::default().with_list(
///     move |ctx: &ListCtx, w: &mut brocanth_admin::rsx::Writer<'_>| {
///         w.raw("<p role=\"note\">Edits here reach customers.</p>")?;
///         shipped.render(ctx, w)
///     },
/// ));
/// # let _ = config;
/// ```
///
/// See `docs/admin-customization.md` for every level, from a title to a
/// whole screen.
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
    /// The confirmation before an action over selected rows. See
    /// [`crate::actions`].
    pub confirm_action: Box<dyn View<ActionCtx>>,
    /// Anything that went wrong: 403, 404.
    pub error: Box<dyn View<ErrorCtx>>,
    /// The top of the sidebar: the header, linked to the index. Django's
    /// `branding` block. The layout receives it rendered, in
    /// [`LayoutCtx::brand`].
    pub brand: Box<dyn View<Chrome>>,
    /// The links about the person and the site: who is signed in, and
    /// "View site". Django's `userlinks` block. The layout receives it
    /// rendered, in [`LayoutCtx::userlinks`].
    pub userlinks: Box<dyn View<Chrome>>,
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
            confirm_action: Box::new(|ctx: &ActionCtx, w: &mut Writer<'_>| {
                templates::action::ConfirmAction { ctx }.render(w)
            }),
            error: Box::new(|ctx: &ErrorCtx, w: &mut Writer<'_>| {
                templates::error::ErrorPage { ctx }.render(w)
            }),
            brand: Box::new(|ctx: &Chrome, w: &mut Writer<'_>| {
                templates::brand::Brand { ctx }.render(w)
            }),
            userlinks: Box::new(|ctx: &Chrome, w: &mut Writer<'_>| {
                templates::userlinks::Userlinks { ctx }.render(w)
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

    /// Replaces the confirmation before an action over selected rows.
    pub fn with_confirm_action(mut self, view: impl View<ActionCtx>) -> Self {
        self.confirm_action = Box::new(view);
        self
    }

    /// Replaces the error page.
    pub fn with_error(mut self, view: impl View<ErrorCtx>) -> Self {
        self.error = Box::new(view);
        self
    }

    /// Replaces the brand block at the top of the sidebar, and keeps the
    /// layout. Django's `{% block branding %}`.
    ///
    /// ```
    /// use brocanth_admin::view::Chrome;
    /// use brocanth_admin::{AdminConfig, Views};
    ///
    /// let config = AdminConfig::new().map_views(|views| {
    ///     views.with_brand(|ctx: &Chrome, w: &mut brocanth_admin::rsx::Writer<'_>| {
    ///         w.raw("<img src=\"/logo.svg\" alt=\"\"> ")?;
    ///         w.text(&ctx.header)
    ///     })
    /// });
    /// # let _ = config;
    /// ```
    pub fn with_brand(mut self, view: impl View<Chrome>) -> Self {
        self.brand = Box::new(view);
        self
    }

    /// Replaces the user links block under the sidebar, and keeps the
    /// layout. Django's `{% block userlinks %}`: the place for "Log out"
    /// and "Change password", which the admin cannot draw because it has
    /// no authentication scheme of its own.
    pub fn with_userlinks(mut self, view: impl View<Chrome>) -> Self {
        self.userlinks = Box::new(view);
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

/// What every screen shares: the titles, the mount path, the sidebar and the
/// stylesheets.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Chrome {
    /// The admin's title, at the end of every `<title>`. Django's
    /// `site_title`.
    pub title: String,
    /// What the top of the sidebar says. Django's `site_header`. The title
    /// when nothing else was set.
    pub header: String,
    /// The application's front page, for a "View site" link. Django's
    /// `site_url`. `None` draws no link.
    pub site_url: Option<String>,
    /// Where the admin is mounted.
    pub base_path: String,
    /// The sidebar.
    pub nav: Vec<NavItem>,
    /// Stylesheets linked after the shipped one, in order: the admin's own,
    /// then the shown model's. Each is an `href` the application serves.
    pub stylesheets: Vec<String>,
    /// Scripts loaded with `defer` after the admin's own, in order: the
    /// admin's, then the shown model's. Each is a `src` the application
    /// serves. Django's `Media.js`.
    pub scripts: Vec<String>,
    /// Who is signed in, as [`AdminConfig::user_label`] names them. `None`
    /// when no label was configured or nobody is signed in.
    ///
    /// [`AdminConfig::user_label`]: crate::AdminConfig::user_label
    pub user: Option<String>,
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
    /// The brand block, rendered by [`Views::brand`]. Unescaped, as `body`.
    pub brand: String,
    /// The user links block, rendered by [`Views::userlinks`]. Unescaped,
    /// as `body`.
    pub userlinks: String,
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
    /// The index's `<h1>`. Django's `index_title`. The title when nothing
    /// else was set.
    pub heading: String,
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

/// One page link: `brocanth-pagination`'s, so the admin and every other
/// list draw the same thing.
pub use brocanth_pagination::PageLink;

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
    /// The actions over selected rows this person may run. Empty draws no
    /// checkboxes and no menu.
    pub actions: Vec<ActionChoice>,
    /// Where the action form posts to.
    pub actions_href: String,
    /// One line about the action that just ran: "Publish selected: 3
    /// posts."
    pub notice: Option<String>,
}

/// One entry in the list screen's action menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionChoice {
    /// The key the form posts.
    pub key: String,
    /// What the menu says.
    pub label: String,
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
    /// More links about this record, such as its history. See
    /// [`AdminConfig::record_link`](crate::AdminConfig::record_link).
    pub links: Vec<RecordLink>,
}

/// One extra link on the detail screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordLink {
    /// What it says.
    pub label: String,
    /// Where it goes.
    pub href: String,
}

/// One record on the action confirmation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectedRecord {
    /// The record's key, in its URL form: the value the form posts back.
    pub key: String,
    /// What the record is called.
    pub label: String,
    /// The detail screen.
    pub href: String,
}

/// The action confirmation's context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionCtx {
    /// The chrome.
    pub chrome: Chrome,
    /// The model.
    pub model: ModelHeading,
    /// The action's key, which the form posts back.
    pub key: String,
    /// What the action is called.
    pub label: String,
    /// Whether it deletes.
    pub destructive: bool,
    /// The records it will act on.
    pub records: Vec<SelectedRecord>,
    /// How many selected records the rules left out.
    pub skipped: usize,
    /// Where the confirmation posts to.
    pub action: String,
    /// Where cancelling goes.
    pub cancel_href: String,
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
    /// The child rows on this form, one set per [`Inline`](crate::Inline)
    /// this person may see. Rendered inside the same `<form>`.
    pub inlines: Vec<InlineSet>,
}

/// One inline on a form: the children of one model.
#[derive(Clone, Debug, PartialEq)]
pub struct InlineSet {
    /// The child model.
    pub model: ModelHeading,
    /// The labels of the fields each row shows, in order.
    pub columns: Vec<String>,
    /// The rows: the existing children first, then the blank rows for new
    /// ones.
    pub rows: Vec<InlineRow>,
    /// The parent has more children than the form shows. They are not on
    /// the form, so the POST does not change them.
    pub more: bool,
}

/// One child row on a form.
#[derive(Clone, Debug, PartialEq)]
pub struct InlineRow {
    /// The start of every name this row posts: `post-0`.
    pub prefix: String,
    /// The child's key, in its URL form. `None` on a row for a new child.
    pub key: Option<String>,
    /// What the child is called. Empty on a row for a new child.
    pub label: String,
    /// The child's own detail screen. `None` on a row for a new child.
    pub href: Option<String>,
    /// The fields, with the posted values and the errors in them. A field
    /// this person may not change is [`Widget::ReadOnly`](crate::Widget).
    pub form: Form,
    /// Whether this person may delete this child: the row draws a delete
    /// box.
    pub can_delete: bool,
    /// Whether the delete box was ticked, on a re-rendered form.
    pub delete: bool,
}

impl InlineRow {
    /// The name a field of this row posts: `post-0-title`.
    pub fn name(&self, column: &str) -> String {
        format!("{}-{column}", self.prefix)
    }

    /// Whether the row posts its key: an existing child that this person
    /// may change or delete. Without it the POST leaves the child alone.
    pub fn posts_key(&self) -> bool {
        self.key.is_some()
            && (self.can_delete || self.form.fields.iter().any(|f| f.widget.is_editable()))
    }
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
