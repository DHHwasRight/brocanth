use crate::admin::DashboardCtx;
use crate::admin_templates::parts;
use crate::admin_templates::tabs::Tabs;

// What is in the queue, and how far behind it is.
//
// Sidekiq's dashboard also plots processed and failed over time. That is the
// History tab, and it is only there when the queue behind the panel keeps an
// archive -- a queue that deletes the row when the job succeeds has no
// history to plot, and a chart of zeroes reads like an outage.

pub fn Dashboard(ctx: &DashboardCtx) -> Element {
    <Tabs tabs={&ctx.tabs} />

    <div class="grid gap-2.5 mb-5 [grid-template-columns:repeat(auto-fit,minmax(130px,1fr))]">
        for count in &ctx.counts {
            <a class="block bg-white border border-line rounded-card p-3.5 no-underline text-ink shadow-card hover:border-accent"
               href={&count.href} dp-boost>
                <span class="block text-2xl font-semibold leading-none">{count.value}</span>
                <span class="block mt-1 text-[11px] font-medium uppercase tracking-wider text-ink-faint">
                    {&count.label}
                </span>
            </a>
        }
    </div>

    // Depth does not tell you a queue is in trouble: ten thousand jobs that
    // drain in a second are fine, and three where the oldest has waited an
    // hour are not. This is the number that does.
    <p class="text-sm text-ink-soft">Oldest runnable job: {&ctx.oldest}</p>

    <h2 class="mt-6 mb-2 text-base font-semibold text-ink">Queues</h2>
    if ctx.queues.is_empty() {
        <div class={parts::card()}><div class={parts::empty()}>Nothing on any queue.</div></div>
    } else {
        <div class={parts::card()}>
            <table class={parts::table()}>
                <thead>
                    <tr>
                        <th class={parts::th()}>Queue</th>
                        <th class={parts::th()}>Enqueued</th>
                        <th class={parts::th()}>Running</th>
                        <th class={parts::th()}>Scheduled</th>
                        <th class={parts::th()}>Latency</th>
                    </tr>
                </thead>
                <tbody>
                    for row in &ctx.queues {
                        <tr class="hover:bg-stage/60">
                            <td class={parts::td()}>{&row.queue}</td>
                            <td class={parts::td()}>{row.ready}</td>
                            <td class={parts::td()}>{row.running}</td>
                            <td class={parts::td()}>{row.scheduled}</td>
                            <td class={parts::td_muted()}>{&row.latency}</td>
                        </tr>
                    }
                </tbody>
            </table>
        </div>
    }
}
