# brocanth-sanitize

`brocanth-sanitize` cleans HTML that a user wrote, so a page can show it
as markup. A `Policy` is an allow-list of tags, attributes and URL
schemes; what is not on it goes. A policy returns a `SafeHtml`, and a
template writes that type as markup.

Use it for comments, rich text from an editor, and Markdown rendered to
HTML. Do not use it for HTML your own code builds: a template escapes that
already.

## Getting started

In a generated project:

```text
brocanth create my-blog --with rsx,sanitize
brocanth add sanitize
```

By hand:

```toml
[dependencies]
brocanth-sanitize = { git = "https://github.com/DHHwasRight/brocanth.git", features = ["rsx"] }
```

## A policy per use

```rust
use brocanth_sanitize::{comment, rich_text, text};

let body = comment(r#"<p onclick="steal()">Nice <a href="javascript:alert(1)">post</a></p><script>alert(1)</script>"#);
assert_eq!(
    body.as_str(),
    r#"<p>Nice <a rel="nofollow ugc noopener noreferrer">post</a></p>"#
);

let article = rich_text(r#"<h2>Results</h2><img src="https://example.com/a.png" alt="A" onerror="x()">"#);
assert_eq!(
    article.as_str(),
    r#"<h2>Results</h2><img src="https://example.com/a.png" alt="A">"#
);

assert_eq!(text("<b>Bold</b> name").as_str(), "Bold name");
```

| Function | Keeps |
|---|---|
| `comment` | `a` (`href`, `title`), `b`, `blockquote`, `br`, `code`, `del`, `em`, `i`, `li`, `ol`, `p`, `pre`, `s`, `strong`, `ul`; links get `rel="nofollow ugc noopener noreferrer"` |
| `rich_text` | all of `comment`, and `h2`-`h4`, `hr`, `img`, `figure`, `figcaption`, tables, `abbr`, `cite`, `mark`, `q`, `sub`, `sup`; links get `rel="noopener noreferrer"` |
| `text` | no tags; the text stays |

Every policy drops `<script>` and `<style>` with their content, every
`on*`, `style`, `class` and `id` attribute, comments, and every URL that is
not relative, `http`, `https` or `mailto`.

For another use, start from one and change it. Build it once:

```rust
use std::sync::LazyLock;
use brocanth_sanitize::Policy;

static REVIEW: LazyLock<Policy> = LazyLock::new(|| {
    Policy::comment()
        .deny_tags(["a"])
        .allow_tags(["h3"])
});

assert_eq!(
    REVIEW.clean(r#"<h3>Good</h3><a href="https://x.example">spam</a>"#).as_str(),
    "<h3>Good</h3>spam"
);
```

## In a template

`SafeHtml` prints its markup. In rsx, write it with `{! }`:

```text
<article>{! brocanth_sanitize::comment(&row.body) }</article>
```

With the `rsx` feature, `SafeHtml` is also an `rsx::Template`, so a
component takes it as a child.

In `{ value }` the markup is escaped again: the tags show as text and
nothing runs. The wrong place is ugly, not dangerous.

A `SafeHtml` comes only from a policy, from `SafeHtml::escape(text)`, or
from `SafeHtml::from_static("<br>")` for a literal in the source. It has
no `From<String>` and no `Deserialize`.

```rust
use brocanth_sanitize::SafeHtml;

assert_eq!(SafeHtml::escape("<3 & \"you\"").as_str(), "&lt;3 &amp; &quot;you&quot;");
```

## Store the source, clean on render

Keep what the user wrote in the database, and clean it when the page
renders. Then a policy that gets stricter covers the old rows as well, and
the editor gets back what was typed.

## Why ammonia

`ammonia` parses HTML with `html5ever`, the WHATWG parsing algorithm the
browsers use, keeps only allowed nodes, and writes the tree again. A
sanitizer with its own parser is wrong where the browser parses
differently, and that is where mutation XSS lives (`<noscript>`, `<svg>`,
`<math>`). The cost is the parser, so it is a crate of its own.

## Tests

`tests/xss.rs` runs known XSS vectors through every policy and checks the
output tag by tag. `tests/scaffold.rs` generates the module into a project
and compares it with `tests/golden/sanitize.txt`.

## More

- `docs/modules/sanitize.md`: the manifest.
