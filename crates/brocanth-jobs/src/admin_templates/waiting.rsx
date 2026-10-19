use crate::admin::WaitingCtx;
use crate::admin_templates::parts;
use crate::admin_templates::tabs::Tabs;

// Retries and scheduled jobs, which are one screen because they are one
// query with one predicate changed. What separates them is `attempts`, and
// the caller has already decided which it asked for.

pub fn Waiting(ctx: &WaitingCtx) -> Element {
    <Tabs tabs={&ctx.tabs} />
    <h1 class={parts::heading()}>{&ctx.title}</h1>

    if ctx.rows.is_empty() {
        <div class={parts::card()}><div class={parts::empty()}>{&ctx.empty}</div></div>
    } else {
        <div class={parts::card()}>
            <table class={parts::table()}>
                <thead>
                    <tr>
                        <th class={parts::th()}>Job</th>
                        <th class={parts::th()}>Queue</th>
                        <th class={parts::th()}>Attempt</th>
                        <th class={parts::th()}>Runs</th>
                        if ctx.can_act {
                            <th class={parts::th()} aria-label="Actions"></th>
                        }
                    </tr>
                </thead>
                <tbody>
                    for row in &ctx.rows {
                        <tr class="hover:bg-stage/60">
                            <td class={parts::td()}>
                                <span class="font-mono text-xs">{&row.job_name}</span>
                                if let Some(error) = &row.error {
                                    <span class="block mt-1 max-w-[46ch] text-xs text-danger [overflow-wrap:anywhere]">
                                        {error}
                                    </span>
                                }
                            </td>
                            <td class={parts::td()}>{&row.queue}</td>
                            <td class={parts::td_muted()}>{&row.attempt}</td>
                            <td class={parts::td_muted()}>{&row.when}</td>
                            if ctx.can_act {
                                <td class="text-right px-3.5 py-2.5 border-b border-line align-top whitespace-nowrap">
                                    // Forms and not links, because both of
                                    // these change something and a crawler
                                    // must not be able to fire them. No
                                    // `dp-` either: dp-swap leaves mutating
                                    // verbs to the server.
                                    <form class="inline m-0" method="post" action={&row.run_href}>
                                        <button class={parts::row_button()} type="submit">Run now</button>
                                    </form>
                                    <span class="text-ink-faint"> · </span>
                                    <form class="inline m-0" method="post" action={&row.delete_href}>
                                        <button class={parts::row_button_danger()} type="submit">Delete</button>
                                    </form>
                                </td>
                            }
                        </tr>
                    }
                </tbody>
            </table>
        </div>
    }
}
