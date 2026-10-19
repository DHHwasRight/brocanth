// The class strings the five screens share.
//
// The same vocabulary as the admin's `parts.rsx`, and a separate copy for
// the same reason the tokens are: this crate cannot reach into another
// crate's templates, and the dependency that would let it is optional.
// If the admin's tables change shape, change these.

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

/// A button inside a table cell. A form, so it posts; styled as a link,
/// because a row full of buttons reads as a toolbar and this is a row.
pub fn row_button() -> &'static str {
    "bg-transparent border-0 p-0 font-inherit text-sm text-accent cursor-pointer hover:underline"
}

pub fn row_button_danger() -> &'static str {
    "bg-transparent border-0 p-0 font-inherit text-sm text-danger cursor-pointer hover:underline"
}
