# `robots`

A robots.txt from your settings, served at /robots.txt; closed outside production.

The wizard calls it: *A robots.txt that says what crawlers may read*.

| | |
|---|---|
| Categories | **Routes** |
| Wizard question | Pages |
| Default | off |
| Presets | Static site |
| Part of | nothing |
| Conflicts | none |
| Outside the runtime | yes |
| Crates | `brocanth-robots` |
| Without it | no robots.txt; every crawler reads every page, and so does every staging server's |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/robots.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | [`i18n`](i18n.md) |
| Provides | none |
| Requires | none |
| Uses | `ConfigSource` (settings read from a file): the per-crawler groups are records, written to the file; the development file opens the site; `Mount` (a mounted path): every `Backoffice` mount, such as the admin, is disallowed, at the path its module file mounts it |
| Adapts to | [`sitemap`](sitemap.md): the Sitemap: line names /sitemap.xml at the sitemap's base URL, unless `sitemap` is set; [`static`](static.md): the file says index = true, because the export is what gets published; [`i18n-routing`](i18n-routing.md): each rule path is also written under each locale segment: /drafts and /fr/drafts |
| Contributes | none |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | `disallow_backoffice` = `true`: Disallow every Backoffice mount, such as the admin. The rule also tells every reader of robots.txt where the backoffice is. |
| Settings (run time) | `[robots]`: `index`, `disallow`, `allow`, `sitemap` |
<!-- end generated: capabilities -->

## What it writes

Files:

- `src/pages/robots.txt.rs`
- `src/plugins/robots/mod.rs`

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs: `pub mod robots;`
- src/plugins/mod.rs (the provider list): `robots crate::plugins::robots::Robots`
- src/pages/mod.rs (the route table test): `/robots.txt`
- config/<env>.toml: `[robots]`

## Settings

Section `[robots]`. Environment variable: `$<APP>_ROBOTS__<KEY>`. Flag: `--robots.<key>`. Rename or hide a flag in `src/modules/robots.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `index` | bool | `false` (`true` with `static`) | `--robots.index` | Let crawlers read this site. False answers `Disallow: /` and nothing else. |
| `disallow` | list | `[]` | none | Paths no crawler may read, each from the root. |
| `allow` | list | `[]` | none | Paths a crawler may read under a disallowed one. |
| `sitemap` | text, optional | commented out | `--robots.sitemap` | Where the sitemap is, as a full URL. With `sitemap`, unset means the sitemap module's URL. |
| `groups` | records, `[[robots.groups]]` | an AI-crawler example, commented out | none | Groups for named crawlers: `agents`, `allow`, `disallow`. File only. |

The plugin's own default for `index` is `true`: a production server with no
`[robots]` section is indexed.

## Choices in `src/modules/robots.rs`

With [`admin`](admin.md), robots.txt disallows the admin at the path
`mount_at` in `src/modules/admin.rs` names. `integrate.admin: false` turns
that rule off. The rule is code in `src/plugins/robots/mod.rs`, not a
`disallow` entry, so it follows `mount_at`. **Reason for a toggle:** the
line `Disallow: /admin` also tells every reader of robots.txt where the
backoffice is.

## Read more

- [robots.txt design](../robots.md)
- [brocanth-robots](../../crates/brocanth-robots/README.md)
- Source: [`crates/brocanth-robots/src/scaffold.rs`](../../crates/brocanth-robots/src/scaffold.rs)
