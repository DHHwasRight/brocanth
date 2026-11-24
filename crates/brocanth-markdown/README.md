# brocanth-markdown

`brocanth-markdown` turns Markdown into HTML that a page can show as
markup. It parses CommonMark with the GFM tables, footnotes,
strikethrough and task lists (`pulldown-cmark`), gives each heading an id,
and cleans the result with `brocanth-sanitize`'s allow-list. It also reads
TOML front matter into a serde type.

## Getting started

In a generated project:

```text
brocanth create my-blog --with rsx,markdown
brocanth add markdown
```

## Render

```rust
let html = brocanth_markdown::render("## Hello\n\n[x](javascript:alert(1)) *world*");
assert_eq!(
    html.as_str(),
    "<h2 id=\"hello\">Hello</h2>\n<p><a rel=\"noopener noreferrer\">x</a> <em>world</em></p>\n"
);
```

`render` is for text the site's authors wrote. `render_user` is for text a
visitor wrote: every id starts with `user-content-`, and links get
`rel="nofollow ugc noopener noreferrer"`. Both return a `SafeHtml`; with the
`rsx` feature a template writes it with `{! body }`.

## Front matter

```rust
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Post {
    title: String,
}

let doc = brocanth_markdown::front_matter::<Post>("+++\ntitle = \"Hi\"\n+++\nBody\n").unwrap();
assert_eq!(doc.data.title, "Hi");
assert_eq!(doc.body, "Body\n");
```

An error names the line and ends with a `help:` line.

## Features

- `render`: the parser and the sanitizer.
- `front-matter`: `serde` and `toml` only, for a build script.
- `runtime` (default): both.
- `rsx`: `SafeHtml` as an rsx template.

See `docs/modules/markdown.md` for the decisions.
