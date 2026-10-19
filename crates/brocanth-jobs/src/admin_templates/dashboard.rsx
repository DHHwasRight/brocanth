use crate::admin::DashboardCtx;
use crate::admin_templates::tabs::Tabs;

// What is in the queue, and how far behind it is.
//
// Sidekiq's dashboard also plots processed and failed over time. There is no
// graph here, and that is not an omission: this queue deletes the row when
// the job succeeds, so the history the graph plots does not exist. Drawing
// one means a counters table, which is a decision and not a template.

pub fn Dashboard(ctx: &DashboardCtx) -> Element {
    <Tabs tabs={&ctx.tabs} />

    <div class="jobs-counts">
        for count in &ctx.counts {
            <a class="jobs-count" href={&count.href}>
                <span class="jobs-count-value">{count.value}</span>
                <span class="jobs-count-label">{&count.label}</span>
            </a>
        }
    </div>

    // Depth does not tell you a queue is in trouble: ten thousand jobs that
    // drain in a second are fine, and three where the oldest has waited an
    // hour are not. This is the number that does.
    <p class="muted">Oldest runnable job: {&ctx.oldest}</p>

    <h2>Queues</h2>
    if ctx.queues.is_empty() {
        <div class="card"><div class="empty">Nothing on any queue.</div></div>
    } else {
        <div class="card">
            <table>
                <thead>
                    <tr>
                        <th>Queue</th><th>Enqueued</th><th>Running</th>
                        <th>Scheduled</th><th>Latency</th>
                    </tr>
                </thead>
                <tbody>
                    for row in &ctx.queues {
                        <tr>
                            <td>{&row.queue}</td>
                            <td>{row.ready}</td>
                            <td>{row.running}</td>
                            <td>{row.scheduled}</td>
                            <td class="muted">{&row.latency}</td>
                        </tr>
                    }
                </tbody>
            </table>
        </div>
    }
}
