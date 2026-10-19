use crate::templates::parts;
use crate::view::ErrorCtx;

/// What a refusal or a missing record looks like.
pub fn ErrorPage(ctx: &ErrorCtx) -> Element {
    <h1 class={parts::heading()}>{&ctx.heading}</h1>
    <p class="mt-2 mb-5 text-sm text-ink-soft">{&ctx.message}</p>
    <a class={parts::button()} href={&ctx.chrome.base_path} dp-boost>
        Back to {&ctx.chrome.title}
    </a>
}
