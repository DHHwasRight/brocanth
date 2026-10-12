use crate::view::DeleteCtx;

/// The confirmation. Deleting is a POST, so a crawler cannot do it.
pub fn Delete(ctx: &DeleteCtx) -> Element {
    <p class="crumbs">
        <a href={&ctx.chrome.base_path}>{&ctx.chrome.title}</a> <span> / </span> <a href={&ctx.model.href}>{&ctx.model.plural}</a> <span> / </span> Delete
    </p>
    <h1>{heading(ctx)}</h1>
    <p>Deleting <strong>{&ctx.label}</strong> cannot be undone.</p>
    <form method="post" action={&ctx.action}>
        <div class="actions">
            <button class="btn btn-danger" type="submit">{confirm_label(ctx)}</button>
            <a class="btn" href={&ctx.cancel_href}>Cancel</a>
        </div>
    </form>
}

fn heading(ctx: &DeleteCtx) -> String {
    format!("Delete this {}?", ctx.model.name.to_lowercase())
}

fn confirm_label(ctx: &DeleteCtx) -> String {
    format!("Yes, delete this {}", ctx.model.name.to_lowercase())
}
