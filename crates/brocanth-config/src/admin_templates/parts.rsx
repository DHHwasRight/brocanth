// The class strings the screen uses.
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

/// A key's dotted path. Monospaced, because the thing a reader does with it
/// is copy it.
pub fn path() -> &'static str {
    "font-mono text-xs text-ink [overflow-wrap:anywhere]"
}

/// A value that had something taken out of it.
pub fn redacted() -> &'static str {
    "font-mono text-xs text-ink-faint italic"
}

/// A value that is printed in full.
pub fn literal() -> &'static str {
    "font-mono text-xs text-ink [overflow-wrap:anywhere]"
}

/// The origin column: a file and a line, a `$VARIABLE`, or a plugin.
pub fn origin() -> &'static str {
    "font-mono text-xs text-ink-soft [overflow-wrap:anywhere]"
}

/// A `was ...` line, under the value it lost to.
pub fn was() -> &'static str {
    "block mt-1 font-mono text-xs text-ink-faint [overflow-wrap:anywhere]"
}
