# brocanth-meta

What a page says about itself: its title, description, canonical URL, Open
Graph and Twitter tags, and the same page in other languages (`hreflang`).
The site's values come from settings; a page sets only what differs. Use it
in a project that serves pages people share or search engines read. With
`runtime` (the default) the crate links the core and nothing else: the tags
are string building.

## Getting started

```sh
brocanth create blog --with rsx,meta
brocanth add meta
```

The module writes `src/plugins/meta/mod.rs` (the `[meta]` settings and the
plugin), `ctx.meta()` and `ctx.head()` in `src/pages/mod.rs`, and, with
`rsx`, a root layout that writes the head. See `docs/modules/meta.md`.

By hand:

```toml
[dependencies]
brocanth-meta = { git = "https://github.com/DHHwasRight/brocanth.git" }
```

## Three types

`Site` is what every page shares. `Meta` is what one page declares. `Head`
is the two resolved, which the root layout writes.

```rust
use brocanth_meta::{Head, Meta, Site};

let site = Site::new("My Blog")
    .title_template("%s | My Blog")
    .base_url("https://blog.example")
    .description("Notes on Rust.");
assert!(site.check().is_ok());

let mut page = Meta::default();
page.title("About").image("/about.png");

let head = Head::new(&site, Some(&page)).path("/about");
assert_eq!(head.title(), "About | My Blog");

let tags = head.to_string();
assert!(tags.contains(r#"<link rel="canonical" href="https://blog.example/about">"#));
assert!(tags.contains(r#"<meta name="description" content="Notes on Rust.">"#));
assert!(tags.contains(r#"<meta property="og:image" content="https://blog.example/about.png">"#));
assert!(tags.contains(r#"<meta name="twitter:card" content="summary_large_image">"#));
```

A page that declares nothing gets the site's name as its title. Without a
base URL there is no canonical URL and no `og:url`: a relative canonical
URL is worse than none.

```rust
use brocanth_meta::{Head, Site};

let head = Head::new(&Site::new("My Blog"), None).path("/about");
assert_eq!(head.title(), "My Blog");
assert_eq!(head.canonical(), None);
```

## In a page

A generated project sets a page's values on the context before it renders.
The root layout declares `meta: &Head`, and `html!` fills it with
`ctx.head()`:

```rust,ignore
// src/pages/about.rs
ctx.meta().title("About").description("Who writes this blog.");
html!(ctx, views::about::About {})

// src/pages/layout.rsx
pub fn Layout(meta: &Head, children: Element) -> Element {
    <html lang={meta.lang_tag()}>
        <head>
            <title>{meta.title()}</title>
            {! meta }
        </head>
        <body>{children}</body>
    </html>
}
```

`{! meta }` writes the tags unescaped because they are markup. Every value
inside them is escaped:

```rust
use brocanth_meta::{Head, Meta, Site};

let mut page = Meta::default();
page.title(r#""><script>alert(1)</script>"#);
let tags = Head::new(&Site::new("x"), Some(&page)).to_string();
assert!(!tags.contains("<script>"));
```

A JSON answer takes `Head::to_json`, which a `.jt` template writes with
`jt::Raw`.

## Languages

With `i18n`, `<html lang>` and `og:locale` follow the request's locale.
With `i18n-routing`, each page also lists itself in every other locale,
and its canonical URL keeps the locale segment:

```rust
use brocanth_meta::{Head, Site};

let head = Head::new(&Site::new("x").base_url("https://x.example"), None)
    .path("/fr/about")
    .lang("fr")
    .alternate("en", "/en/about")
    .alternate("x-default", "/about");
let tags = head.to_string();
assert!(tags.contains(r#"<link rel="alternate" hreflang="en" href="https://x.example/en/about">"#));
assert!(tags.contains(r#"<link rel="canonical" href="https://x.example/fr/about">"#));
```

## Settings

| Setting | Default | Meaning |
|---|---|---|
| `name` | the project's title | The title of a page with none, and `og:site_name`. |
| `title_template` | `"%s \| <title>"` | How a page's title becomes the document's. Must contain `%s`. |
| `base_url` | none | Scheme and host. A canonical URL and `og:url` need it. |
| `description` | none | The description of a page that gives none. |
| `image` | none | The `og:image` of a page that gives none. |
| `twitter` | none | `twitter:site`. |

A template with no `%s` or a base URL with no scheme stops the start, with
a `help:` line.

## Features

| Feature | What it adds |
|---|---|
| `runtime` (default) | `Site`, `Meta`, `Head`; `Site` is a context item |
| `scaffold` | the `meta` module `brocanth create` offers |
