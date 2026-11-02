# `admin-model`

The admin's rows over SeaORM, one line per entity.

| | |
|---|---|
| Categories | **Routes** |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | [`admin`](admin.md), off until you tick it |
| Needs | [`model`](model.md): it reads rows through the ORM, and without `model` there is none |
| After | [`model`](model.md) |
| Adapts to | none |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | none |
| Blocked | the generated model plugin opens no pool, so there is no connection to read |

## What it writes

Nothing. The module is blocked, so `create` never calls it.

## Settings

None.

## Read more

- [The backoffice](../backoffice.md)
- Source: [`crates/brocanth-admin/src/scaffold.rs`](../../crates/brocanth-admin/src/scaffold.rs)
