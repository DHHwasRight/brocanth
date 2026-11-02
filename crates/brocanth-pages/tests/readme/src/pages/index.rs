//! `/`, because the file is called `index.rs`.

use crate::pages::{html, views, Ctx, Error, IntoResponse};

pub fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    html!(ctx, views::index::Index)
}
