use crate::admin::BusyCtx;
use crate::admin_templates::tabs::Tabs;

// What every worker is holding right now.
//
// A worker with nothing in flight does not appear. There is no worker
// table: a worker is known here only by the jobs it holds, so a worker
// holding none has nothing to show and nothing to do to it.

pub fn Busy(ctx: &BusyCtx) -> Element {
    <Tabs tabs={&ctx.tabs} />
    <h1>Busy</h1>

    if ctx.rows.is_empty() {
        <div class="card"><div class="empty">No job is running.</div></div>
    } else {
        <div class="card">
            <table>
                <thead>
                    <tr>
                        <th>Job</th><th>Queue</th><th>Worker</th>
                        <th>Attempt</th><th>Running for</th>
                    </tr>
                </thead>
                <tbody>
                    for row in &ctx.rows {
                        <tr>
                            <td><span class="mono">{&row.job_name}</span></td>
                            <td>{&row.queue}</td>
                            <td>
                                <span class="mono">{&row.worker}</span>
                                // A worker that stopped saying it is alive
                                // still holds its jobs until somebody
                                // reclaims them, and it looks exactly like a
                                // busy one until this says otherwise.
                                if row.stale {
                                    <span class="jobs-stale"> · not responding</span>
                                }
                            </td>
                            <td class="muted">{&row.attempt}</td>
                            <td class="muted">{&row.running_for}</td>
                        </tr>
                    }
                </tbody>
            </table>
        </div>
    }
}
