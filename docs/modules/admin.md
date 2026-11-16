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
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-admin` (no default features) |
| Without it | no backoffice; a row is changed through a page somebody wrote, or SQL |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/admin.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | [`middleware-csrf`](middleware-csrf.md): every form carries the CSRF token; [`superuser`](superuser.md): the principal is the signed-in member of staff, read from the session; [`admin-model`](admin-model.md): the rows are read through the ORM, from `src/plugins/admin/models.rs`; [`commands`](commands.md): the binary gets `admin eject` and `admin diff`, which run `brocanth admin` in the project |
| Contributes | `AgentsSection` `admin`: how a model gets an admin screen |
| Collects | `AdminPanel`: each tab in the sidebar, behind its RBAC resource; `plugins::admin_panels` lists them |
| Exports | none |
| Hooks | none |
| Options (build time) | `mount_at` = `"/admin"`, mount, Backoffice: Where the backoffice answers. A page of yours under this path is a build error, not a race. Every environment serves the same routes, so this is a constant and not a setting. |
| Settings (run time) | `[admin]`: `title`, `header`, `index_title`, `site_url`, `stylesheet` |
<!-- end generated: capabilities -->

## What it writes

Files:

- `src/plugins/admin/mod.rs`
- `src/commands/admin.rs`, with `commands`: `admin eject` and `admin diff`, which run
  `brocanth admin` in the project root

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

Section `[admin]`. Environment variable: `$<APP>_ADMIN__<KEY>`. Flag: `--admin.<key>`. Rename or hide a flag in `src/modules/admin.rs`.

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
