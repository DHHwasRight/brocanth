use brocanth_admin::view::{LayoutCtx, ListCtx};

// An application's own screens, written the way an application would write
// them: a `.rsx` file, components taking the admin's context types, and no
// knowledge of how the default ones are built.

/// A list that is nothing like the shipped one: no table at all.
pub fn PostList(ctx: &ListCtx) -> Element {
    <section class="my-own-list" data-model={&ctx.model.key}>
        <h1>Our posts</h1>
        <ol>
            for row in &ctx.rows {
                <li><a href={&row.href}>{&row.label}</a></li>
            }
        </ol>
    </section>
}

/// A layout of the application's own, to show the shell is a seam too.
pub fn Shell(ctx: &LayoutCtx) -> Element {
    <!DOCTYPE html>
    <html lang="en">
        <head><title>{&ctx.heading}</title></head>
        <body class="our-shell">{! &ctx.body }</body>
    </html>
}
