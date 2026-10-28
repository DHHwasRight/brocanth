use crate::admin::BusyCtx;
use crate::admin_templates::parts;
use crate::admin_templates::tabs::Tabs;

// What every worker is holding right now.
//
// A worker with nothing in flight does not appear. There is no worker
// table on either queue: a worker is known here only by the jobs it holds,
// so one holding none has nothing to show and nothing to do to it.

pub fn Busy(ctx: &BusyCtx) -> Element {
    <Tabs tabs={&ctx.tabs} />
    <h1 class={parts::heading()}>Busy</h1>

    if ctx.rows.is_empty() {
        <div class={parts::card()}><div class={parts::empty()}>No job is running.</div></div>
    } else {
        <div class={parts::card()}>
            <table class={parts::table()}>
                <thead>
                    <tr>
                        <th class={parts::th()}>Job</th>
                        <th class={parts::th()}>Queue</th>
                        <th class={parts::th()}>Worker</th>
                        <th class={parts::th()}>Attempt</th>
                        <th class={parts::th()}>Running for</th>
                    </tr>
                </thead>
                <tbody>
                    for row in &ctx.rows {
                        <tr class="hover:bg-stage/60">
                            <td class={parts::td()}><span class="font-mono text-xs">{&row.job_name}</span></td>
                            <td class={parts::td()}>{&row.queue}</td>
                            <td class={parts::td()}>
                                <span class="font-mono text-xs">{&row.worker}</span>
                                // A lease is the proof a worker is alive,
                                // and one that runs out puts the job back
                                // by itself. So this is not "stopped
                                // answering": it is a job that has been
                                // held longer than anyone expected.
                                if row.stale {
                                    <span class="ml-1.5 text-xs font-semibold text-danger">
                                        · running long
                                    </span>
                                }
                            </td>
                            <td class={parts::td_muted()}>{&row.attempt}</td>
                            <td class={parts::td_muted()}>{&row.running_for}</td>
                        </tr>
                    }
                </tbody>
            </table>
        </div>
    }
}
