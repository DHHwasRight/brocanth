# robots.txt

The `robots` module serves `/robots.txt` from settings. The "A static site"
wizard preset ticks it by default.

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
| `groups` | records, `[[robots.groups]]` | groups for named crawlers, after the `*` group |

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
the settings report (`my-app settings`) and can be checked per layer.
Decided: keep the explicit setting.

**Closed is a whole file.** When `index = false`, the `allow`, `groups`
and `sitemap` values are not written. One `Allow:` left over from the production list
would open that path on staging.

**Lists have no flag.** A list of paths is file-only, as for every declared
list (see `docs/module-flags.md`).

**Groups are records, file-only.** Each `[[robots.groups]]` table has
`agents`, `allow` and `disallow`, all lists. A record list has no flag and
no environment variable, as for the rate limit's rules. A table with no
agent stops the build with a `help:` line. The generated file holds one
example, commented out, that asks AI training crawlers (`GPTBot`, `CCBot`,
`Google-Extended`, `ClaudeBot`) to read nothing. robots.txt is a request:
nothing makes a crawler obey it, and the docs say so. A crawler that
matches a named group follows that group only, not the `*` rules (RFC
9309), so a group repeats any `*` rule it should keep.

## Adaptations

| With | What changes |
|---|---|
| `config` | settings come from `[robots]`, not a literal |
| `admin` | the plugin disallows the admin's `mount_at`, read from `src/modules/admin.rs` (`integrate.admin`); the `disallow` setting stays empty |
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

## sitemap.xml

The `sitemap` module serves `/sitemap.xml`. Its page is
`docs/modules/sitemap.md`.

- **The list.** Every GET page from the page table
  (`crate::pages::handlers()`) whose pattern has no parameter and whose
  last segment has no dot, plus `extra`, minus the prefixes in `exclude`.
  The static export reads the same table, so the sitemap and the export
  agree on which pages exist.
- **The base URL.** A setting of the sitemap, `[sitemap] base_url`. It may
  carry a base path (`https://user.github.io/repo`), so the sitemap of a
  GitHub Pages project site is right.
- **With `robots`.** `robots.sitemap` defaults to
  `crate::plugins::sitemap::location()`. The robots plugin then renders in
  `after_config`, because the sitemap plugin sets its URL in `configure`
  and every plugin finishes a stage before the next starts.
- **lastmod.** The builder takes one per page (`path_modified`). The
  settings do not: the page table has no dates, and one date for every page
  says nothing true.

### Where the base URL lives

Decision: in `[sitemap] base_url`, read by `robots` through the sitemap
plugin. Reasons:

- The sitemap is the only reader that needs an absolute address today.
  `analytics` names a site by its host, `gtag` needs none, and `beacon`
  names another server.
- A framework-level `[app] url` needs a framework setting with a flag and
  an environment variable. The framework has no such setting yet:
  `[server] shutdown_grace_secs` has no flag for that reason
  (`docs/module-flags.md`).
- One place still holds it: `robots` reads the sitemap's value, it does not
  repeat it.

Move it to `[app]` when a second module needs the address.

### Why in this crate

`brocanth-robots` holds both builders and both modules. Both files are for
crawlers, both have no dependencies, and the `Sitemap:` line of one is the
location of the other. A crate of its own would add a crate and save
nothing. `analytics` and `beacon` share a crate for the same reason.

## Decisions

1. The `index` setting stays explicit. It is not derived from `$<APP>_ENV`.
2. The "A static site" preset ticks `robots`.
3. Per-crawler groups are settings: `[[robots.groups]]`, file-only.
4. The site's base URL is `[sitemap] base_url`, not a framework setting,
   until a second module needs it.
5. The preset does not tick `sitemap`: its development base URL is
   localhost, and a published sitemap of localhost URLs is worse than none.
