# `sitemap`

A sitemap.xml of every page with no parameter, served at /sitemap.xml.

The wizard calls it: *A sitemap.xml that lists every page*.

| | |
|---|---|
| Categories | **Routes** |
| Wizard question | Pages |
| Default | off |
| Presets | none |
| Part of | nothing |
| Needs | none |
| After | [`config`](config.md) |
| Adapts to | [`config`](config.md), [`admin`](admin.md), [`jt`](jt.md), [`realtime-sse`](realtime-sse.md), [`static`](static.md) |
| Conflicts | none |
| Outside the runtime | yes |
| Crates | `brocanth-robots` |
| Without it | no sitemap.xml; a crawler finds only the pages a link reaches |

## What it writes

Files:

- `src/pages/sitemap.xml.rs`
- `src/plugins/sitemap/mod.rs`

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs: `pub mod sitemap;`
- src/plugins/mod.rs (the provider list): `sitemap crate::plugins::sitemap::Sitemap`
- src/pages/mod.rs (the route table test): `/sitemap.xml`
- config/<env>.toml: `[sitemap]`

The plugin renders the XML once, at startup, from the page table
(`crate::pages::handlers()`). It lists every GET page whose pattern has no
parameter and whose last segment has no dot. So `/users/{id}` and
`/robots.txt` are not listed. A new page is listed at the next build.

## Settings

Section `[sitemap]`. Environment variable: `$<APP>_SITEMAP__<KEY>`. Flag: `--sitemap.<key>`. Rename or hide a flag in `src/modules/sitemap.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `base_url` | text | `"http://localhost:3000"` | `--sitemap.base_url` | The site's address: scheme, host, and base path if any. |
| `extra` | list | `[]` | none | More paths to list, such as the pages of a route with a parameter. |
| `exclude` | list | `[]` (`/admin`, `/status`, `/realtime` with `admin`, `jt`, `realtime-sse`) | none | Path prefixes kept out of the sitemap. |

Set `base_url` in production, or every `<loc>` names localhost. A base URL
that is not a full `http(s)` address stops the build with a `help:` line.

To give a page a `<lastmod>`, call `path_modified` in the plugin's
`sitemap` method. The builder accepts `2026-11-05` or a full W3C date and
time.

## With `robots`

The `robots` plugin writes `Sitemap: <base_url>/sitemap.xml` when its own
`sitemap` setting is unset. A closed robots.txt (`index = false`) writes no
sitemap line.

## With `static`

`export` requests `/sitemap.xml` like every GET route and writes the file
`sitemap.xml`. The export is the published site, so set `base_url` to the
published address before you publish. Under a base path, include it:
`https://user.github.io/repo`.

## Read more

- [robots.txt and sitemap.xml design](../robots.md)
- [brocanth-robots](../../crates/brocanth-robots/README.md)
- Source: [`crates/brocanth-robots/src/scaffold_sitemap.rs`](../../crates/brocanth-robots/src/scaffold_sitemap.rs)
