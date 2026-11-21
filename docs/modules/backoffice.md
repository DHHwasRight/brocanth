# `backoffice`

The admin as a service of its own: a second binary that serves only the admin.

The wizard calls it: *The admin as a service of its own: a second binary that serves only the admin*.

| | |
|---|---|
| Categories | **Middleware**, External |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | nothing; it needs [`admin`](admin.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-backoffice` |
| Without it | no second binary; the admin is served by the application alone and reads its rows directly |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/backoffice.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | [`admin`](admin.md): it serves the admin from a second binary, so there is nothing to serve without it |
| After | [`admin`](admin.md), [`observability`](observability.md) |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | [`superuser`](superuser.md): the claim each call carries holds the signed-in member of staff's grants |
| Contributes | `AgentsSection` `backoffice`: how the second binary serves the admin |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | `[backoffice]`: `addr`, `embedded` |
<!-- end generated: capabilities -->

## What it writes

Files:

- `src/bin/backoffice.rs`: the second binary. It compiles the
  application's own modules with `#[path]` (the generator writes the
  `mod` lines, `Scaffold::app_binary`), reads the flags, and serves at
  `[backoffice] addr`. Rewritten by `brocanth add`, as `src/main.rs` is.
  `Cargo.toml` lists it with `test = false`: its tests are the
  application's, and they run once.
- `src/plugins/backoffice/mod.rs`: the `[backoffice]` settings, the
  `Backoffice` plugin that checks them at start, `contract` (the admin's
  data source on the tab data contract), `claim` (the `Claim` each call
  carries; with `superuser` it reads `brocanth_auth::Staff`), and the
  server wrap `Door`.

Slots:

- src/plugins/mod.rs (the builder chain): the `Backoffice` plugin.
- src/plugins/mod.rs (admin_panels):
  `config.data_through(crate::plugins::backoffice::contract)`.
- src/plugins/mod.rs (the server wraps): `backoffice::Door`, inside
  `observability`'s wrap.
- mise.toml: `[tasks.backoffice]`, `cargo run --bin backoffice`.
- AGENTS.md: `## The backoffice binary`.

## How it runs

One code path, two roles (docs/backoffice.md §7.2). Both binaries build
the same application: the same plugins, the same pool, the same route
table. `Door` answers 404 to a path its binary does not serve:

| Binary | Serves |
|---|---|
| the application, `embedded = true` | every path |
| the application, `embedded = false` | every path except `mount_at` |
| `backoffice` | `mount_at` and the paths under it |

To split: run the application with `--backoffice.embedded=false`, run
`backoffice` beside it, and route `mount_at` to it at the proxy.

## Settings

| Key | File value | Flag | What |
|---|---|---|---|
| `addr` | `"127.0.0.1:3001"` | `--backoffice.addr` | Where `src/bin/backoffice.rs` listens: an address and a port. Not an address: both binaries stop at start. |
| `embedded` | `true` | `--backoffice.embedded` | Serve the admin from the application binary too. False when the backoffice runs as its own service. |
