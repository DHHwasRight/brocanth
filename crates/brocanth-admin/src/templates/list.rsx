use crate::paging::Page;
use crate::view::{ColumnHeader, ListCtx, RowView};

/// One model's records: search, sortable headers, rows and pagination.
pub fn List(ctx: &ListCtx) -> Element {
    <p class="crumbs"><a href={&ctx.chrome.base_path}>{&ctx.chrome.title}</a> <span> / </span> {&ctx.model.plural}</p>
    <div class="bar">
        <h1>{&ctx.model.plural}</h1>
        <span class="bar-fill"></span>
        if ctx.searchable {
            <form class="search" method="get" action={&ctx.model.href}>
                <input type="search" name="q" value={&ctx.query.search} placeholder={search_placeholder(ctx)} aria-label="Search" />
                if let Some(sort) = &ctx.query.sort {
                    <input type="hidden" name="sort" value={&sort.column} />
                    <input type="hidden" name="dir" value={sort.direction.as_str()} />
                }
                <button class="btn" type="submit">Search</button>
            </form>
        }
        if ctx.can_create {
            <a class="btn btn-primary" href={&ctx.create_href}>{add_label(ctx)}</a>
        }
    </div>

    if ctx.rows.is_empty() {
        <div class="card"><div class="empty">{empty_message(ctx)}</div></div>
    } else {
        <div class="card">
            <table>
                <thead>
                    <tr>
                        for header in &ctx.columns {
                            <Header header={header} />
                        }
                        <th aria-label="Actions"></th>
                    </tr>
                </thead>
                <tbody>
                    for row in &ctx.rows {
                        <Record row={row} />
                    }
                </tbody>
            </table>
        </div>
    }

    <div class="pager">
        <span class="count">{page_summary(&ctx.page)}</span>
        if let Some(href) = &ctx.previous_href {
            <a href={href} rel="prev">Previous</a>
        }
        for link in &ctx.pages {
            <a href={&link.href} class={page_class(link.current)}>{link.number}</a>
        }
        if let Some(href) = &ctx.next_href {
            <a href={href} rel="next">Next</a>
        }
    </div>
}

/// One column header, a sort link when the column may be sorted on.
pub fn Header(header: &ColumnHeader) -> Element {
    <th class={sort_class(header.sorted)}>
        if header.sortable {
            <a href={&header.href}>{&header.label}</a>
        } else {
            <span>{&header.label}</span>
        }
    </th>
}

/// One row. A foreign-key cell carries the target's label and links to it.
pub fn Record(row: &RowView) -> Element {
    <tr>
        for cell in &row.cells {
            <td>
                if let Some(href) = &cell.href {
                    <a href={href}>{&cell.text}</a>
                } else {
                    <span>{&cell.text}</span>
                }
            </td>
        }
        <td>
            <a href={&row.href}>View</a>
            if let Some(href) = &row.edit_href {
                <span class="muted"> · </span>
                <a href={href}>Edit</a>
            }
            if let Some(href) = &row.delete_href {
                <span class="muted"> · </span>
                <a href={href}>Delete</a>
            }
        </td>
    </tr>
}

// Branching that produces a value lives in plain functions: inside a template
// body `if` opens a markup conditional, so `let x = if …` never binds.

fn sort_class(sorted: &str) -> &'static str {
    match sorted {
        "asc" => "sorted-asc",
        "desc" => "sorted-desc",
        _ => "",
    }
}

fn page_class(current: bool) -> &'static str {
    if current {
        "is-current"
    } else {
        ""
    }
}

fn search_placeholder(ctx: &ListCtx) -> String {
    format!("Search {}", ctx.model.plural.to_lowercase())
}

fn add_label(ctx: &ListCtx) -> String {
    format!("Add {}", ctx.model.name.to_lowercase())
}

fn empty_message(ctx: &ListCtx) -> String {
    if ctx.query.search.is_empty() {
        format!("No {} yet.", ctx.model.plural.to_lowercase())
    } else {
        format!("No {} match “{}”.", ctx.model.plural.to_lowercase(), ctx.query.search)
    }
}

fn page_summary(page: &Page) -> String {
    match page.range() {
        Some((first, last)) => format!(
            "{first}–{last} of {total} · page {number} of {pages}",
            total = page.total(),
            number = page.number(),
            pages = page.total_pages()
        ),
        None => "Nothing to show".to_string(),
    }
}
