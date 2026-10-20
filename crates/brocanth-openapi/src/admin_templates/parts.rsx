// The class strings the two screens share, and the stylesheet they carry.
//
// The same vocabulary as the admin's own `parts.rsx`, and a separate copy
// for the same reason the tokens are: this crate cannot reach into another
// crate's templates, and the dependency that would let it is optional. If
// the admin's tables change shape, change these.

// The panel's sheet, inline rather than served: the admin answers with a
// page and mounts no asset route, and a panel is in no position to add
// one. No preflight — the admin has already reset the document.
pub fn Style() -> Element {
    <style>{! crate::admin::STYLE }</style>
}

pub fn heading() -> &'static str {
    "text-[22px] font-semibold tracking-[-0.01em] text-ink m-0 mb-1"
}

pub fn crumbs() -> &'static str {
    "text-xs text-ink-faint mb-3.5 flex items-center gap-1.5"
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

pub fn action() -> &'static str {
    "text-accent no-underline hover:underline"
}

pub fn mono() -> &'static str {
    "font-mono text-xs"
}

/// A verb, as a fixed-width badge, so a column of them lines up and a
/// reader can scan for DELETE without reading any of the paths.
///
/// One colour per class of effect rather than one per verb: reading is
/// grey, writing is the accent, removing is the alarm. A rainbow of seven
/// teaches nothing, and the two that matter when you are skimming are
/// "does this change anything" and "does this destroy anything".
pub fn verb_badge(verb: &str) -> &'static str {
    // Written out three times rather than concatenated, because Tailwind
    // scans this file for whole class names and never sees a string it
    // would have to join.
    match verb {
        "POST" | "PUT" | "PATCH" => {
            "inline-block w-[4.5rem] text-center rounded-control px-1.5 py-0.5 \
             font-mono text-[11px] font-semibold tracking-wide bg-accent/10 text-accent"
        }
        "DELETE" => {
            "inline-block w-[4.5rem] text-center rounded-control px-1.5 py-0.5 \
             font-mono text-[11px] font-semibold tracking-wide bg-danger/10 text-danger"
        }
        _ => {
            "inline-block w-[4.5rem] text-center rounded-control px-1.5 py-0.5 \
             font-mono text-[11px] font-semibold tracking-wide bg-stage text-ink-soft"
        }
    }
}

/// What a document marked deprecated looks like beside its path.
pub fn deprecated_badge() -> &'static str {
    "ml-2 inline-block rounded-control bg-danger/10 px-1.5 py-0.5 text-[11px] \
     font-semibold uppercase tracking-wider text-danger"
}
