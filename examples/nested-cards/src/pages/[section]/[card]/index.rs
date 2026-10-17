//! `/{section}/{card}` -- opening a card lands on its first panel.

use crate::pages::{Ctx, Error, IntoResponse, Response};

pub fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let first = crate::data::TABS[0].0;
    let to = format!("{}/{first}", ctx.path);
    Ok(Response::empty(303).with_header("location", to))
}
