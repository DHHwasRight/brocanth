# `resource`

Typed CRUD routes over one table, with the policy in the WHERE clause.

| | |
|---|---|
| Categories | **Routes** |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | nothing |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-resource` (features `pages`; no default features) |
| Without it | no records API; every CRUD route is a page somebody wrote by hand |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/resource.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | [`middleware-csrf`](middleware-csrf.md): AGENTS.md says how a fetch caller and a non-browser client get past the CSRF check |
| Contributes | `AgentsSection` `resource`: the records API, and its CSRF rule |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | `mount_at` = `"/api"`, mount, Machine: Where the records API answers. Every resource is under it. |
| Settings (run time) | none |
<!-- end generated: capabilities -->

## What it writes

Files:

- `src/plugins/resource/mod.rs`

Slots:

- src/plugins/mod.rs: `pub mod resource;`
- src/pages/mod.rs (the mounted routes): `crate::plugins::resource::mount(routes, "/api")?;`

## Settings

None.

## Read more

- [brocanth-resource](../../crates/brocanth-resource/README.md)
- [Policy](../policy.md)
- Source: [`crates/brocanth-resource/src/scaffold.rs`](../../crates/brocanth-resource/src/scaffold.rs)
