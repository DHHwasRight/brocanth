//! `/{section}/{card}/{tab}` -- one panel, and the deepest page there is.
//!
//! Switching between panels replaces only this, because the three layouts
//! above are shared by every tab of one card.

use crate::pages::{html, views, Ctx, Error, IntoResponse};

pub fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let (section, card, tab) = (
        ctx.require("section")?.to_string(),
        ctx.require("card")?.to_string(),
        ctx.require("tab")?.to_string(),
    );
    let found = crate::data::section(&section)
        .and_then(|s| s.card(&card))
        .zip(crate::data::tab(&tab));
    let Some((card, label)) = found else {
        return Err(Error::new(404, format!("no panel `{section}/{card}/{tab}`")));
    };
    html!(ctx, views::_section_::_card_::_tab_::Panel { card, tab: label })
}
