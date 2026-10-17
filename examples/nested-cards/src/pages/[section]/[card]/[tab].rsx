use crate::data::Card;

/// One panel inside an expanded card.
pub fn Panel(card: &Card, tab: &str) -> Element {
    <div class="panel">
        <span class="panel-label">{tab}</span>
        for row in panel_rows(tab) {
            <span class={row}></span>
        }
        if tab == "Overview" {
            <div class="blocks">
                for _ in 0..3 {
                    <span class="block" style={block_style(card)}></span>
                }
            </div>
        }
    </div>
}

fn panel_rows(tab: &str) -> Vec<&'static str> {
    // Each panel draws a different shape so a swap is obvious at a glance.
    match tab {
        "Overview" => vec!["line", "line", "line short"],
        "Details" => vec!["line", "line", "line", "line", "line short"],
        _ => vec!["line short", "line", "line short", "line"],
    }
}

fn block_style(card: &Card) -> String {
    format!("background:{}", card.colour())
}
