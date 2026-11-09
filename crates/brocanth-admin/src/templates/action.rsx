use crate::templates::parts;
use crate::view::ActionCtx;

/// The confirmation before an action over selected rows. It lists what the
/// action will touch, and posts the same keys back with `confirm`. Nothing
/// is written until that second POST.
pub fn ConfirmAction(ctx: &ActionCtx) -> Element {
    <p class={parts::crumbs()}>
        <a class={parts::action()} href={&ctx.chrome.base_path} dp-boost>{&ctx.chrome.title}</a>
        <span>/</span>
        <a class={parts::action()} href={&ctx.model.href} dp-boost>{&ctx.model.plural}</a>
        <span>/</span>
        <span>{&ctx.label}</span>
    </p>
    <h1 class={parts::heading()}>{heading(ctx)}</h1>
    <p class="mt-2 mb-5 text-sm text-ink-soft">{summary(ctx)}</p>
    if ctx.skipped > 0 {
        <p class="mt-2 mb-5 text-sm text-ink-soft" role="note">{skipped(ctx)}</p>
    }
    <div class={parts::card()}>
        <table class={parts::table()}>
            <tbody>
                for record in &ctx.records {
                    <tr>
                        <td class={parts::td()}>
                            <a class={parts::action()} href={&record.href} dp-boost>{&record.label}</a>
                        </td>
                    </tr>
                }
            </tbody>
        </table>
    </div>
    <form class="mt-4" method="post" action={&ctx.action}>
        <input type="hidden" name="action" value={&ctx.key} />
        <input type="hidden" name="confirm" value="yes" />
        for record in &ctx.records {
            <input type="hidden" name="pk" value={&record.key} />
        }
        <div class="flex items-center gap-2">
            if ctx.destructive {
                <button class={parts::button_danger()} type="submit">{confirm_label(ctx)}</button>
            } else {
                <button class={parts::button_primary()} type="submit">{confirm_label(ctx)}</button>
            }
            <a class={parts::button()} href={&ctx.cancel_href} dp-boost>Cancel</a>
        </div>
    </form>
}

fn heading(ctx: &ActionCtx) -> String {
    format!("{}?", ctx.label)
}

fn summary(ctx: &ActionCtx) -> String {
    let count = ctx.records.len();
    let noun = noun(ctx, count);
    if ctx.destructive {
        format!("This deletes {count} {noun}. It cannot be undone.")
    } else {
        format!("This changes {count} {noun}.")
    }
}

fn skipped(ctx: &ActionCtx) -> String {
    let what = if ctx.skipped == 1 { "record is" } else { "records are" };
    format!(
        "{} selected {what} left out: you may not change them, or they are gone.",
        ctx.skipped
    )
}

fn confirm_label(ctx: &ActionCtx) -> String {
    format!("Yes, {}", ctx.label.to_lowercase())
}

fn noun(ctx: &ActionCtx, count: usize) -> String {
    if count == 1 {
        ctx.model.name.to_lowercase()
    } else {
        ctx.model.plural.to_lowercase()
    }
}
