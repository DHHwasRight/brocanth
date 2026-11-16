# `jobs`

A queue in Postgres with retries and a dead letter; a worker in this process.

The wizard calls it: *Background jobs, with retries and a dead letter*.

| | |
|---|---|
| Categories | **Enhancer** |
| Wizard question | Behaviour |
| Default | off |
| Presets | Something that does work |
| Part of | nothing |
| Needs | none |
| Requires | `PgPool` (a Postgres pool). Without a provider chosen, `brocanth create` and `brocanth add` add [`db`](db.md) and say so; not with [`jobs-memory`](jobs-memory.md) |
| After | [`config`](config.md) |
| Adapts to | [`jobs-memory`](jobs-memory.md), [`observability`](observability.md), [`observability-metrics`](observability-metrics.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-jobs` (feature `admin` with `admin`), `serde` (features `derive`) |
| Without it | no queue; slow work happens inside the request that asked for it |

## What it writes

Files:

- `src/plugins/jobs/mod.rs`

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs: `pub mod jobs;`
- src/plugins/mod.rs (the provider list): `jobs crate::plugins::jobs::Jobs`
- config/<env>.toml: `[jobs]`
- src/plugins/mod.rs (admin_panels), when the admin collects tabs: `JobsPanel::late(queue)`, RBAC resource `jobs`

The generated job derives `serde`, so the module declares `serde` itself. With `config` too, the project still gets one `serde` line, with both modules' features.

## Settings

Section `[jobs]`. Environment variable: `$<APP>_JOBS__<KEY>`. Flag: `--jobs.<key>`. Rename or hide a flag in `src/modules/jobs.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `migrate` | bool | `true` | `--jobs.migrate` | Create the queue's tables at startup if they are absent. Absent with `jobs-memory`. |
| `worker` | bool | `true` | `--jobs.worker` | Run a worker in this process. |
| `queues` | list | `["default"]` | none (a list) | Which queues the worker claims from. |
| `concurrency` | integer | `5` | `--jobs.concurrency` | How many jobs the worker runs at once. |

## Where the jobs live

The jobs run on the `brocanth-queue` engine (`brocanth-queue/docs/DECISIONS.md`, Part 6).

- In Postgres: the queue requires `PgPool`, so `brocanth create --with jobs` adds `db` and prints `added db: jobs requires PgPool (a Postgres pool)`. The queue runs on the `db` database, through a small control pool of its own (`Queue::beside`), so a running job never holds the connection its heartbeat needs. `[jobs]` has no `url`.
- With [`jobs-memory`](jobs-memory.md): the queue lives in the process (`Queue::memory()`). For development and tests only.

## Choices

Declared in the manifest. `src/modules/jobs.rs` turns them on or off.

| Choice | Kind | Default | What it does |
|---|---|---|---|
| `admin` | integration | on | A tab at `/admin/jobs`: the queues, what runs, the retries and the dead letter (`JobsPanel`). |

## Read more

- [brocanth-jobs](../../crates/brocanth-jobs/README.md)
- Source: [`crates/brocanth-jobs/src/scaffold.rs`](../../crates/brocanth-jobs/src/scaffold.rs)
