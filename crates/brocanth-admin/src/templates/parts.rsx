// The class strings every screen shares.
//
// Functions and not a `@layer components` block in the stylesheet, because
// a component class hides which utilities are in play and the next person
// to restyle has to open two files. These are in a `.rsx` file so Tailwind
// sees them: `@source` scans templates.
//
// Nothing here renders. It is the vocabulary the six screens are written in,
// and the one place to change how a button or a table looks.

/// A page's `<h1>`.
pub fn heading() -> &'static str {
    "text-[22px] font-semibold tracking-[-0.01em] text-ink m-0"
}

/// The `Admin / Posts / On the Analytical Engine` line above it.
pub fn crumbs() -> &'static str {
    "text-xs text-ink-faint mb-3.5 flex items-center gap-1.5"
}

/// A white panel on the stage.
pub fn card() -> &'static str {
    "bg-white border border-line rounded-card overflow-hidden shadow-card"
}

/// The row of controls under a heading.
pub fn bar() -> &'static str {
    "flex items-center gap-3 flex-wrap mb-4"
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

/// What an empty table says, in the space the table would have filled.
pub fn empty() -> &'static str {
    "px-7 py-10 text-center text-ink-soft bg-white"
}

pub fn muted() -> &'static str {
    "text-ink-soft"
}

pub fn mono() -> &'static str {
    "font-mono text-xs"
}

/// A link that is not a button and not body text: a row action.
pub fn action() -> &'static str {
    "text-accent no-underline hover:underline"
}

/// The three buttons. One base, three skins, so they cannot drift apart.
pub fn button() -> &'static str {
    "inline-flex items-center justify-center rounded-control border px-3 py-1.5 \
     text-sm font-medium no-underline cursor-pointer border-line bg-white \
     text-ink hover:bg-stage"
}

pub fn button_primary() -> &'static str {
    "inline-flex items-center justify-center rounded-control border px-3 py-1.5 \
     text-sm font-medium no-underline cursor-pointer border-accent bg-accent \
     text-white hover:bg-accent-dark shadow-card"
}

pub fn button_danger() -> &'static str {
    "inline-flex items-center justify-center rounded-control border px-3 py-1.5 \
     text-sm font-medium no-underline cursor-pointer border-danger bg-danger \
     text-white hover:brightness-110"
}

/// A text input, a select and a textarea all look the same.
pub fn control() -> &'static str {
    "w-full rounded-control border border-line bg-white px-2.5 py-2 text-sm \
     text-ink focus:outline-none focus:border-accent focus:ring-2 focus:ring-accent/20"
}

pub fn control_error() -> &'static str {
    "w-full rounded-control border border-danger bg-white px-2.5 py-2 text-sm \
     text-ink focus:outline-none focus:ring-2 focus:ring-danger/20"
}
