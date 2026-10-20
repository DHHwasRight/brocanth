use crate::admin::OverviewCtx;
use crate::admin_templates::parts;

// The broker, on one screen.
//
// The order is the argument. PocketBase's realtime screen leads with the
// connection count; this one leads with the indexed / scanned split,
// because the connection count is the size of the deployment and the split
// is the cost of a write. A thousand idle clients are free. Forty scanned
// subscriptions are forty policy evaluations, every time a row changes.
//
// The table under it is `Broker::audit`, which names the topic and quotes
// the reason the policy would not invert. That is what makes the number
// actionable instead of merely alarming.

pub fn Overview(ctx: &OverviewCtx) -> Element {
    // Inline, not served: the admin answers with a page and mounts no asset
    // route, and a panel is in no position to add one. No preflight — the
    // admin has already reset the document.
    <style>{! crate::admin::STYLE }</style>

    <h1 class={parts::heading()}>Realtime</h1>

    <section class={parts::verdict_card()} aria-label="Subscription index">
        <p class="m-0 text-sm font-medium">{&ctx.verdict}</p>
        <div class="flex flex-wrap gap-10 mt-4">
            <div>
                <span class={parts::figure()}>{ctx.indexed}</span>
                <span class={parts::figure_label()}>On the index</span>
            </div>
            <div>
                <span class={scanned_figure(ctx.healthy)}>{ctx.scanned}</span>
                <span class={parts::figure_label()}>Scanned per change</span>
            </div>
        </div>
        if !ctx.share.is_empty() {
            <p class="mt-3 mb-0 text-xs text-ink-faint">{&ctx.share}</p>
        }
    </section>

    <div class="grid gap-2.5 mt-5 [grid-template-columns:repeat(auto-fit,minmax(150px,1fr))]">
        for count in &ctx.counts {
            <div class="bg-white border border-line rounded-card p-3.5 shadow-card">
                <span class="block text-2xl font-semibold leading-none tabular-nums">
                    {count.value}
                </span>
                <span class="block mt-1 text-[11px] font-medium uppercase tracking-wider text-ink-faint">
                    {&count.label}
                </span>
                <span class="block mt-1 text-xs text-ink-soft">{&count.note}</span>
            </div>
        }
    </div>

    if !ctx.denied.is_empty() {
        <p class="mt-4 rounded-control border border-danger/30 bg-danger/5 px-3.5 py-2.5 text-sm text-danger">
            {&ctx.denied}
        </p>
    }

    <h2 class={parts::subheading()}>Topics on the slow path</h2>
    if ctx.fallbacks.is_empty() {
        <div class={parts::card()}>
            <div class={parts::empty()}>
                No topic is on the slow path. Every write is a map lookup.
            </div>
        </div>
    } else {
        <div class={parts::card()}>
            <table class={parts::table()}>
                <thead>
                    <tr>
                        <th class={parts::th()}>Topic</th>
                        <th class={parts::th()}>Why there is no key</th>
                        <th class={parts::th()}>Subscriptions</th>
                    </tr>
                </thead>
                <tbody>
                    for row in &ctx.fallbacks {
                        <tr class="hover:bg-stage/60">
                            <td class={parts::td()}>
                                <span class="font-mono text-xs">{&row.topic}</span>
                            </td>
                            <td class={parts::td_muted()}>{&row.reason}</td>
                            <td class={parts::td()}>{row.subscribers}</td>
                        </tr>
                    }
                </tbody>
            </table>
        </div>
    }
}

/// Nothing scanned is a faint zero; anything scanned is in the body colour,
/// so it reads as a figure rather than as an absence.
///
/// Deliberately not red. The fallback works, it runs in memory against a
/// row read once, and the crate documentation says plainly that even this
/// path beats PocketBase's only path. It is a cost to go and look at, not
/// an outage, and the palette's one alarm colour would say otherwise.
fn scanned_figure(healthy: bool) -> &'static str {
    if healthy {
        "block text-[40px] font-semibold leading-none tabular-nums text-ink-faint"
    } else {
        "block text-[40px] font-semibold leading-none tabular-nums text-ink"
    }
}
