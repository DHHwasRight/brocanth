// Layer 1. The shell: sidebar and the outlet everything else lives in. It is
// the one layer a partial never contains, so it is never re-sent.
//
// It covers `src/pages` and so every route below it, which is why nothing
// names it: which layout wraps a page is a property of where the page is.
//
// `layout` is the URL prefix this one covers and `path` is the request being
// answered. `html!` fills both, and fills them only because this signature
// asks for them.

use crate::data::{sections, Section};
use crate::pages::TITLE;

pub fn Layout(layout: &str, path: &str, children: Element) -> Element {
    <!DOCTYPE html>
    <html lang="en">
        <head>
            <meta charset="utf-8">
            <meta name="viewport" content="width=device-width, initial-scale=1">
            <title>{TITLE}</title>
            <link rel="stylesheet" href={crate::assets::url("css/site.css")}>
        </head>
        <body dp-boost dp-target="#app">
            <div class="frame">
                <aside class="rail">
                    <div class="rail-mark"></div>
                    <nav class="rail-nav" id="rail-nav">
                        for section in sections() {
                            <a href={section.url()} class={rail_class(&section.url(), path)}>
                                <span class="dot" style={dot_style(section)}></span>
                                {section.name}
                            </a>
                        }
                    </nav>
                    <div class="rail-foot"></div>
                </aside>
                // The marker goes on the outlet, not the body: a swap
                // replaces the marked element's children, so everything
                // rendered beside it -- the whole sidebar -- is preserved by
                // construction.
                <main id="app" dp-layout={layout} class="stage">{children}</main>
            </div>
            <script src="/dpswap.min.js"></script>
            <script src={crate::assets::url("js/boot.js")}></script>
        </body>
    </html>
}

fn rail_class(href: &str, path: &str) -> &'static str {
    if path == href || path.starts_with(&format!("{href}/")) {
        "rail-link is-current"
    } else {
        "rail-link"
    }
}

fn dot_style(section: &Section) -> String {
    let colour = section.cards.first().map_or("#cdb4db", |c| c.colour());
    format!("background:{colour}")
}

// ------------------------------------------------------------ out of band
//
// The shell is skipped by every partial, so it is never re-sent -- which is
// the point, and the reason its own sidebar goes stale. This renders a
// replacement for exactly the part that depends on which page is showing.
// `dp-swap-oob` tells the client to apply it by id before the main swap.
//
// `html!` renders this once per answer that skips this layout, and never for
// the whole document, where the sidebar above is already right.

pub fn Oob(path: &str) -> Element {
    <nav class="rail-nav" id="rail-nav" dp-swap-oob="outerHTML">
        for section in sections() {
            <a href={section.url()} class={rail_class(&section.url(), path)}>
                <span class="dot" style={dot_style(section)}></span>
                {section.name}
            </a>
        }
    </nav>
}
