use crate::templates::parts;
use crate::view::IndexCtx;

/// The front page: every model the admin discovered, as a card each.
pub fn Index(ctx: &IndexCtx) -> Element {
    <h1 class={parts::heading()}>{&ctx.chrome.title}</h1>
    <p class="mt-1 mb-6 text-sm text-ink-soft">{summary(ctx)}</p>

    if ctx.models.is_empty() {
        <div class={parts::card()}>
            <div class={parts::empty()}>
                Nothing to administer. Either no model was discovered, or the
                policy denies every one of them.
            </div>
        </div>
    }

    <div class="grid gap-3 [grid-template-columns:repeat(auto-fill,minmax(250px,1fr))]">
        for entry in &ctx.models {
            <div class="bg-white border border-line rounded-card p-4 shadow-card flex flex-col gap-1">
                <h2 class="m-0 text-[15px] font-semibold">
                    <a class="text-ink no-underline hover:text-accent"
                       href={&entry.href} dp-boost>{&entry.label}</a>
                </h2>
                <p class="m-0 font-mono text-xs text-ink-faint">{&entry.module_path}</p>
                <p class="m-0 text-sm text-ink-soft">{entry.columns} columns</p>
                if entry.can_create {
                    <a class={parts::button()} href={&entry.create_href} dp-boost>Add</a>
                }
            </div>
        }
    </div>
}

fn summary(ctx: &IndexCtx) -> String {
    match ctx.models.len() {
        1 => "1 model".to_string(),
        n => format!("{n} models"),
    }
}
