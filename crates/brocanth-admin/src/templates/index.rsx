use crate::view::IndexCtx;

/// The front page: every model the admin discovered, as a card each.
pub fn Index(ctx: &IndexCtx) -> Element {
    <h1>{&ctx.chrome.title}</h1>
    <p class="muted">{summary(ctx)}</p>
    if ctx.models.is_empty() {
        <div class="empty">Nothing to administer. Either no model was discovered, or the policy denies every one of them.</div>
    }
    <div class="index-grid">
        for entry in &ctx.models {
            <div class="index-card">
                <h2><a href={&entry.href}>{&entry.label}</a></h2>
                <p class="muted mono">{&entry.module_path}</p>
                <p class="muted">{entry.columns} columns</p>
                if entry.can_create {
                    <a class="btn" href={&entry.create_href}>Add</a>
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
