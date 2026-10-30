# Static export

`brocanth-static` writes a brocanth application to a directory of files
that any static host serves: GitHub Pages, an S3 bucket, nginx. The
`static` module's generated `export` command calls it. An application
that was not generated calls it directly.

## Who depends on it

- **Generated projects.** `brocanth create --with static` writes
  `src/commands/export.rs`, a thin caller of the crate. The exact text is
  in `crates/brocanth-cli/tests/golden/static.txt`.
- **The d3set chart gallery** ([github.com/DHHwasRight/d3set](https://github.com/DHHwasRight/d3set),
  `rust/gallery/src/export.rs`). It exports under the base path `/d3set`
  with two layout layers and recorded feeds as JSON, and publishes the
  result to GitHub Pages. `crates/brocanth-static/tests/d3set.rs` mirrors
  what it needs. If that test must change, the gallery probably must
  change too: say so in the commit.

## The API

```rust
let report = brocanth_static::Export::new("dist")
    .base("/d3set")                 // the path the site is served under
    .partials(2)                    // one partial tree per depth, 1..=2
    .routes(&app.router)            // every GET route with no parameter
    .paths(["/charts/bar", "/data/sales.json"]) // the rest, by name
    .assets(assets)                 // (published path, bytes)
    .run(|request| handle(&app, request))
    .await?;
eprintln!("export: {report}");
```

`run` takes the application's own handler: an `async Fn(Request) ->
Response`. The export therefore sees what a browser sees, hooks included.
It does not start a runtime. The caller drives the future.

## What it writes

| Request | File |
|---|---|
| `GET /` | `index.html` |
| `GET /about` (HTML) | `about/index.html` |
| `GET /data/sales.json` (last segment has a dot) | `data/sales.json` |
| `GET /about` with `DP-Partial: 2` | `partials/2/about/index.html` |
| `GET /__export_missing__`, when it answers 404 | `404.html` |
| each asset | its published path |

- The output directory is emptied first. `.`, `/` and `..` are refused.
- Every request is a GET with no cookie and no query. The only header is
  `DP-Partial`, on partial requests.
- A path that answers anything but 200, or answers a stream, stops the
  export. The error names the path, the depth, and what to do.
- A path given twice is written once.
- A route with a parameter is listed in `Report::skipped`. Give its
  concrete paths to `paths`.

## Partials per depth

dp-swap ([github.com/brocanth/htmx](https://github.com/brocanth/htmx))
asks for only the layout layers the client does not hold. On a server it
sends the count in `DP-Partial`. A static host cannot read a header, so
with `partialDir: "partials"` it puts the count in the path:
`/partials/<depth>/<route>`.

`partials(n)` writes that tree. For each HTML page and each depth `1..=n`
the export sends `DP-Partial: <depth>` through the handler and writes the
answer to `partials/<depth>/<route>/index.html`. JSON and other files get
no partials: the content type decides.

**Every depth is written, even when two are equal.** A page with one
layout answers depth 2 like depth 1. The client chooses the depth from the
page it leaves, not from the page it goes to, so a missing file would be a
failed navigation. A duplicate costs only disk. Set `n` to the most layout
layers any page has.

## Base path

A GitHub Pages project site is served under `/<repo>/`. A page names its
links from the root. `base("/repo")` prefixes every root-relative URL
(`/x`, not `//cdn/x`, not `x`) in what the export writes:

- the attributes `href`, `src`, `action`, `formaction`, `poster`,
  `srcset` and `dp-layout` of every HTML page and partial;
- `url(...)` and `@import` in `.css` assets, `<style>` elements and
  `style` attributes.

The contents of `<script>` elements, JSON and JavaScript files are never
changed. A URL inside data is the application's to prefix, because only
the application knows which strings are URLs. The d3set gallery reads its
own `GALLERY_BASE` for that.

`dp-layout` is rewritten because dp-swap compares it with
`location.pathname`, which carries the base. Configure dp-swap with the
same `base` so the partial URLs are `/repo/partials/<depth>/<route>`.

The rewrite is a scanner, not an HTML parser. It reads tags, attributes,
comments and the `script` and `style` raw-text elements, and copies every
other byte unchanged. That is enough for markup a template engine wrote,
and keeps the crate free of a parser dependency.

## The rule around it

`static` has `Manifest::static_output`. Every other chosen module must
have `Manifest::outside_runtime`. `create::exclusive` refuses the rest
for `create` and `add`, and the chooser takes the other side off. See
`docs/modules.md`, "Static output". The crate does not enforce this rule:
an application that calls it directly decides for itself what its pages
need.
