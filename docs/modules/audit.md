# `audit`

Change history: who changed which row, when, and what; rows in your Postgres.

The wizard calls it: *Change history: who changed which row, and when*.

| | |
|---|---|
| Categories | **Middleware**, Enhancer |
| Wizard question | Operations |
| Default | off |
| Presets | none |
| Part of | nothing |
| Needs | [`db`](db.md): the history is rows in the database the `db` module opens; [`model`](model.md): it records the changes the model layer publishes |
| After | [`db`](db.md) |
| Adapts to | [`config`](config.md), [`admin`](admin.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-audit` (feature `config` with `config`, `admin` with `admin`) |
| Without it | no change history; who changed a row is not recorded |

With [`admin`](admin.md): a History tab at `/admin/history`, a History link
on every record's detail screen, and every admin write recorded with the
values before and after. All three are in the generated
`plugins::admin_panels`, behind `integrate.admin` in `src/modules/audit.rs`.
`hooks.record` in the same file turns the model hook off.

## What it writes

Files:

- `src/plugins/audit/mod.rs`: `sink(ctx)`, the `ChangeSink` a page hands
  to `insert_published`, `update_published` and `delete_published`, and
  `actor(ctx)`, which names who is asking. `actor` returns `None` until the
  project has users.

Slots:

- src/plugins/mod.rs (the builder chain): `plugin_hooking_from_config::<brocanth_audit::AuditPlugin>("audit")`, then `hooks_off_from("audit", "src/modules/audit.rs", ..)`
- src/plugins/mod.rs (the provider list): `audit brocanth_audit::AuditPlugin`
- src/plugins/mod.rs (the hook record): `brocanth_audit::AuditPlugin`
- src/plugins/mod.rs (admin_panels), with `admin`: `brocanth_audit::admin::install(config)`
- config/<env>.toml: `[audit]`
- AGENTS.md: `## Change history`

## What the plugin does

- `Trail` on every request context. `trail.by(actor)` and
  `trail.sink(Option<actor>)` are `ChangeSink`s: each change published to
  them becomes an entry with that actor.
- A model hook, `audit.record`, on `brocanth_model::plugin::RowWritten`.
  A change published through `HookSink::new(app.model_dispatch())` becomes
  an entry with no actor.
- A write only queues. A task writes the queue every `flush_secs` seconds,
  in one transaction. A failed write keeps the queue for the next try and
  never reaches a request. Past 10 000 queued entries, a new one is dropped
  and counted.
- Once a day, the task deletes the entries older than `retention_days`.
  0 keeps every entry.
- `before_start` takes the `brocanth_db::Db` the `db` plugin put in the
  services. With `migrate`, the first readiness check creates the table.

An entry holds: when (seconds since 1970), who, the source (`app` or
`admin`), the table, the row key, the label the admin showed, the kind
(created, updated, deleted) and the columns. A change of the model layer
names its columns only: a `RowChange` carries no values. An admin write
carries each column's value before and after.

## Decisions

| Question | Decision | Reason |
|---|---|---|
| A crate of its own? | yes, `brocanth-audit` | It reads two seams that must not depend on each other: `brocanth-events` is std-only, `brocanth-model` must not know who reads its changes (PLAN.md §10), and `brocanth-admin` keeps no table. |
| How does a model write reach the history? | a model hook, and `Trail` as a `ChangeSink` | The hook is how a plugin hears a write in the core (`brocanth_model::plugin`). The sink is how a page records the actor: a model hook runs with no request, so it cannot know who wrote. |
| How does an admin write reach it? | a new seam in the admin, `AdminConfig::journal` | The admin knows the values before and after; a `RowChange` does not. The admin keeps no table, so it tells a `Journal`, and this crate is one. |
| Values for a model write? | no, the column names | A `RowChange` carries names only. Reading the row again to diff it would put a query on every write. |
| `at` as a `timestamptz`? | no, a `bigint` of seconds | No date library in the crate. The page formats it as UTC with Hinnant's civil-from-days. |
| The fields as `jsonb`? | no, three `text[]` of one length | No JSON library. Postgres arrays hold NULL, which is a NULL value. |
| A write waits for the history? | no, a queue and a task | A write must not fail or slow down because the history is slow. The cost: a crash loses at most `flush_secs` seconds of entries. |
| Retention a setting? | yes, `retention_days` | Two deployments of one image may keep a different time. A compliance rule is per deployment. |
| The model hook a toggle? | yes, `hooks.record`, on | A project that records only admin writes turns it off, and the plugin still serves the tab and the journal. |
| The admin tab a toggle? | yes, `integrate.admin`, on | `docs/module-config.md` §2.2: the guest holds the toggle. |

## Not built

- The actor of a model hook's entry. A model hook has no request. Use the
  sink of `src/plugins/audit/mod.rs`.
- A revert button. An entry of the model layer holds no values to revert to.
- A history of a table the admin lists, as a filter on the model's list.
  The History tab filters by `?table=`.
- An admin write through a data source that also publishes to the model
  hook is recorded twice, once per source. `brocanth-admin-model` does not
  publish, so a generated project records it once.

## Settings

Section `[audit]`. Environment variable: `$<APP>_AUDIT__<KEY>`. Flag: `--audit.<key>`. Rename or hide a flag in `src/modules/audit.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `retention_days` | integer | `365` | `--audit.retention_days` | Days a change is kept. 0 keeps every change. |
| `flush_secs` | integer | `5` | `--audit.flush_secs` | Seconds between two writes of the queue. |
| `migrate` | bool | `true` | `--audit.migrate` | Create the brocanth_audit table on the first readiness check. |

## Read more

- [`crates/brocanth-audit/README.md`](../../crates/brocanth-audit/README.md)
- [`crates/brocanth-admin/src/journal.rs`](../../crates/brocanth-admin/src/journal.rs): the admin's journal
- Source: [`crates/brocanth-audit/src/scaffold.rs`](../../crates/brocanth-audit/src/scaffold.rs)
