# `superuser`

Operator accounts that pass every admin check, and a createsuperuser
command.

| | |
|---|---|
| Categories | **Enhancer** |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | none |
| Needs | [`auth`](auth.md): a superuser is an account with a flag, and `auth` holds the accounts |
| After | [`auth`](auth.md) |
| Adapts to | [`config`](config.md), [`commands`](commands.md), [`jobs`](jobs.md), [`cache`](cache.md), [`admin`](admin.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-auth`, feature `admin` with `admin` |
| Without it | no operator account; the admin opens only for a policy you write |

## What it writes

Files:

- `src/plugins/superuser/mod.rs`: the plugin, `staff(ctx)`, which reads the
  signed-in member of staff, and the store cell the roles tab reads.
- `src/commands/createsuperuser.rs`, with `commands` and accounts in
  Postgres (`jobs` or `cache`).

Slots:

- `AdminPanels`, with `admin`, behind `integrate.admin`:
  `config.with_policy(brocanth_auth::admin::StaffPolicy)`.
- `CommandsMod`: `pub mod createsuperuser;`, with the command.
- `AgentsSections`: "Superusers".

And `admin` adapts: its `screen` awaits `superuser::staff(ctx)` and passes
the member of staff as the principal.

## Decisions

- **A superuser is a flag on an account, set by an operator.** At start
  from `[superuser] email`, or with `createsuperuser --email`. Never from a
  form a visitor can reach. Both are idempotent, and an account that exists
  keeps its password.
- **The command exists only over Postgres.** An account that a command
  makes in a `MemoryStore` is gone when the command exits. In memory, the
  setting is the way.
- **The policy is an integration; the principal is not.** Turn
  `integrate.admin` off and the admin is back to `DenyAll`, but its
  principal is still the member of staff, so a policy of yours can read
  it. The principal is in the admin's file because reading a session is
  async, and no core hook or provider may wait.
- **An anonymous session is not staff.** A pre-login session carries no
  proof, whatever account it names.

## Settings

Section `[superuser]`.

| Key | File value | Help |
|---|---|---|
| `email` | none | The address of the superuser the server makes at start. |
| `password_env` | `"<APP>_SUPERUSER_PASSWORD"` | The name of the variable that holds its password. |

## Read more

- Runtime: [`crates/brocanth-auth/src/staff.rs`](../../crates/brocanth-auth/src/staff.rs)
- Source: [`crates/brocanth-auth/src/scaffold/staff.rs`](../../crates/brocanth-auth/src/scaffold/staff.rs)
