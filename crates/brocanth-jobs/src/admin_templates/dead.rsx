use crate::admin::DeadCtx;
use crate::admin_templates::tabs::Tabs;

// Jobs that used up every attempt.
//
// The payload is here, verbatim, because the question this screen exists to
// answer is "why did this not happen", and the answer is the error plus the
// arguments it was given.

pub fn Dead(ctx: &DeadCtx) -> Element {
    <Tabs tabs={&ctx.tabs} />
    <h1>Dead</h1>

    if ctx.rows.is_empty() {
        <div class="card"><div class="empty">Nothing has run out of attempts.</div></div>
    } else {
        <div class="card">
            <table>
                <thead>
                    <tr>
                        <th>Job</th><th>Queue</th><th>Attempts</th><th>Failed</th>
                        if ctx.can_act {
                            <th></th>
                        }
                    </tr>
                </thead>
                <tbody>
                    for row in &ctx.rows {
                        <tr>
                            <td>
                                <span class="mono">{&row.job_name}</span>
                                <span class="jobs-err">{&row.error}</span>
                                <span class="mono muted">{&row.payload}</span>
                            </td>
                            <td>{&row.queue}</td>
                            <td class="muted">{&row.attempts}</td>
                            <td class="muted">{&row.failed}</td>
                            if ctx.can_act {
                                <td>
                                    <form class="jobs-act" method="post" action={&row.retry_href}>
                                        <button type="submit">Retry</button>
                                    </form>
                                    <span class="muted"> · </span>
                                    <form class="jobs-act" method="post" action={&row.delete_href}>
                                        <button type="submit" class="danger">Delete</button>
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
