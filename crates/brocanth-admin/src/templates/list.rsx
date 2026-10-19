use crate::paging::Page;
use crate::templates::parts;
use crate::view::{ColumnHeader, ListCtx, RowView};

/// One model's records: search, sortable headers, rows and pagination.
pub fn List(ctx: &ListCtx) -> Element {
    <p class={parts::crumbs()}>
        <a class={parts::action()} href={&ctx.chrome.base_path} dp-boost>{&ctx.chrome.title}</a>
        <span>/</span>
        <span>{&ctx.model.plural}</span>
    </p>
    <div class={parts::bar()}>
        <h1 class={parts::heading()}>{&ctx.model.plural}</h1>
        <span class="flex-1"></span>
        if ctx.searchable {
            // A GET form, so dp-swap does enhance this one: searching is
            // navigation, and the result is a URL you can send to somebody.
            <form class="flex gap-1.5" method="get" action={&ctx.model.href}>
                <input
                    class="w-56 rounded-control border border-line bg-white px-2.5 py-1.5 text-sm text-ink focus:outline-none focus:border-accent focus:ring-2 focus:ring-accent/20"
                    type="search" name="q" value={&ctx.query.search}
                    placeholder={search_placeholder(ctx)} aria-label="Search" />
                if let Some(sort) = &ctx.query.sort {
                    <input type="hidden" name="sort" value={&sort.column} />
                    <input type="hidden" name="dir" value={sort.direction.as_str()} />
                }
                <button class={parts::button()} type="submit">Search</button>
            </form>
        }
        if ctx.can_create {
            <a class={parts::button_primary()} href={&ctx.create_href} dp-boost>{add_label(ctx)}</a>
        }
    </div>

    if ctx.rows.is_empty() {
        <div class={parts::card()}><div class={parts::empty()}>{empty_message(ctx)}</div></div>
    } else {
        <div class={parts::card()}>
            <table class={parts::table()}>
                <thead>
                    <tr>
                        for header in &ctx.columns {
                            <Header header={header} />
                        }
                        <th class={parts::th()} aria-label="Actions"></th>
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

    <div class="flex items-center gap-1.5 mt-4 flex-wrap text-sm">
        <span class="mr-auto text-xs text-ink-faint">{page_summary(&ctx.page)}</span>
        if let Some(href) = &ctx.previous_href {
            <a class={page_link()} href={href} rel="prev" dp-boost>Previous</a>
        }
        for link in &ctx.pages {
            if link.current {
                <a class={page_link_current()} href={&link.href} aria-current="page" dp-boost>{link.number}</a>
            } else {
                <a class={page_link()} href={&link.href} dp-boost>{link.number}</a>
            }
        }
        if let Some(href) = &ctx.next_href {
            <a class={page_link()} href={href} rel="next" dp-boost>Next</a>
        }
    </div>
}

/// One column header, a sort link when the column may be sorted on.
pub fn Header(header: &ColumnHeader) -> Element {
    <th class={parts::th()} aria-sort={aria_sort(header.sorted)}>
        if header.sortable {
            <a class="text-ink-faint no-underline hover:text-ink" href={&header.href} dp-boost>
                {&header.label}{sort_mark(header.sorted)}
            </a>
        } else {
            <span>{&header.label}</span>
        }
    </th>
}

/// One row. A foreign-key cell carries the target's label and links to it.
pub fn Record(row: &RowView) -> Element {
    <tr class="hover:bg-stage/60">
        for cell in &row.cells {
            <td class={parts::td()}>
                if let Some(href) = &cell.href {
                    <a class={parts::action()} href={href} dp-boost>{&cell.text}</a>
                } else {
                    <span>{&cell.text}</span>
                }
            </td>
        }
        <td class="text-right px-3.5 py-2.5 border-b border-line align-top whitespace-nowrap">
            <a class={parts::action()} href={&row.href} dp-boost>View</a>
            if let Some(href) = &row.edit_href {
                <span class="text-ink-faint"> · </span>
                <a class={parts::action()} href={href} dp-boost>Edit</a>
            }
            if let Some(href) = &row.delete_href {
                <span class="text-ink-faint"> · </span>
                <a class={parts::action()} href={href} dp-boost>Delete</a>
            }
        </td>
    </tr>
}

// Branching that produces a value lives in plain functions: inside a template
// body `if` opens a markup conditional, so `let x = if …` never binds.

/// Which way a sorted column is sorted, for a screen reader. The arrow
/// beside the label is for everybody else.
fn aria_sort(sorted: &str) -> &'static str {
    match sorted {
        "asc" => "ascending",
        "desc" => "descending",
        _ => "none",
    }
}

fn sort_mark(sorted: &str) -> &'static str {
    match sorted {
        "asc" => " \u{2191}",
        "desc" => " \u{2193}",
        _ => "",
    }
}

fn page_link() -> &'static str {
    "rounded-control border border-line bg-white px-2.5 py-1 text-ink no-underline hover:bg-stage"
}

fn page_link_current() -> &'static str {
    "rounded-control border border-accent bg-accent px-2.5 py-1 text-white no-underline"
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
