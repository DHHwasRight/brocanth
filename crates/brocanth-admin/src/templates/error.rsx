use crate::view::ErrorCtx;

/// What a refusal or a missing record looks like.
pub fn ErrorPage(ctx: &ErrorCtx) -> Element {
    <h1>{&ctx.heading}</h1>
    <p class="muted">{&ctx.message}</p>
    <p><a class="btn" href={&ctx.chrome.base_path}>Back to {&ctx.chrome.title}</a></p>
}
