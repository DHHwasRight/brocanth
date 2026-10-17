use crate::data::{Card, Section};

/// The grid of cards for a section.
pub fn Grid(section: &Section) -> Element {
    <div class="grid">
        for card in &section.cards {
            <a class="card" href={card.url(section)} style={card_style(card)}>
                <span class="card-top"></span>
                <span class="card-title">{card.name}</span>
                for _ in 0..card.lines {
                    <span class="line"></span>
                }
                <span class="line short"></span>
            </a>
        }
    </div>
}

fn card_style(card: &Card) -> String {
    format!("--tint:{}", card.colour())
}
