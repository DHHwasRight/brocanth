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
| Needs | none |
| After | [`config`](config.md) |
| Adapts to | [`config`](config.md), [`admin`](admin.md), [`sitemap`](sitemap.md), [`static`](static.md) |
| Conflicts | none |
| Outside the runtime | yes |
| Crates | `brocanth-robots` |
| Without it | no robots.txt; every crawler reads every page, and so does every staging server's |

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

Section `[robots]`. Environment variable: `$<APP>_ROBOTS__<KEY>`. Flag: `--robots.<key>`. Rename a flag in `src/flags.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `index` | bool | `false` (`true` with `static`) | `--robots.index` | Let crawlers read this site. False answers `Disallow: /` and nothing else. |
| `disallow` | list | `[]` (`["/admin"]` with `admin`) | none | Paths no crawler may read, each from the root. |
| `allow` | list | `[]` | none | Paths a crawler may read under a disallowed one. |
| `sitemap` | text, optional | commented out | `--robots.sitemap` | Where the sitemap is, as a full URL. With `sitemap`, unset means the sitemap module's URL. |
| `groups` | records, `[[robots.groups]]` | an AI-crawler example, commented out | none | Groups for named crawlers: `agents`, `allow`, `disallow`. File only. |

The plugin's own default for `index` is `true`: a production server with no
`[robots]` section is indexed.

## Read more

- [robots.txt design](../robots.md)
- [brocanth-robots](../../crates/brocanth-robots/README.md)
- Source: [`crates/brocanth-robots/src/scaffold.rs`](../../crates/brocanth-robots/src/scaffold.rs)
