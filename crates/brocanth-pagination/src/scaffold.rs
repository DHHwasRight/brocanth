//! `pagination` -- what `brocanth create` writes when somebody asks for one
//! paginator.
//!
//! The crate is a library: no plugin, no route, no setting. What a project
//! gets is the way a page reads it:
//!
//! - `ctx.page_params()` and `ctx.links()` on the context, in
//!   `src/pages/mod.rs`, so a list reads `page`, `per_page`, `after` and
//!   `before` with a 400 for a bad value, and writes links that keep its
//!   filters;
//! - with `rsx`, `src/pages/parts/pager.rsx`: two components that draw an
//!   offset pager and a keyset pager;
//! - with `jt`, `src/pages/parts/pager.jt`: the same links as JSON;
//! - with neither, `pager_html` in `src/pages/mod.rs`, which writes the
//!   offset pager as escaped HTML;
//! - with `policy`, the crate's `policy` feature: a keyset page as a `Pred`.
//!
//! `outside_runtime`: it writes code that pages call, and a static export
//! renders pages too.
//!
//! Compiled only under the `scaffold` feature, which is off.

use brocanth_scaffold::{Category, Dep, Manifest, Module, Project, Scaffold, Slot};

/// One paginator, as a choice on the `brocanth create` menu.
pub struct Pagination;

impl Module for Pagination {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "pagination",
            "offset and keyset pages, with links that keep the query; no dependency",
        )
        .category(Category::Renderer)
        .outside_runtime()
        .adapts_to(
            "rsx",
            "writes src/pages/parts/pager.rsx, two pager components",
        )
        .adapts_to("jt", "writes src/pages/parts/pager.jt, the page links as JSON")
        .adapts_to(
            "policy",
            "turns on the crate's `policy` feature, and AGENTS.md says to AND a keyset cursor with the rule",
        )
        .asked_in("pages", "Pages of rows, with links that keep the query")
        .krate(
            Dep::brocanth("brocanth-pagination")
                .feature_with("policy", "policy")
                .note("pages, cursors and the links between them"),
        )
        .absent(
            "brocanth-pagination",
            "no paginator; each list counts its own offset and writes its own links",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.at(Slot::PagesItems).block(CTX_PAGES.to_string());
        if project.has("rsx") {
            s.file("src/pages/parts/pager.rsx", PAGER_RSX);
        }
        if project.has("jt") {
            s.file("src/pages/parts/pager.jt", PAGER_JT);
        }
        if !project.has("rsx") {
            s.at(Slot::PagesItems).block(PAGER_HTML.to_string());
        }
        s.at(Slot::AgentsSections).block(agents(project));
    }
}

const CTX_PAGES: &str = r#"/// The page a list was asked for, and the links it answers with.
///
/// `allow(dead_code)`: it is here for the first list, and a new project has
/// none.
#[allow(dead_code)]
impl Ctx {
    /// `page`, `per_page`, `after` and `before` from the query string. A
    /// value that is not a number, or a cursor that does not parse, is a 400
    /// with a `help:` line, not page one.
    pub fn page_params(&self) -> Result<brocanth_pagination::Params, Error> {
        brocanth_pagination::Params::parse(&self.query).map_err(|e| Error::new(400, e.to_string()))
    }

    /// The links of the list at this path. Each keeps every key of the query
    /// string but `page`, `after` and `before`, so a filter set on page one
    /// is still set on page three.
    pub fn links(&self) -> brocanth_pagination::Links {
        brocanth_pagination::Links::new(&self.path, &self.query)
    }
}
"#;

const PAGER_HTML: &str = r#"/// An offset pager as HTML: previous, the numbered pages, next. Every href
/// is escaped. Empty when there is one page.
///
/// ```ignore
/// let pager = brocanth_pagination::Pager::new(page, &ctx.links());
/// body.push_str(&pager_html(&pager));
/// ```
#[allow(dead_code)]
pub fn pager_html(pager: &brocanth_pagination::Pager) -> String {
    fn attr(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('"', "&quot;")
            .replace('<', "&lt;")
    }
    if !pager.is_needed() {
        return String::new();
    }
    let mut html = String::from("<nav aria-label=\"Pages\">");
    if let Some(href) = &pager.previous {
        html.push_str(&format!(
            "<a href=\"{}\" rel=\"prev\">Previous</a> ",
            attr(href)
        ));
    }
    for link in &pager.pages {
        if link.current {
            html.push_str(&format!(
                "<span aria-current=\"page\">{}</span> ",
                link.number
            ));
        } else {
            html.push_str(&format!(
                "<a href=\"{}\">{}</a> ",
                attr(&link.href),
                link.number
            ));
        }
    }
    if let Some(href) = &pager.next {
        html.push_str(&format!("<a href=\"{}\" rel=\"next\">Next</a>", attr(href)));
    }
    html.push_str("</nav>");
    html
}
"#;

