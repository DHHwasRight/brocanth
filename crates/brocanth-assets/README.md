# brocanth-assets

Static files, embedded in the binary at compile time. One proc macro,
`assets!("assets", "public")`, reads two directories and writes a lookup
table into your crate. Files in the first directory are **fingerprinted**:
`css/site.css` is published at `/css/site.a1b2c3d4.css`, so a browser caches
it for a year and a new build changes the name. Files in the second
directory are served **verbatim**: `robots.txt`, `favicon.ico`, and any URL
a third party links to must keep its name. Use it for the stylesheets,
scripts, images and fonts a page references. Do not use it for files users
upload (that is `brocanth-storage`), or for files that change while the
process runs: the bytes are fixed at build time.

**Reason for embedding:** brocanth compiles its templates, its route table
and its layouts into the binary. A stylesheet read from the working
directory would be the one file that can go missing after `cargo build`
passes. Embedding also computes each fingerprint once, at build time.

## Getting started

### In a generated project

`brocanth-assets` is not a module of its own. `brocanth create` adds it when
you choose `rsx-tailwind` or `rsx-htmx`:

```sh
brocanth create my_app --with rsx,rsx-htmx
brocanth add rsx-tailwind
```

The generator writes `src/assets.rs` with the macro call, an `assets/` and
a `public/` directory, and a `serve_asset` function that `serve` checks
before the route table. That function answers `GET` and `HEAD`, sends the
`cache-control` and the strong `ETag`, and answers `304` to a matching
`if-none-match`. The crate has no settings, no features and no `scaffold`
module.

### By hand

```toml
[dependencies]
brocanth-assets = { git = "https://github.com/DHHwasRight/brocanth.git" }
```

```rust,ignore
// src/assets.rs
brocanth_assets::assets!("assets", "public");
```

`ignore`: this block expands against the directories of your crate, which
this crate does not have. The examples below run against
`tests/fixtures/`.

Both paths are relative to `CARGO_MANIFEST_DIR`. A missing directory is not
an error: a project with no `public/` has no favicon yet. The same logical
path in both directories is a compile error.

## What the macro writes

| Item | Meaning |
| --- | --- |
| `struct Asset` | `logical`, `published`, `body`, `content_type`, `integrity` (`sha384-…`), `etag` (the content hash), `fingerprinted` |
| `Asset::cache_control()` | `public, max-age=31536000, immutable` when fingerprinted, else `public, max-age=0, must-revalidate` |
| `static ASSETS` | every published file, sorted by logical path |
| `fn lookup(path)` | the file a browser asked for, by published path |
| `fn url(logical)` | the published path for the name the author wrote. Panics when no file has that name. |

## Examples

### Write a URL, serve a file

```rust
mod assets {
    brocanth_assets::assets!("tests/fixtures/assets", "tests/fixtures/public");
}

// In a template: never write the fingerprinted name by hand.
let href = assets::url("css/site.css");
assert!(href.starts_with("/css/site.") && href.ends_with(".css"));
assert_ne!(href, "/css/site.css");

// In the server: the browser asks for the published path.
let file = assets::lookup(href).unwrap();
assert_eq!(file.content_type, "text/css; charset=utf-8");
assert_eq!(file.cache_control(), "public, max-age=31536000, immutable");
assert!(file.integrity.starts_with("sha384-"));

// public/ keeps its name, and is revalidated with the ETag.
let robots = assets::lookup("/robots.txt").unwrap();
assert!(!robots.fingerprinted);
assert_eq!(robots.cache_control(), "public, max-age=0, must-revalidate");
```

In an `.rsx` template:

```text
<link rel="stylesheet" href={crate::assets::url("css/site.css")}>
```

### A name beginning with `_` is an input

A file or directory whose name begins with `_` (or `.`) is not published.
`_tailwind.css` is the input of the CSS build, and `_src/` holds the modules
a bundle imports. `brocanth-bundle` builds them into published files.

```rust
mod assets {
    brocanth_assets::assets!("tests/fixtures/assets", "tests/fixtures/public");
}

let published: Vec<&str> = assets::ASSETS.iter().map(|a| a.logical).collect();
assert_eq!(published, ["css/site.css", "robots.txt"]);
```

### Add a file

An edit to a file that is already embedded triggers a rebuild, because the
body is an `include_bytes!`. A **new** file does not, because nothing
references it yet. Run `touch src/assets.rs`. Until then, `url` panics with
that advice.

## Testing

```sh
cargo test -p brocanth-assets
```

No environment variable is needed. The unit tests check the fingerprint
position, the base64 of the `integrity` value and the content types. The
README examples above run as doctests against `tests/fixtures/`.

An application tests its static files through its own `serve_asset`, or
with `assets::lookup` as above. A page test checks the reference, not the
hash: with `brocanth-test`, find the `<link>` element with
`html.tags("link")` and compare its `href` to `assets::url("css/site.css")`.
(`assert_link_to` reads `<a>` elements only.) See
`crates/brocanth-test/README.md`.

## Links

- `crates/brocanth-bundle/README.md`: builds the `_` inputs into published
  files.
- `docs/llm-authoring.md`: how a page references an asset.
- `docs/static-export.md`: how `brocanth export` writes the files to disk.
