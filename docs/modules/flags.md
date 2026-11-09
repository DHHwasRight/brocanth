# `flags`

Feature flags: on for all, a percentage, users or groups; rows in your Postgres.

The wizard calls it: *Feature flags: turn code on for some users without a deploy*.

| | |
|---|---|
| Categories | **Enhancer** |
| Wizard question | Operations |
| Default | off |
| Presets | none |
| Part of | nothing |
| Needs | [`db`](db.md): the rules are rows in the database the `db` module opens |
| After | [`db`](db.md) |
| Adapts to | [`config`](config.md), [`admin`](admin.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-flags` (feature `config` with `config`, `admin` with `admin`) |
| Without it | no feature flags; new code is on for everybody the moment it ships |

With [`admin`](admin.md), a tab at `/admin/flags` that lists every declared
flag and every stored rule, and turns each one. It is in the generated
`plugins::admin_panels`, behind `integrate.admin` in `src/modules/flags.rs`.

## What it writes

Files:

- `src/plugins/flags/mod.rs`: the flags as `Flag` constants, `ALL`,
  `enabled(ctx, flag)` for a page, and `actor(ctx)`, which says who is
  asking. `actor` is anonymous until the project has users.

Slots:

- src/plugins/mod.rs (the builder chain): `plugin_from_config::<crate::plugins::flags::Flags>("flags")`
- src/plugins/mod.rs (admin_panels), with `admin`: `FlagsPanel::installed(crate::plugins::flags::ALL)`
- config/<env>.toml: `[flags]`
- AGENTS.md: `## Feature flags`

## What the plugin does

- `Flags` on every request context: the rules, in memory. A read is a map
  lookup under a read lock. It never waits on the database.
- The first readiness check creates the `brocanth_flags` table (with
  `migrate`) and reads it, so the server never answers with every flag off
  because the copy is still empty. A task reads the table again every
  `refresh_secs`.
- A change made in the admin reaches this process at once and the others
  at their next read. A failed read keeps the last rules, and the tab shows
  the error.

A rule: on for everyone, a percentage of signed-in users, a list of user
ids, a list of groups. Any one that matches turns the flag on. An
anonymous actor sees only "on for everyone". A percentage puts a user in a
bucket from 0 to 99 from a hash of the flag key and the user id, so one
person stays in or out, and two flags at 10 % do not pick the same tenth.

## Decisions

| Question | Decision | Reason |
|---|---|---|
| A crate of its own? | yes, `brocanth-flags` | No crate owns "a switch the operator turns while the process runs": `brocanth-config` is read once, and `brocanth-cache` forgets. |
| `enabled(ctx, "x")` with a string? | no, `enabled(ctx, flags::X)` with a constant | A misspelt string is a flag that is always off, silently. A misspelt constant does not compile. `Flags::enabled_key` takes a string for a key known only at run time. |
| A flag with no stored rule? | off | Code ships first, then somebody turns it on. |
| Read the database per request? | no, a copy in memory, read every `refresh_secs` | A flag is read on hot paths. Tens of rows fit in memory. The cost: another process sees a change up to `refresh_secs` later. |
| Who is the actor? | `actor(ctx)` in the project's file | The framework does not know the project's user type. Two lines name it. |
| The admin tab a toggle? | yes, `integrate.admin`, on | `docs/module-config.md` §2.2: the guest holds the toggle. |
| A key the code does not declare? | the tab lists its stored rule as "not declared", and refuses a form for any other key | A rule left behind after a flag was deleted can be removed. A typo in a URL cannot create a row. |

## Not built

- A rule per environment in the table. One table is one environment.
- A schedule (on at a time), and a history of changes. With `audit`, a
  table write through the model layer is recorded; the flags store writes
  SQL and is not.

## Settings

Section `[flags]`. Environment variable: `$<APP>_FLAGS__<KEY>`. Flag: `--flags.<key>`. Rename or hide a flag in `src/modules/flags.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `refresh_secs` | integer | `10` | `--flags.refresh_secs` | Seconds between two reads of the table. 0 reads only at start. |
| `migrate` | bool | `true` | `--flags.migrate` | Create the brocanth_flags table on the first readiness check. |

## Read more

- [`crates/brocanth-flags/README.md`](../../crates/brocanth-flags/README.md)
- Source: [`crates/brocanth-flags/src/scaffold.rs`](../../crates/brocanth-flags/src/scaffold.rs)
