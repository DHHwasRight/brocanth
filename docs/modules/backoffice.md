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
| Adapts to | [`superuser`](superuser.md): the claim each call carries holds the signed-in member of staff's grants; [`admin-model`](admin-model.md): the wire serves the admin's models over the ORM, not the in-memory fixture; [`rls`](rls.md): with `policy` too, the wire runs each call as the staff member's database identity; [`policy`](policy.md): with `rls` too, the wire runs each call as the staff member's database identity |
| Contributes | `AgentsSection` `backoffice`: how the second binary serves the admin |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | `[backoffice]`: `addr`, `embedded`, `wire`, `data_url`, `service_secret` (secret), `tx_timeout_secs`, `tx_max` |
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
  carries; with `superuser` it reads `brocanth_auth::Staff`), the
  server wrap `Door`, and `wire_source`, the admin's source as the wire
  serves it (with `admin-model` it compiles `src/plugins/admin/models.rs`
  a second time).

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

Over the wire (docs/backoffice.md §7.5): the application with
`--backoffice.wire=true` also answers `POST /_backoffice/wire`, signed
calls only. `backoffice` with `--backoffice.data_url=http://app:3000`
reads and writes the admin's rows there, not in its own process. Both
take `--backoffice.service_secret_file=PATH` (or `_env`); without it, or
with fewer than 32 bytes, both stop at start. Start the application
first: the backoffice reads the registry at start.

## Settings

| Key | File value | Flag | What |
|---|---|---|---|
| `addr` | `"127.0.0.1:3001"` | `--backoffice.addr` | Where `src/bin/backoffice.rs` listens: an address and a port. Not an address: both binaries stop at start. |
| `embedded` | `true` | `--backoffice.embedded` | Serve the admin from the application binary too. False when the backoffice runs as its own service. |
| `wire` | `false` | `--backoffice.wire` | The application serves the admin's rows at `/_backoffice/wire`. Needs `service_secret`. |
| `data_url` | none | `--backoffice.data_url` | `src/bin/backoffice.rs` reads the admin's rows at this URL. Needs `service_secret`. |
| `service_secret` | none | `--backoffice.service_secret_file`, `--backoffice.service_secret_env` | The secret both binaries sign the wire with, 32 bytes or more. A secret: no value flag. |
| `tx_timeout_secs` | `30` | `--backoffice.tx_timeout_secs` | An idle transaction of the wire rolls back after this. |
| `tx_max` | `32` | `--backoffice.tx_max` | The most transactions the wire holds open. |
