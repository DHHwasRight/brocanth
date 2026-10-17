// Layer 2. One section. Holds the toolbar and the area the cards live in;
// moving between sections replaces this whole thing, moving within one does
// not.
//
// The directory is `[section]`, so the prefix this layout covers is not known
// until a request arrives: `html!` hands it `/alpha` under `/alpha/card-2`,
// never `/[section]`. A marker that read the same under every section would
// tell the client that two sections share a layer they do not.

use crate::data::section;

pub fn Layout(layout: &str, path: &str, children: Element) -> Element {
    <div class="section">
        // Outside the marker, so opening a card does not rebuild the toolbar.
        <header class="bar">
            <span class="bar-title">{title(path)}</span>
            <span class="bar-fill"></span>
            <span class="chip"></span>
            <span class="chip"></span>
        </header>
        // The board holds either the grid or the split; which one is decided
        // by the page below, so the container needs no state of its own.
        <div dp-layout={layout} class="board">{children}</div>
    </div>
}

/// The section's name, read back out of the path.
///
/// A layout is handed the request rather than the record: it sits above every
/// page under this directory, and those pages load different things. Two of
/// them would have to agree on a type for the layout to take one.
fn title(path: &str) -> &'static str {
    path.split('/')
        .nth(1)
        .and_then(section)
        .map_or("", |s| s.name)
}
