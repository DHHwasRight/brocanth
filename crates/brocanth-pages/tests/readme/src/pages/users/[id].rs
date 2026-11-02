//! `/users/{id}`: the brackets in the file name are the parameter.

use crate::pages::{html, routes, views, Ctx, Error, IntoResponse, Response};

/// GET. Two layouts wrap the page: `layout.rsx` and `users/layout.rsx`.
pub fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let id: u64 = ctx.require("id")?.parse()?;
    html!(ctx, views::users::_id_::User { id })
}

/// POST to the same path. One file, one handler per verb.
pub fn handle_post(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let id: u64 = ctx.require("id")?.parse()?;
    Ok(Response {
        status: 303,
        body: routes::users::id::path(id),
    })
}
