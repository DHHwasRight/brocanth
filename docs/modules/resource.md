# `resource`

Typed CRUD routes over one table, with the policy in the WHERE clause.

| | |
|---|---|
| Categories | **Routes** |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | nothing |
| Needs | none |
| After | none |
| Adapts to | none |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-resource` (features `pages`; no default features) |
| Without it | no records API; every CRUD route is a page somebody wrote by hand |

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
