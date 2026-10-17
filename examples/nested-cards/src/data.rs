//! The site's content, which is deliberately nothing: coloured rectangles and
//! placeholder lines. The point of this example is the navigation, so anything
//! readable would only distract from which part of the page was replaced.
//!
//! Built once and leaked into a `OnceLock`, because every request reads it and
//! none of it changes. A real application would reach for the model layer
//! here, and the shape of the handlers would not move.

use std::sync::OnceLock;

/// The five-colour palette every rectangle is drawn from.
pub const PALETTE: [&str; 5] = ["#cdb4db", "#ffc8dd", "#ffafcc", "#bde0fe", "#a2d2ff"];

/// Panels inside an expanded card. These are the deepest level, and switching
/// between them replaces the least of the page.
pub const TABS: [(&str, &str); 3] = [
    ("overview", "Overview"),
    ("details", "Details"),
    ("activity", "Activity"),
];

pub struct Section {
    pub slug: &'static str,
    pub name: &'static str,
    pub cards: Vec<Card>,
}

pub struct Card {
    pub slug: String,
    pub name: String,
    /// Index into [`PALETTE`].
    pub tint: usize,
    /// How many placeholder lines the summary draws, so the cards differ.
    pub lines: usize,
}

/// Every section, built on first use.
pub fn sections() -> &'static [Section] {
    static SECTIONS: OnceLock<Vec<Section>> = OnceLock::new();
    SECTIONS.get_or_init(|| {
        [
            ("alpha", "Alpha"),
            ("beta", "Beta"),
            ("gamma", "Gamma"),
            ("delta", "Delta"),
        ]
        .into_iter()
        .enumerate()
        .map(|(s, (slug, name))| Section {
            slug,
            name,
            cards: (1..=6)
                .map(|c| Card {
                    slug: format!("card-{c}"),
                    name: format!("{name} {c}"),
                    // Offset per section so neighbouring sections do not
                    // repeat the same sequence of colours.
                    tint: (s * 2 + c) % PALETTE.len(),
                    lines: 2 + (c % 3),
                })
                .collect(),
        })
        .collect()
    })
}

/// The section a URL names, or `None` for a slug nothing matches.
///
/// The router matched `{section}` against anything, so this is where a
/// request for `/nope` becomes a 404 rather than a panic.
pub fn section(slug: &str) -> Option<&'static Section> {
    sections().iter().find(|s| s.slug == slug)
}

/// The label of a tab slug, which is also the test that it exists.
pub fn tab(slug: &str) -> Option<&'static str> {
    TABS.iter().find(|(s, _)| *s == slug).map(|(_, label)| *label)
}

impl Section {
    pub fn url(&self) -> String {
        format!("/{}", self.slug)
    }

    pub fn card(&self, slug: &str) -> Option<&Card> {
        self.cards.iter().find(|c| c.slug == slug)
    }
}

impl Card {
    pub fn url(&self, section: &Section) -> String {
        format!("/{}/{}", section.slug, self.slug)
    }
    pub fn tab_url(&self, section: &Section, tab: &str) -> String {
        format!("/{}/{}/{}", section.slug, self.slug, tab)
    }
    pub fn colour(&self) -> &'static str {
        PALETTE[self.tint]
    }
}
