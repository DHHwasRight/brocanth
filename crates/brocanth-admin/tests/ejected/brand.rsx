// Ejected from brocanth-admin 0.1.0 by `brocanth admin eject brand`.
//
// Yours now: an upgrade of brocanth-admin does not change this file.
// After one, `brocanth admin diff` compares it with the shipped
// `brand.rsx`. Utility classes that the shipped stylesheet does not
// contain have no effect: add a stylesheet of your own for those
// (`AdminConfig::stylesheet`).

use brocanth_admin::view::Chrome;

// The top of the sidebar: Django's `branding` block. A block and not part of
// the layout, so an application puts its logo here and keeps the layout.
// The layout receives it rendered, in `LayoutCtx::brand`.

/// The header, linked to the index.
pub fn Brand(ctx: &Chrome) -> Element {
    <a class="px-6 text-[15px] font-semibold text-ink no-underline"
       href={&ctx.base_path} dp-boost>
        {&ctx.header}
    </a>
}
