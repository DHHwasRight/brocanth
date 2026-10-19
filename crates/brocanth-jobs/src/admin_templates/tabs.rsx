use crate::admin::TabBar;

// The panel's tab strip, and the stylesheet it brings with it.
//
// Inline rather than served: the admin answers with a page and mounts no
// asset route, and a panel is in no position to add one. Its sheet carries
// no preflight — the admin's already reset the document — only the theme
// variables and the utilities these five templates name.

pub fn Tabs(tabs: &TabBar) -> Element {
    <style>{! crate::admin::STYLE }</style>
    <nav class="flex gap-1 mb-5 border-b border-line" aria-label="Queue">
        for tab in &tabs.tabs {
            if tab.current {
                <a href={&tab.href} class={tab_class(true)} aria-current="page">{&tab.label}</a>
            } else {
                <a href={&tab.href} class={tab_class(false)} dp-boost>{&tab.label}</a>
            }
        }
    </nav>
}

fn tab_class(current: bool) -> &'static str {
    if current {
        "px-3.5 py-2 -mb-px border-b-2 border-accent text-accent text-sm \
         font-semibold no-underline"
    } else {
        "px-3.5 py-2 -mb-px border-b-2 border-transparent text-ink-soft text-sm \
         no-underline hover:text-ink"
    }
}
