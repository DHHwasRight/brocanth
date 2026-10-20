use crate::admin::OperationsCtx;
use crate::admin_templates::parts;
use crate::admin_templates::parts::Style;

// Every operation the document describes.
//
// Ordered by path and then by verb, not grouped by tag. The question this
// screen answers is "what is at `/posts/{id}`", and a tag is a
// documentation choice an application may never have made — grouping by
// one would put half of a real document under "default".
//
// The summary column is often empty, and it is left empty rather than
// filled with the operationId. An undescribed operation is a true thing
// about the document, and this tab is a preview of the document.

pub fn Operations(ctx: &OperationsCtx) -> Element {
    <Style />

    <h1 class={parts::heading()}>{&ctx.title}</h1>
    <p class="mt-0 mb-4 text-sm text-ink-soft">
        <span>Version {&ctx.version}</span>
        <span class="text-ink-faint"> · </span>
        <span>{&ctx.counted}</span>
    </p>

    if !ctx.servers.is_empty() {
        <ul class="list-none m-0 mb-5 p-0 flex flex-wrap gap-2">
            for server in &ctx.servers {
                <li class="rounded-control border border-line bg-white px-2.5 py-1 font-mono text-xs text-ink-soft">
                    {server}
                </li>
            }
        </ul>
    }

    if ctx.rows.is_empty() {
        <div class={parts::card()}>
            <div class={parts::empty()}>
                This document describes no operation. A document built from a
                router gets one per route.
            </div>
        </div>
    } else {
        <div class={parts::card()}>
            <table class={parts::table()}>
                <thead>
                    <tr>
                        <th class={parts::th()}>Verb</th>
                        <th class={parts::th()}>Path</th>
                        <th class={parts::th()}>Summary</th>
                        <th class={parts::th()}>Tags</th>
                    </tr>
                </thead>
                <tbody>
                    for row in &ctx.rows {
                        <tr class="hover:bg-stage/60">
                            <td class={parts::td()}>
                                <span class={parts::verb_badge(&row.verb)}>{&row.verb}</span>
                            </td>
                            <td class={parts::td()}>
                                <a class="font-mono text-xs text-accent no-underline hover:underline"
                                   href={&row.href} dp-boost>{&row.path}</a>
                                if row.deprecated {
                                    <span class={parts::deprecated_badge()}>deprecated</span>
                                }
                                <span class="block mt-1 font-mono text-[11px] text-ink-faint">
                                    {&row.operation_id}
                                </span>
                            </td>
                            <td class={parts::td()}>{&row.summary}</td>
                            <td class={parts::td_muted()}>{&row.tags}</td>
                        </tr>
                    }
                </tbody>
            </table>
        </div>
    }
}
