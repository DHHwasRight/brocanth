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
| After | [`config`](config.md), [`db`](db.md) |
| Adapts to | [`config`](config.md), [`db`](db.md), [`jobs-memory`](jobs-memory.md), [`admin`](admin.md) |
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
- the admin's panel list, with `admin`: `JobsPanel::installed()`

The generated job derives `serde`, so the module declares `serde` itself. With `config` too, the project still gets one `serde` line, with both modules' features.

## Settings

Section `[jobs]`. Environment variable: `$<APP>_JOBS__<KEY>`. Flag: `--jobs.<key>`. Rename or hide a flag in `src/modules/jobs.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `url` | text | `"postgres://localhost/development"` | none (secret) | The database the queue lives in. Absent with `db` or `jobs-memory`. |
| `migrate` | bool | `true` | `--jobs.migrate` | Create the queue's tables at startup if they are absent. Absent with `jobs-memory`. |
| `worker` | bool | `true` | `--jobs.worker` | Run a worker in this process. |
| `queues` | list | `["default"]` | none (a list) | Which queues the worker claims from. |
| `concurrency` | integer | `5` | `--jobs.concurrency` | How many jobs the worker runs at once. |

## Where the jobs live

The jobs run on the `brocanth-queue` engine (`brocanth-queue/docs/DECISIONS.md`, Part 6).

- With `db`: the queue uses the pool the `db` plugin opened (`Queue::from_pool`). One set of connections, and `[jobs]` has no `url`.
- Without `db`: the queue opens its own pool from `[jobs] url`.
- With [`jobs-memory`](jobs-memory.md): the queue lives in the process (`Queue::memory()`). For development and tests only.

## Choices

Declared in the manifest. `src/modules/jobs.rs` turns them on or off.

| Choice | Kind | Default | What it does |
|---|---|---|---|
| `admin` | integration | on | A tab at `/admin/jobs`: the queues, what runs, the retries and the dead letter (`JobsPanel`). |

## Read more

- [brocanth-jobs](../../crates/brocanth-jobs/README.md)
- Source: [`crates/brocanth-jobs/src/scaffold.rs`](../../crates/brocanth-jobs/src/scaffold.rs)
