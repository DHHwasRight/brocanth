//! `/{section}` -- the grid of cards.
//!
//! Clicking one opens it, which replaces this grid with the split view: the
//! same layer, a different arrangement. That is why the board lives in the
//! layout above and not here.

use crate::pages::{html, views, Ctx, Error, IntoResponse};

pub fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let slug = ctx.require("section")?.to_string();
    let Some(section) = crate::data::section(&slug) else {
        return Err(Error::new(404, format!("no section `{slug}`")));
    };
    html!(ctx, views::_section_::index::Grid { section })
}
