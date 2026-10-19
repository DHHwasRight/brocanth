use crate::admin::WaitingCtx;
use crate::admin_templates::tabs::Tabs;

// Retries and scheduled jobs, which are one screen because they are one
// query with one predicate changed. What separates them is `attempts`, and
// the caller has already decided which it asked for.

pub fn Waiting(ctx: &WaitingCtx) -> Element {
    <Tabs tabs={&ctx.tabs} />
    <h1>{&ctx.title}</h1>

    if ctx.rows.is_empty() {
        <div class="card"><div class="empty">{&ctx.empty}</div></div>
    } else {
        <div class="card">
            <table>
                <thead>
                    <tr>
                        <th>Job</th><th>Queue</th><th>Attempt</th><th>Runs</th>
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
                                if let Some(error) = &row.error {
                                    <span class="jobs-err">{error}</span>
                                }
                            </td>
                            <td>{&row.queue}</td>
                            <td class="muted">{&row.attempt}</td>
                            <td class="muted">{&row.when}</td>
                            if ctx.can_act {
                                <td>
                                    // A form and not a link, because both of
                                    // these change something and a crawler
                                    // must not be able to fire them.
                                    <form class="jobs-act" method="post" action={&row.run_href}>
                                        <button type="submit">Run now</button>
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
