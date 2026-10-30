# Static export

The `static` module compiles a project's pages to files. This page
records what the export does today, so a later change (a library crate,
partials per depth, a base path) can say what it changes.

## What the module writes

- `src/commands/export.rs`, from `EXPORT_RS` in
  `crates/brocanth-cli/src/modules/static_site.rs`. The file belongs to
  the project after it is written.
- `pub mod export;` in `src/commands/mod.rs`, `/dist` in `.gitignore`,
  a `[tasks.export]` mise task, and a `## Static site` section in
  `AGENTS.md`.
- With `ci-github`: a `cargo run -- export` step and an
  `actions/upload-artifact` step for `dist/`.

The golden file `crates/brocanth-cli/tests/golden/static.txt` holds the
exact text.

## What `cargo run -- export` does

1. Builds the application with `crate::app::build()`, inside a
   current-thread Tokio runtime.
2. Collects the routes of `app.router.routes()` that allow GET:
   - a route with no parameter is exported;
   - a route with a parameter is skipped and listed on stderr.
3. Adds every path in the `PATHS` const (default `["/users/7"]`).
4. Empties `--out` (default `dist`).
5. For each path, sends `brocanth_server::Request::new(Verb::Get, path)`
   through `crate::app::handle`, the same hook pipeline `serve` uses. The
   request has no header, no cookie and no query.
   - A status other than 200 stops the export with an error.
   - A streamed body stops the export with an error.
6. Writes each answer:
   - `/` to `index.html`, `/about` to `about/index.html`;
   - a path whose last segment has a dot (`/feed.xml`) to that name.
7. Requests `/__export_missing__`. A 404 answer is written to `404.html`.
8. Writes every entry of `crate::assets::ASSETS` at its published path,
   when the project embeds assets (`rsx-tailwind` or `rsx-htmx`).

## What it does not do

- No base path. Every URL in a page is root-relative.
- No partials. A request never carries `DP-Partial`, so each file is the
  whole document.
- No route list other than the router and `PATHS`.

## The rule around it

`static` has `Manifest::static_output`. Every other chosen module must
have `Manifest::outside_runtime`. `create::exclusive` refuses the rest
for `create` and `add`, and the chooser takes the other side off. See
`docs/modules.md`, "Static output".
