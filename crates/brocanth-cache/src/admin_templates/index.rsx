use crate::admin::IndexCtx;
use crate::admin_templates::parts;

// The cache, with nothing made up.
//
// There is no entry count, no hit rate and no size on this screen because
// `Cache` has no `stats()`, and a panel holds an `Arc<dyn Cache>`. The
// paragraph under the heading says so. A dashboard that filled the space
// with a figure it could not read would be the worse screen: an invented
// number gets believed, and an admission is a to-do item.

pub fn Index(ctx: &IndexCtx) -> Element {
    // Inline, not served: the admin answers with a page and mounts no asset
    // route, and a panel is in no position to add one. No preflight — the
    // admin has already reset the document.
    <style>{! crate::admin::STYLE }</style>

    <h1 class={parts::heading()}>Cache</h1>
    <p class={parts::note()}>
        Two buttons and no numbers. The cache trait has
        <span class="font-mono text-xs"> clear </span>and
        <span class="font-mono text-xs"> purge_expired </span>and no
        <span class="font-mono text-xs"> stats</span>, so this tab can empty
        things and report what it emptied, and cannot tell you how much is in
        there. The largest value it will store is {&ctx.max_value_bytes}.
    </p>

    if !ctx.notice.is_empty() {
        // `role="status"` and not an alert: this is the outcome of
        // something the reader just did, not news.
        <p class={parts::notice()} role="status">{&ctx.notice}</p>
    }

    <h2 class={parts::subheading()}>Namespaces</h2>
    if ctx.rows.is_empty() {
        <div class={parts::card()}>
            <div class={parts::empty()}>
                No namespace is declared, so there is nothing here to empty. A
                namespace is a prefix the caller chose rather than a record the
                cache keeps, so this tab cannot find them: the application lists
                the ones it owns with CachePanel::namespace.
            </div>
        </div>
    } else {
        <div class={parts::card()}>
            <table class={parts::table()}>
                <thead>
                    <tr>
                        <th class={parts::th()}>Namespace</th>
                        if ctx.can_act {
                            <th class={parts::th()} aria-label="Actions"></th>
                        }
                    </tr>
                </thead>
                <tbody>
                    for row in &ctx.rows {
                        <tr class="hover:bg-stage/60">
                            <td class={parts::td()}>
                                <span class="font-mono text-xs">{&row.name}</span>
                            </td>
                            if ctx.can_act {
                                <td class="text-right px-3.5 py-2.5 border-b border-line align-top whitespace-nowrap">
                                    // A form and not a link, because this
                                    // changes something and a crawler must
                                    // not be able to fire it. No `dp-`
                                    // either: dp-swap leaves mutating verbs
                                    // to the server.
                                    <form class="inline m-0" method="post" action={&row.clear_href}>
                                        <button class={parts::row_button_danger()} type="submit">
                                            Clear
                                        </button>
                                    </form>
                                </td>
                            }
                        </tr>
                    }
                </tbody>
            </table>
        </div>
    }

    <h2 class={parts::subheading()}>Expired entries</h2>
    <p class={parts::note()}>
        A TTL is enforced on read, so an expired entry is already a miss. This
        is only about the disk those rows still occupy.
        // On one line with the words around it: a newline between text and
        // an interpolation is not a space once the markup is written out.
        <span>One press removes up to {ctx.purge_limit} rows</span>, and a
        table that needs more than that wants a recurring job rather than a
        person holding a button down.
    </p>
    if ctx.can_act {
        <form class="mt-3" method="post" action={&ctx.purge_href}>
            <button class={parts::button()} type="submit">Purge expired</button>
        </form>
    }
}
