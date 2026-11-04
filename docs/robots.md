# robots.txt

The `robots` module serves `/robots.txt` from settings. A draft: the
decisions below are open to review.

## Where it lives

In a new crate, `brocanth-robots`, with the module behind its `scaffold`
feature. The reasons:

- The builder (`Robots`, `Group`) is useful without the module. A
  hand-written application calls it in `main`.
- The module writes calls to that builder into a project. Kept in the same
  crate, a renamed method breaks this crate's build, not a generated
  project later. This is the openapi module's rule.
- The crate has no dependencies. An existing crate would add its own
  dependencies to every application that wants only a robots.txt.

## What a project gets

| File | Role |
|---|---|
| `src/plugins/robots/mod.rs` | `Settings`, and a plugin that renders the text once at `configure` |
| `src/pages/robots.txt.rs` | the route; the last dot is the extension, so it answers `/robots.txt` |
| `[robots]` in `config/<env>.toml` | the settings, with `config` |

The plugin renders at startup, so a rule a crawler would ignore stops the
build. `Robots::render` refuses a rule path that does not start with `/` or
`*`, a sitemap that is not an absolute `http(s)` URL, a crawler token with a
space or `:`, and any line break (which would let a value add a rule).

The page answers `text/plain; charset=utf-8` with `cache-control:
max-age=3600`. Crawlers keep the file for about a day anyway.

The generated page has a test that sends `GET /robots.txt` through the whole
application and compares the body with what the project answered on the day
it was generated.

## Settings

| Key | Kind | Why |
|---|---|---|
| `index` | bool | `false` answers `User-agent: *` / `Disallow: /` and nothing else |
| `disallow` | list | paths no crawler may read |
| `allow` | list | exceptions under a disallowed path |
| `sitemap` | text, optional | an absolute URL |

**Decision: development and staging are closed, production is open.** The
plugin's default for `index` is `true`. The generated
`config/development.toml` writes `index = false`. So:

- a development server answers `Disallow: /`;
- production reads the default, `true`, unless its file says otherwise;
- staging sets `index = false` in its file or with `$<APP>_ROBOTS__INDEX=false`.

Why not read the environment name (`$<APP>_ENV`) and close every
environment that is not `production`? It ties a security-relevant answer to
a naming convention the project does not otherwise have, and a project
without `config` has no environment name at all. A setting is visible in
the settings report (`my-app settings`) and can be checked per layer. This
is open question 1 below.

**Closed is a whole file.** When `index = false`, the `allow` and `sitemap`
values are not written. One `Allow:` left over from the production list
would open that path on staging.

**Lists have no flag.** A list of paths is file-only, as for every declared
list (see `docs/module-flags.md`).

## Adaptations

| With | What changes |
|---|---|
| `config` | settings come from `[robots]`, not a literal |
| `admin` | `disallow` starts as `["/admin"]` |
| `static` | the file says `index = true`, because `export` reads that file and the export is the published site |

`outside_runtime`: the static export asks every GET route for its answer.
`/robots.txt` has a dot in its last segment, so the export writes the file
`robots.txt` (`docs/static-export.md`). The `static` preset can therefore
take this module.

**No base-path prefix.** Crawlers read only the root `/robots.txt` of a
host. Under a base path, as on a GitHub Pages project site
(`/repo/robots.txt`), no crawler asks for the file, so prefixing its rules
would change nothing. The README of the module says this. The export's
`--base` rewrite does not touch `text/plain` files either.

Not adapted: `openapi` and `resource`. An API path in robots.txt is
harmless and rarely wanted; a developer adds `/api` to `disallow` if they
want it.

## Later: sitemap.xml

A sitemap is the natural next module, and it is not built here. Notes for
it:

- The route table already knows every GET route without a parameter; the
  static export uses the same list. A sitemap is that list plus concrete
  paths for the routes with parameters.
- It needs the site's absolute origin (`https://example.com`), which no
  setting holds today. Both modules would read it.
- With a sitemap module, `robots.sitemap` defaults to its URL.

## Open questions

1. Close every environment that is not `production` automatically, from
   `$<APP>_ENV`, instead of the `index` setting?
2. Tick `robots` in the `static` preset by default?
3. Per-crawler groups (for example blocking AI crawlers such as `GPTBot`)
   are in the builder but not in the settings. Add a `[[robots.groups]]`
   table?
