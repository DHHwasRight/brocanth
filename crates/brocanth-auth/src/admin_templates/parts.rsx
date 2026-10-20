// The class strings the two screens share.
//
// The same vocabulary as the admin's `parts.rsx` and as `brocanth-jobs`'s,
// and a separate copy for the same reason the tokens are: this crate cannot
// reach into another crate's templates, and the dependency that would let it
// is optional. If the admin's tables change shape, change these.

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

pub fn heading() -> &'static str {
    "text-[22px] font-semibold tracking-[-0.01em] text-ink m-0 mb-4"
}

/// A yes/no cell. Two words and not a tick, because a tick and a blank are
/// the same cell to a screen reader that announces neither.
pub fn yes() -> &'static str {
    "text-ink"
}

pub fn no() -> &'static str {
    "text-ink-faint"
}

/// The key half of the definition list on an account's screen.
pub fn term() -> &'static str {
    "text-[11px] font-semibold uppercase tracking-wider text-ink-faint"
}

/// The value half.
pub fn definition() -> &'static str {
    "mt-1 mb-4 text-sm text-ink [overflow-wrap:anywhere]"
}

/// A button that changes something, at the top of a card rather than in a
/// row. Red, because ending every session signs somebody out of everything.
pub fn danger_button() -> &'static str {
    "bg-white border border-danger rounded-control px-3 py-1.5 text-sm text-danger \
     cursor-pointer hover:bg-danger hover:text-white"
}
