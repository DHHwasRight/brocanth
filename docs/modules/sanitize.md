# `sanitize`

An allow-list HTML sanitizer and a SafeHtml type; links html5ever.

The wizard calls it: *Show HTML that users write, cleaned*.

| | |
|---|---|
| Categories | **Renderer** |
| Wizard question | Pages |
| Default | off |
| Presets | none |
| Part of | nothing |
| Needs | none |
| After | none |
| Adapts to | none |
| Conflicts | none |
| Outside the runtime | yes |
| Crates | `brocanth-sanitize` (feature `rsx` with [`rsx`](rsx.md)) |
| Without it | no sanitizer; HTML a user wrote can only be shown escaped, as text |

## What it writes

Slots:

- AGENTS.md: `## HTML from users`

The module writes no code. A page calls the sanitizer with the policy that
page needs, so a generated call site would be a guess about a page that does
not exist yet. With `rsx`, the crate arrives with its `rsx` feature, and a
component takes a `SafeHtml` as a child.

`src/modules/sanitize.rs` holds `..Module::DEFAULT` alone: the module has no
hook, no integration and no setting. A policy is code, chosen per page.

## What the crate gives

| Item | Does |
|---|---|
| `comment(&str)` | links, emphasis, lists, quotes, code; a link gets `rel="nofollow ugc noopener noreferrer"` |
| `rich_text(&str)` | a comment's tags, and `h2`–`h4`, images, tables, figures |
| `text(&str)` | no tags; the text stays, and `<script>` and `<style>` go with their content |
| `Policy` | an allow-list built once in a `static`: `allow_tags`, `allow_attributes`, `deny_tags`, `link_rel`, `url_schemes` |
| `SafeHtml` | what a policy returns; no `From<String>`, no `Deserialize` |

Every policy drops `style`, `class`, `id` and every `on*` attribute, and keeps
a URL only when it is relative or `http`, `https` or `mailto`.
`tests/xss.rs` runs a list of known vectors through every policy and parses
the output again to check that no script, handler or bad scheme is left.

**Why ammonia.** It parses with `html5ever`, the WHATWG algorithm the browsers
use, so the output parses into the tree that was checked. A sanitizer with its
own parser is open to mutation XSS. The parser is the cost, so the sanitizer
is a crate of its own.

## Read more

- [brocanth-sanitize](../../crates/brocanth-sanitize/README.md)
- Source: [`crates/brocanth-sanitize/src/policy.rs`](../../crates/brocanth-sanitize/src/policy.rs), [`crates/brocanth-sanitize/src/scaffold.rs`](../../crates/brocanth-sanitize/src/scaffold.rs)
