// Ejected from brocanth-admin 0.1.0 by `brocanth admin eject userlinks`.
//
// Yours now: an upgrade of brocanth-admin does not change this file.
// After one, `brocanth admin diff` compares it with the shipped
// `userlinks.rsx`. Utility classes that the shipped stylesheet does not
// contain have no effect: add a stylesheet of your own for those
// (`AdminConfig::stylesheet`).

use brocanth_admin::view::Chrome;

// The links about the person and the site: Django's `userlinks` block. The
// admin has no authentication scheme, so it draws no "Log out": an
// application that has one replaces this block and keeps the layout.

/// Who is signed in, and the "View site" link.
pub fn Userlinks(ctx: &Chrome) -> Element {
    if let Some(user) = &ctx.user {
        <p class="m-0 px-6 text-sm text-ink-soft">Signed in as {user}</p>
    }
    // No `dp-boost`: the front page is not an admin screen, and a swap
    // into `dp-layout="admin"` would be wrong.
    if let Some(href) = &ctx.site_url {
        <a class="block rounded-control px-3 py-1.5 text-sm no-underline text-ink-soft hover:text-ink hover:bg-stage"
           href={href}>View site</a>
    }
}
