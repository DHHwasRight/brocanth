// Ejected from brocanth-admin 0.1.0 by `brocanth admin eject layout`.
//
// Yours now: an upgrade of brocanth-admin does not change this file.
// Compare it with the shipped `layout.rsx` after one. Utility classes
// that the shipped stylesheet does not contain have no effect: add
// a stylesheet of your own for those (`AdminConfig::stylesheet`).

use brocanth_admin::{INLINE_SCRIPT as SCRIPT, STYLESHEET as STYLE};
use brocanth_admin::view::LayoutCtx;

// The shell every screen is wrapped in. It is a `View` like any other, so an
// application that wants its own chrome replaces this one and keeps the six
// screens, or replaces a screen and keeps this.
//
// Two things ride here and nowhere else: the stylesheet, built from
// `style/admin.css` and embedded, and dp-swap, vendored beside it. Both are
// inline rather than served, because the admin answers with a page and
// mounts no asset route -- widening that seam for two files would be the
// tail wagging the dog.

pub fn Layout(ctx: &LayoutCtx) -> Element {
    <!DOCTYPE html>
    <html lang="en" class="h-full">
        <head>
            <meta charset="utf-8" />
            <meta name="viewport" content="width=device-width, initial-scale=1" />
            <title>{page_title(ctx)}</title>
            <style>{! STYLE }</style>
            // After the shipped sheet, so an equal selector here wins, and
            // a `:root` rule wins over the theme layer whatever its order.
            for href in &ctx.chrome.stylesheets {
                <link rel="stylesheet" href={href} />
            }
        </head>
        <body class="h-full m-0 font-sans text-[15px] leading-normal text-ink bg-stage antialiased">
            <div class="flex min-h-full">
                <aside class="w-60 shrink-0 bg-white border-r border-line flex flex-col gap-6 py-6">
                    <a class="px-6 text-[15px] font-semibold text-ink no-underline"
                       href={&ctx.chrome.base_path} dp-boost>
                        {&ctx.chrome.header}
                    </a>
                    <nav class="flex flex-col gap-0.5 px-3" aria-label="Sections">
                        for item in &ctx.chrome.nav {
                            // `aria-current` and not only a class: it is
                            // what tells a screen reader which screen this
                            // is, and it is the thing to assert on, because
                            // a class name is a detail of the stylesheet.
                            if item.current {
                                <a href={&item.href} class={nav_class(true)}
                                   aria-current="page" dp-boost>{&item.label}</a>
                            } else {
                                <a href={&item.href} class={nav_class(false)} dp-boost>
                                    {&item.label}
                                </a>
                            }
                        }
                    </nav>
                    // No `dp-boost`: the front page is not an admin screen,
                    // and a swap into `dp-layout="admin"` would be wrong.
                    if let Some(href) = &ctx.chrome.site_url {
                        <a class={nav_class(false)} href={href}>View site</a>
                    }
                </aside>
                // `dp-layout` marks what a swap may replace. The rail is
                // outside it, so moving between screens does not rebuild
                // the navigation or lose its scroll position.
                <main class="flex-1 min-w-0 px-10 py-8" dp-layout="admin">
                    {! &ctx.body }
                </main>
            </div>
            <script>{! SCRIPT }</script>
            <script>{! brocanth_admin::MUTATE_SCRIPT }</script>
        </body>
    </html>
}

fn page_title(ctx: &LayoutCtx) -> String {
    if ctx.heading.is_empty() || ctx.heading == ctx.chrome.title {
        ctx.chrome.title.clone()
    } else {
        format!("{} · {}", ctx.heading, ctx.chrome.title)
    }
}

fn nav_class(current: bool) -> &'static str {
    if current {
        "block rounded-control px-3 py-1.5 text-sm font-semibold no-underline text-accent bg-accent/8"
    } else {
        "block rounded-control px-3 py-1.5 text-sm no-underline text-ink-soft hover:text-ink hover:bg-stage"
    }
}
