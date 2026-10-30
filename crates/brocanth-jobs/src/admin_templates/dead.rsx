use crate::admin::DeadCtx;
use crate::admin_templates::parts;
use crate::admin_templates::tabs::Tabs;

// Jobs that used up every attempt.
//
// The payload is here, verbatim, because the question this screen exists to
// answer is "why did this not happen", and the answer is the error plus the
// arguments it was given.

pub fn Dead(ctx: &DeadCtx) -> Element {
    <Tabs tabs={&ctx.tabs} />
    <h1 class={parts::heading()}>Dead</h1>

    if ctx.rows.is_empty() {
        <div class={parts::card()}>
            <div class={parts::empty()}>Nothing has run out of attempts.</div>
        </div>
    } else {
        <div class={parts::card()}>
            <table class={parts::table()}>
                <thead>
                    <tr>
                        <th class={parts::th()}>Job</th>
                        <th class={parts::th()}>Queue</th>
                        <th class={parts::th()}>Attempts</th>
                        <th class={parts::th()}>Failed</th>
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
                                <span class="block mt-1 max-w-[46ch] text-xs text-danger [overflow-wrap:anywhere]">
                                    {&row.error}
                                </span>
                                <span class="block mt-1 max-w-[60ch] font-mono text-xs text-ink-faint [overflow-wrap:anywhere]">
                                    {&row.payload}
                                </span>
                            </td>
                            <td class={parts::td()}>{&row.queue}</td>
                            <td class={parts::td_muted()}>{&row.attempts}</td>
                            <td class={parts::td_muted()}>{&row.failed}</td>
                            if ctx.can_act {
                                <td class="text-right px-3.5 py-2.5 border-b border-line align-top whitespace-nowrap">
                                    // `dp-post` with no value posts to the form's
                                    // action, so the form still works with
                                    // no script. The answer is a redirect,
                                    // which dp-mutate does not follow for a
                                    // `delete` swap: the row leaving the
                                    // table is the whole answer.
                                    <form class="inline m-0" method="post" action={&row.retry_href}
                                          dp-post dp-target="closest tr" dp-swap="delete">
                                        <button class={parts::row_button()} type="submit">Retry</button>
                                    </form>
                                    <span class="text-ink-faint"> · </span>
                                    <form class="inline m-0" method="post" action={&row.delete_href}
                                          dp-post dp-target="closest tr" dp-swap="delete">
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
