use crate::admin::ListCtx;
use crate::admin_templates::chrome::Chrome;
use crate::admin_templates::parts;

// Every account, newest first.
//
// Five columns and no sixth. There is no session count, because one count per
// row is one query per row and the store has no batched count; the row links
// to the account, whose own screen lists its sessions in one call.
//
// "Password" is a column and not a problem: an account with no password signs
// in through a provider, which is the ordinary shape of an account created by
// Google or Apple. The column says which accounts a password reset can reach.

pub fn Accounts(ctx: &ListCtx) -> Element {
    <Chrome volatile={ctx.volatile} />
    <h1 class={parts::heading()}>Accounts</h1>
    <p class="text-sm text-ink-soft mb-4">{&ctx.total}</p>

    if ctx.rows.is_empty() {
        <div class={parts::card()}>
            <div class={parts::empty()}>No account has been created.</div>
        </div>
    } else {
        <div class={parts::card()}>
            <table class={parts::table()}>
                <thead>
                    <tr>
                        <th class={parts::th()}>Address</th>
                        <th class={parts::th()}>Verified</th>
                        <th class={parts::th()}>Password</th>
                        <th class={parts::th()}>Created</th>
                        <th class={parts::th()} aria-label="Account"></th>
                    </tr>
                </thead>
                <tbody>
                    for row in &ctx.rows {
                        <tr class="hover:bg-stage/60">
                            <td class={parts::td()}>
                                if row.has_email {
                                    <span>{&row.email}</span>
                                } else {
                                    <span class="text-ink-faint italic">{&row.email}</span>
                                }
                                <span class="block mt-1 font-mono text-xs text-ink-faint [overflow-wrap:anywhere]">
                                    {&row.id}
                                </span>
                            </td>
                            <td class={parts::td()}>
                                if row.verified {
                                    <span class={parts::yes()}>yes</span>
                                } else {
                                    <span class={parts::no()}>no</span>
                                }
                            </td>
                            <td class={parts::td()}>
                                if row.password {
                                    <span class={parts::yes()}>yes</span>
                                } else {
                                    <span class={parts::no()}>provider only</span>
                                }
                            </td>
                            <td class={parts::td_muted()}>{&row.created}</td>
                            <td class="text-right px-3.5 py-2.5 border-b border-line align-top whitespace-nowrap">
                                <a class="text-sm text-accent no-underline hover:underline"
                                   href={&row.href} dp-boost>Sessions</a>
                            </td>
                        </tr>
                    }
                </tbody>
            </table>
        </div>

        <nav class="flex gap-2.5 mt-4 text-sm" aria-label="Pages">
            if let Some(previous) = &ctx.previous {
                <a class="text-accent no-underline hover:underline" href={previous} dp-boost>Previous</a>
            }
            <span class="text-ink-soft">{&ctx.summary}</span>
            if let Some(next) = &ctx.next {
                <a class="text-accent no-underline hover:underline" href={next} dp-boost>Next</a>
            }
        </nav>
    }
}
