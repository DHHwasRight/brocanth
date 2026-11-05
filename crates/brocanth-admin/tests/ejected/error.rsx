// Ejected from brocanth-admin 0.1.0 by `brocanth admin eject error`.
//
// Yours now: an upgrade of brocanth-admin does not change this file.
// After one, `brocanth admin diff` compares it with the shipped
// `error.rsx`. Utility classes that the shipped stylesheet does not
// contain have no effect: add a stylesheet of your own for those
// (`AdminConfig::stylesheet`).

use brocanth_admin::parts;
use brocanth_admin::view::ErrorCtx;

/// What a refusal or a missing record looks like.
pub fn ErrorPage(ctx: &ErrorCtx) -> Element {
    <h1 class={parts::heading()}>{&ctx.heading}</h1>
    <p class="mt-2 mb-5 text-sm text-ink-soft">{&ctx.message}</p>
    <a class={parts::button()} href={&ctx.chrome.base_path} dp-boost>
        Back to {&ctx.chrome.title}
    </a>
}
