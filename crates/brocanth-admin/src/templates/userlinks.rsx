use crate::view::Chrome;

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
