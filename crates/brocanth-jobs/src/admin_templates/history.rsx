use crate::admin::HistoryCtx;
use crate::admin_templates::parts;
use crate::admin_templates::tabs::Tabs;

// How much finished, over time.
//
// The tab only exists when the queue keeps an archive, so this screen is
// never reached on a queue that deletes a job's row when it succeeds. See
// `has_history`.
//
// The bars are divs, not a chart library and not an inline SVG. A bar chart
// is a row of rectangles with known heights, which is one CSS property, and
// a backoffice should not load a plotting library to draw twenty-four of
// them. It also means the chart inherits the admin's own colours.

pub fn History(ctx: &HistoryCtx) -> Element {
    <Tabs tabs={&ctx.tabs} />
    <h1 class={parts::heading()}>History</h1>

    <div class={parts::card()}>
        <div class="px-3.5 pt-3.5">
            <span class="block text-2xl font-semibold leading-none">{ctx.total}</span>
            <span class="block mt-1 text-[11px] font-medium uppercase tracking-wider text-ink-faint">
                finished in {&ctx.window}
            </span>
        </div>

        // `items-end` is what makes a height a bar: every column is the
        // full height of the strip and the filled part sits at the bottom.
        <div class="flex items-end gap-0.5 h-32 px-3.5 mt-4" role="img"
             aria-label={&ctx.window}>
            for bar in &ctx.bars {
                <div class="flex-1 flex flex-col justify-end h-full" title={&bar.title}>
                    <div class="w-full bg-accent rounded-t-[2px] min-h-px"
                         style={format!("height:{}%", bar.height)}></div>
                </div>
            }
        </div>
        <div class="flex gap-0.5 px-3.5 pb-3 pt-1">
            for bar in &ctx.bars {
                <div class="flex-1 text-[10px] text-ink-faint whitespace-nowrap">{&bar.label}</div>
            }
        </div>
    </div>

    // The caveat belongs on the screen and not only in a doc comment. The
    // wrong reading of this chart -- "no bar means it did not run" -- is
    // exactly the one somebody reaches for during an incident.
    <p class="mt-2 text-xs text-ink-faint">
        Counts jobs that finished, not jobs that ran. Delivery is at least
        once, so a job that ran and then lost its lease is counted once.
        Read it as a lower bound.
    </p>

    <h2 class="mt-6 mb-2 text-base font-semibold text-ink">By job</h2>
    if ctx.kinds.is_empty() {
        <div class={parts::card()}>
            <div class={parts::empty()}>Nothing finished in {&ctx.window}.</div>
        </div>
    } else {
        <div class={parts::card()}>
            <table class={parts::table()}>
                <thead>
                    <tr>
                        <th class={parts::th()}>Job</th>
                        <th class={parts::th()}>Finished</th>
                        <th class={parts::th()}>Share</th>
                    </tr>
                </thead>
                <tbody>
                    for row in &ctx.kinds {
                        <tr class="hover:bg-stage/60">
                            <td class={parts::td()}>
                                <span class="font-mono text-xs">{&row.kind}</span>
                            </td>
                            <td class={parts::td()}>{row.count}</td>
                            <td class={parts::td_muted()}>{row.share}%</td>
                        </tr>
                    }
                </tbody>
            </table>
        </div>
    }
}
