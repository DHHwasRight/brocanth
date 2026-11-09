# `roles`

Roles that grant permissions, stored with the accounts, for staff and the
admin.

| | |
|---|---|
| Categories | **Enhancer** |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | none |
| Needs | [`superuser`](superuser.md): only a superuser gives a role, and `superuser` reads the signed-in member of staff |
| After | [`superuser`](superuser.md) |
| Adapts to | [`admin`](admin.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-auth`, feature `policy` |
| Without it | no roles; a member of staff is a superuser, and nobody else is |

## What it writes

Files:

- `src/plugins/roles/mod.rs`: `require(ctx, permission)`, for a page.

Slots:

- `AdminPanels`, with `admin`, behind `integrate.admin`: the Roles tab,
  `brocanth_auth::admin::RolesPanel`.
- `AgentsSections`: "Roles".

## Decisions

- **A permission is a string.** The admin asks for `<model>.view`, `.add`,
  `.change`, `.delete`, and `<panel>.view` and `.act`. These are Django's
  words, so a team that knows `post.change_post` reads them.
- **Only a superuser changes roles.** A role holder who could edit roles
  could give itself every permission.
- **Roles answer which people, not which rows.** `Staff::gate` turns a
  permission into a `brocanth_policy::Pred`, so a role check and a row
  rule go into one `condition()`. Nothing filters after a load.
- **Deleting a role removes its assignments.** A role created later with
  the same name grants nothing by accident.
- **The roles live in the accounts store.** `MemoryStore` and `PgStore`
  implement `StaffStore`. With `PgStore` the tables are
  `brocanth_auth_superusers`, `brocanth_auth_roles` and
  `brocanth_auth_account_roles`, whose foreign key deletes the assignments
  of a deleted role.

## Settings

None.

## Read more

- Runtime: [`crates/brocanth-auth/src/staff.rs`](../../crates/brocanth-auth/src/staff.rs),
  [`crates/brocanth-auth/src/admin/staff.rs`](../../crates/brocanth-auth/src/admin/staff.rs)
- Source: [`crates/brocanth-auth/src/scaffold/staff.rs`](../../crates/brocanth-auth/src/scaffold/staff.rs)
