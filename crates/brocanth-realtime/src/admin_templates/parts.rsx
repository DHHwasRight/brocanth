// The class strings this panel's screen is written in.
//
// The same vocabulary as the admin's own `parts.rsx`, and a separate copy
// for the same reason the tokens are: this crate cannot reach into another
// crate's templates, and the dependency that would let it is optional. If
// the admin's tables change shape, change these.

pub fn heading() -> &'static str {
    "text-[22px] font-semibold tracking-[-0.01em] text-ink m-0 mb-4"
}

pub fn subheading() -> &'static str {
    "mt-7 mb-2 text-base font-semibold text-ink"
}

pub fn card() -> &'static str {
    "bg-white border border-line rounded-card overflow-hidden shadow-card"
}

pub fn table() -> &'static str {
    "w-full border-collapse text-sm"
}

pub fn th() -> &'static str {
    "text-left px-3.5 py-2.5 bg-stage/70 text-[11px] font-semibold uppercase \
     tracking-wider text-ink-faint border-b border-line"
}

pub fn td() -> &'static str {
    "text-left px-3.5 py-2.5 border-b border-line align-top text-ink"
}

pub fn td_muted() -> &'static str {
    "text-left px-3.5 py-2.5 border-b border-line align-top text-ink-soft"
}

pub fn empty() -> &'static str {
    "px-7 py-10 text-center text-ink-soft bg-white"
}

/// The card the split sits in: a card with the accent down its left edge,
/// which is how the admin marks the thing on a screen that is the point of
/// the screen.
pub fn verdict_card() -> &'static str {
    "bg-white border border-line rounded-card border-l-4 border-l-accent p-5 \
     shadow-card text-ink"
}

/// A number big enough to be read from across the room, and its label.
pub fn figure() -> &'static str {
    "block text-[40px] font-semibold leading-none tabular-nums"
}

pub fn figure_label() -> &'static str {
    "block mt-1.5 text-[11px] font-medium uppercase tracking-wider text-ink-faint"
}
