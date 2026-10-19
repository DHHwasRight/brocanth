use crate::templates::parts;
use crate::view::DeleteCtx;

/// The confirmation. Deleting is a POST, so a crawler cannot do it, and
/// dp-swap does not enhance it: mutating verbs are the server's half.
pub fn Delete(ctx: &DeleteCtx) -> Element {
    <p class={parts::crumbs()}>
        <a class={parts::action()} href={&ctx.chrome.base_path} dp-boost>{&ctx.chrome.title}</a>
        <span>/</span>
        <a class={parts::action()} href={&ctx.model.href} dp-boost>{&ctx.model.plural}</a>
        <span>/</span>
        <span>Delete</span>
    </p>
    <h1 class={parts::heading()}>{heading(ctx)}</h1>
    <p class="mt-2 mb-5 text-sm text-ink-soft">
        Deleting <strong class="font-semibold text-ink">{&ctx.label}</strong> cannot be undone.
    </p>
    <form method="post" action={&ctx.action}>
        <div class="flex items-center gap-2">
            <button class={parts::button_danger()} type="submit">{confirm_label(ctx)}</button>
            <a class={parts::button()} href={&ctx.cancel_href} dp-boost>Cancel</a>
        </div>
    </form>
}

fn heading(ctx: &DeleteCtx) -> String {
    format!("Delete this {}?", ctx.model.name.to_lowercase())
}

fn confirm_label(ctx: &DeleteCtx) -> String {
    format!("Yes, delete this {}", ctx.model.name.to_lowercase())
}
