use crate::view::Chrome;

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