const PAGER_RSX: &str = r#"// Two pagers, for any list. Not a page: the route table reads `.rs` files
// only, so this file adds no route.
//
//     let pager = brocanth_pagination::Pager::new(page, &ctx.links());
//     <PageNav pager={&pager} />
//
// A link is an ordinary `<a href>`, so the pager works with no script.

/// An offset pager: the summary, previous, the numbered pages, next. Nothing
/// when there is one page.
pub fn PageNav(pager: &brocanth_pagination::Pager) -> Element {
    if pager.is_needed() {
        <nav aria-label="Pages">
            <span>{pager.summary()}</span>
            if let Some(href) = &pager.previous {
                <a href={href} rel="prev">Previous</a>
            }
            for link in &pager.pages {
                if link.current {
                    <span aria-current="page">{link.number}</span>
                } else {
                    <a href={&link.href}>{link.number}</a>
                }
            }
            if let Some(href) = &pager.next {
                <a href={href} rel="next">Next</a>
            }
        </nav>
    }
}

/// A keyset pager: the first page, the previous one and the next one. A
/// keyset page has no numbers.
pub fn KeysetNav(pager: &brocanth_pagination::KeysetPager) -> Element {
    <nav aria-label="Pages">
        if let Some(href) = &pager.first {
            <a href={href} rel="first">First</a>
        }
        if let Some(href) = &pager.previous {
            <a href={href} rel="prev">Previous</a>
        }
        if let Some(href) = &pager.next {
            <a href={href} rel="next">Next</a>
        }
    </nav>
}
"#;

const PAGER_JT: &str = r#"// The links of a page, as JSON. Not a page: the route table reads `.rs`
// files only, so this file adds no route.
//
//     "items": [ for p in &posts { Post(p) } ],
//     "pages": PageLinks(&pager),

/// An offset page: where it is, and the links on either side.
pub fn PageLinks(pager: &brocanth_pagination::Pager) -> Json {
    "page": pager.page.number(),
    "per_page": pager.page.per_page(),
    "total": pager.page.total(),
    "total_pages": pager.page.total_pages(),
    "previous"?: pager.previous.as_deref(),
    "next"?: pager.next.as_deref(),
}

/// A keyset page: the first page, the previous one and the next one.
pub fn KeysetLinks(pager: &brocanth_pagination::KeysetPager) -> Json {
    "first"?: pager.first.as_deref(),
    "previous"?: pager.previous.as_deref(),
    "next"?: pager.next.as_deref(),
}
"#;

fn agents(project: &Project) -> String {
    let draw = if project.has("rsx") {
        "Draw it with `<PageNav pager={&pager} />` from\n\
         `src/pages/parts/pager.rsx`"
    } else {
        "Draw it with `pager_html(&pager)` from `src/pages/mod.rs`"
    };
    let keyset = if project.has("policy") {
        "- Build the keyset `WHERE` with `brocanth_pagination::Keyset`:\n\
         \x20 `rule.and(keyset.after(params.after.as_ref()).map_err(|e|\n\
         \x20 Error::new(e.status(), e.to_string()))?)`, compiled once, so a\n\
         \x20 forged cursor only moves the window inside the rule. The column\n\
         \x20 enum implements `brocanth_policy::Sortable`, so a cursor key of the\n\
         \x20 wrong type is a 400 and not a database error. Keep the `map_err`:\n\
         \x20 a bare `?` makes it a 500.\n\
         - A `before=<cursor>` page (the previous link) reads with\n\
         \x20 `keyset.before(&cursor)` (the same `map_err`) and\n\
         \x20 `keyset.reversed().order_by()`, then\n\
         \x20 `KeysetPage::before` puts the rows back in order.\n"
    } else {
        ""
    };
    format!(
        "\n## Pagination\n\
         \n\
         A list reads its page with `ctx.page_params()?` and builds a\n\
         `brocanth_pagination::Page` from the count. {draw}. The links come\n\
         from `ctx.links()`, which keeps the filters and the sort.\n\
         \n\
         - Put the rule, the filters and the search in the `WHERE` of the\n\
         \x20 count and of the page. Never drop rows after the page is read: page\n\
         \x20 two then overlaps page one, and the total is wrong.\n\
         - Order by a unique key last. Without it, two pages can share a row.\n\
         - A long list or a feed uses a keyset page (`after=<cursor>`), not an\n\
         \x20 offset: it reads one index seek, not every row before it.\n{keyset}"
    )
}
