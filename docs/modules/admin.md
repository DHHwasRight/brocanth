# `admin`

A Django-shaped backoffice at /admin, locked until you say who may in.

The wizard calls it: *A backoffice at /admin, locked until you open it*.

| | |
|---|---|
| Categories | **Routes**, Enhancer |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | nothing |
| Needs | none |
| After | none |
| Adapts to | [`config`](config.md), [`middleware-csrf`](middleware-csrf.md), [`admin-model`](admin-model.md), [`commands`](commands.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-admin` (no default features) |
| Without it | no backoffice; a row is changed through a page somebody wrote, or SQL |

## What it writes

Files:

- `src/plugins/admin/mod.rs`
- `src/commands/admin.rs`, with `commands`: `admin eject`, which runs
  `brocanth admin eject` in the project root

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs: `pub mod admin;`
- src/commands/mod.rs, with `commands`: `pub mod admin;`
- src/plugins/mod.rs (the provider list): `admin crate::plugins::admin::AdminSettings`
- src/pages/mod.rs (the mounted routes): `crate::plugins::admin::mount(routes, "/admin")?;`
- src/pages/mod.rs (the route table test): `/admin` (2 entries)
- config/<env>.toml: `[admin]`
- AGENTS.md: `## The admin`

## Settings

Section `[admin]`. Environment variable: `$<APP>_ADMIN__<KEY>`. Flag: `--admin.<key>`. Rename a flag in `src/flags.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `title` | text | `"<Title> admin"` | `--admin.title` | The end of every <title>, and the header and index heading unless set. |
| `header` | text | `"Acme backoffice"` (commented out) | `--admin.header` | What the top of the sidebar says. Empty means none. |
| `index_title` | text | `"Site administration"` (commented out) | `--admin.index_title` | The index page's heading. Empty means none. |
| `site_url` | text | `"/"` (commented out) | `--admin.site_url` | Where the sidebar's "View site" link points. Empty means none. |
| `stylesheet` | text | `"/admin-theme.css"` (commented out) | `--admin.stylesheet` | A stylesheet every screen links after the shipped one: the theme. Empty means none. |

## Read more

- [The backoffice](../backoffice.md)
- [Customizing the admin](../admin-customization.md)
- Source: [`crates/brocanth-admin/src/scaffold.rs`](../../crates/brocanth-admin/src/scaffold.rs)
