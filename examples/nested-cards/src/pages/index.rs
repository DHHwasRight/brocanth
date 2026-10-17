//! `/` -- the front door, which is the first section.
//!
//! There is nothing else to show at the root: the shell is all layout, and a
//! page that said "pick a section" would be a page nobody reads twice.

use crate::pages::{Ctx, Error, IntoResponse, Response};

pub fn handle(_ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let first = crate::data::sections()[0].url();
    Ok(Response::empty(303).with_header("location", first))
}
