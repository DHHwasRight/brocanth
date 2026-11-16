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
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-auth`, feature `policy` |
| Without it | no roles; a member of staff is a superuser, and nobody else is |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/roles.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | [`superuser`](superuser.md): only a superuser gives a role, and `superuser` reads the signed-in member of staff |
| After | [`superuser`](superuser.md), [`superuser`](superuser.md) |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | none |
| Contributes | `AgentsSection` `roles`: how roles grant staff access, and who may change them; `AdminPanel` `roles`, on: a Roles tab where a superuser defines roles and gives them to accounts, and the owner of a tab gives read or edit on it |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | none |
<!-- end generated: capabilities -->

## What it writes

Files:

- `src/plugins/roles/mod.rs`: `require(ctx, permission)`, for a page.

Slots:

- `AdminPanels`, when the admin collects tabs, behind `integrate.admin`:
  the Roles tab, `brocanth_auth::admin::RolesPanel`, RBAC resource `roles`.
- `AgentsSections`: "Roles".

## Decisions

- **A permission is a level on a resource.** The admin asks for
  `<resource>.read` to look and `<resource>.edit` for every button and
  write. `<resource>.owner` includes both. A resource is a tab's key or a
  model's key. Reason: three levels per tab are a model a person can hold
  in their head (`docs/module-capabilities.md` §5.2).
- **Only a superuser changes roles, except inside one tab.** A role holder
  who could edit roles could give itself every permission. The owner of a
  resource gives and takes `read` and `edit` on it, and nothing else; only
  a superuser gives `owner`.
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
