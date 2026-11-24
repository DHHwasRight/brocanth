# `markdown`

Markdown to cleaned HTML: GFM tables and footnotes, heading ids, TOML front matter; links html5ever.

The wizard calls it: *Render Markdown to HTML, cleaned*.

| | |
|---|---|
| Categories | **Renderer** |
| Wizard question | Pages |
| Default | off |
| Presets | none |
| Part of | nothing |
| Conflicts | none |
| Outside the runtime | yes |
| Crates | `brocanth-markdown` (feature `rsx` with [`rsx`](rsx.md)) |
| Without it | no Markdown; a page shows Markdown as text, or links a parser and a sanitizer by hand |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/markdown.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | [`rsx`](rsx.md): the result is an rsx template, so a page writes it with `{! body }` |
| Contributes | `AgentsSection` `markdown`: how Markdown becomes HTML on a page |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | none |
<!-- end generated: capabilities -->

## What it writes

Slots:

- AGENTS.md: `## Markdown`

The module writes no code, as [`sanitize`](sanitize.md) writes none. A page
calls the renderer where it shows Markdown, so a generated call site would be
a guess about a page that does not exist yet. [`content`](content.md) is the
module that renders files with it. With `rsx`, the crate arrives with its
`rsx` feature, and a template writes the result with `{! body }`.

## What the crate gives

| Item | Does |
|---|---|
| `render(&str)` | text the site's authors wrote: heading ids, links with `rel="noopener noreferrer"` |
| `render_user(&str)` | text a visitor wrote: every id starts with `user-content-`, a `#name` link follows it, links get `rel="nofollow ugc noopener noreferrer"`, no `h1` |
| `Markdown` | a renderer built once: `authored()`, `user()`, `without_heading_ids()`, `highlight(f)` |
| `front_matter::<T>(&str)` | the TOML between two `+++` lines read into `T`, and the body after it |
| `FrontMatterError` | the line, what is wrong, and a `help:` line |
| `SafeHtml` | re-exported from `brocanth-sanitize`: what `render` returns |

Features: `render` (the parser and the sanitizer), `front-matter` (`serde`
and `toml` only, for a build script), `rsx`. `runtime`, on by default, is the
first two.

## Decisions

**pulldown-cmark, not comrak.** pulldown-cmark implements CommonMark and the
GFM tables, footnotes, strikethrough and task lists. rustdoc and mdBook render
with it. It is MIT and pulls three small crates. comrak covers more of GFM
(autolinks without `<>`), but it is BSD-2-Clause and larger, and this
workspace takes MIT or Apache.

**Every result goes through the sanitizer.** Markdown passes raw HTML through,
and a link may carry `javascript:`. The policy is `brocanth-sanitize`'s rule
set (no `script`, no `on*`, `http`, `https` and `mailto` only), plus the
attributes Markdown writes, each held to the shape Markdown writes it: a code
block's `class` is `language-<name>`, a footnote's class is one of three, a
table cell's `style` is `text-align` and nothing else, an `input` is a
checkbox. `tests/render.rs` runs a list of vectors through both presets.

**Heading ids from the text.** `## Getting started` is `id="getting-started"`,
and a second one is `getting-started-1`, so `#anchor` links work. In text a
visitor wrote, an id can shadow a global name a script reads (DOM clobbering),
so `render_user` prefixes every id with `user-content-` and rewrites `#name`
links to match, as GitHub does.

**No syntax highlighting by default.** A highlighter is a grammar per language
(megabytes for `syntect`) and writes either inline `style`, which no policy
keeps, or classes that need a stylesheet. A code block keeps
`class="language-rust"`, which a client highlighter reads.
`Markdown::highlight(f)` plugs in a server-side one: the policy then keeps
`<span class>` where each class starts with `hl-`.

**TOML front matter, not YAML.** `serde_yaml` was deprecated by its author in
March 2024, and its forks publish rarely. `toml` is maintained, already in the
workspace, and has a date type, so `date = 2026-11-22` is a date. Hugo and Zola
write TOML front matter between `+++` lines. A file that starts with `---` is
refused with the TOML spelling in its `help:` line.

## Read more

- [brocanth-markdown](../../crates/brocanth-markdown/README.md)
- Source: [`crates/brocanth-markdown/src/render.rs`](../../crates/brocanth-markdown/src/render.rs), [`crates/brocanth-markdown/src/front.rs`](../../crates/brocanth-markdown/src/front.rs), [`crates/brocanth-markdown/src/scaffold.rs`](../../crates/brocanth-markdown/src/scaffold.rs)
