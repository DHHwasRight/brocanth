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
| Crates | `brocanth-admin-model` |
| Without it | the admin reads no table; its data source is an empty fixture |

## What it writes

Files:

- `src/plugins/admin/models.rs`: `admin_source!` with an empty list. Add one
  line per entity the admin shows.

And `admin` adapts: `src/plugins/admin/mod.rs` declares `mod models;` and
builds the admin over `Models`, with the connection of the
`brocanth_model::Orm` on the request's context.

## Why the admin is built on the first request

`mount` runs when the route table is built. The plugins start after that:
`db` opens the pool in `before_start`, and `model` wraps it. So `mount`
holds an `OverOrm`, and the first request builds the admin from the ORM on
its context, once. A row policy for a model that `models.rs` does not list
panics on that first request, not at startup.

A route table that a test builds sees a `Ctx` built by hand, with no ORM.
That table's admin is built over an empty fixture, with the same
configuration, so the generated "ships locked" test still holds.

## Settings

None.

## Read more

- [The backoffice](../backoffice.md)
- Source: [`crates/brocanth-admin/src/scaffold.rs`](../../crates/brocanth-admin/src/scaffold.rs)
