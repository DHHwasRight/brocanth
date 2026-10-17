// Layer 3. One expanded card: the detail panel at two thirds and its siblings
// collapsed to a list at one third. Switching cards replaces this; switching
// panels inside a card does not.

use crate::data::{section, Card, Section, TABS};

pub fn Layout(layout: &str, path: &str, children: Element) -> Element {
    if let Some((section, card)) = open(path) {
        <div class="split">
            <article class="detail" style={tint_style(card)}>
                <header class="detail-head">
                    <span class="swatch" style={swatch_style(card)}></span>
                    <span class="detail-title">{card.name}</span>
                    <a class="close" href={section.url()} aria-label="Back to the grid"></a>
                </header>
                <nav class="tabs" id="card-tabs">
                    for (slug, label) in TABS {
                        <a href={card.tab_url(section, slug)}
                           class={tab_class(&card.tab_url(section, slug), path)}>
                            {label}
                        </a>
                    }
                </nav>
                // The marker sits here, so moving between panels replaces one
                // panel and leaves the card header, its tabs and the sibling
                // list exactly where they are.
                <div dp-layout={layout} class="detail-body">{children}</div>
            </article>

            // Every card, including the open one. Removing it would make the
            // list shuffle under the pointer each time you pick from it;
            // marking it keeps the list stable and says where you are.
            <aside class="siblings" id="siblings">
                for other in &section.cards {
                    // Each row carries its own card's colour, so the
                    // highlight is that card's rather than one shared accent.
                    <a href={other.url(section)}
                       class={sibling_class(&other.url(section), path)}
                       style={tint_style(other)}>
                        <span class="swatch sm" style={swatch_style(other)}></span>
                        <span class="sibling-name">{other.name}</span>
                    </a>
                }
            </aside>
        </div>
    } else {
        // The page below found nothing either, and answered 404. A layout
        // cannot refuse, so it wraps that answer in nothing.
        <div class="split">{children}</div>
    }
}

/// The section and card this path names.
fn open(path: &str) -> Option<(&'static Section, &'static Card)> {
    let mut parts = path.split('/').skip(1);
    let section = section(parts.next()?)?;
    let card = section.card(parts.next()?)?;
    Some((section, card))
}

fn tab_class(href: &str, path: &str) -> &'static str {
    if path == href {
        "tab is-current"
    } else {
        "tab"
    }
}

fn sibling_class(href: &str, path: &str) -> &'static str {
    if path == href || path.starts_with(&format!("{href}/")) {
        "sibling is-current"
    } else {
        "sibling"
    }
}

fn swatch_style(card: &Card) -> String {
    format!("background:{}", card.colour())
}

fn tint_style(card: &Card) -> String {
    format!("--tint:{}", card.colour())
}

/// The tab strip, when this card layout is skipped.
///
/// Only the tabs. If this layout is shared then the card is the same one, so
/// the sibling list beside it cannot have changed and the header above it
/// cannot either -- the active panel is the only thing that moved.
pub fn Oob(path: &str) -> Element {
    if let Some((section, card)) = open(path) {
        <nav class="tabs" id="card-tabs" dp-swap-oob="outerHTML">
            for (slug, label) in TABS {
                <a href={card.tab_url(section, slug)}
                   class={tab_class(&card.tab_url(section, slug), path)}>
                    {label}
                </a>
            }
        </nav>
    }
}
