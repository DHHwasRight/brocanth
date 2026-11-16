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
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-jobs` (feature `admin` with `admin`), `serde` (features `derive`) |
| Without it | no queue; slow work happens inside the request that asked for it |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/jobs.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | `JobQueue` (a job queue): the queue, on Postgres (SKIP LOCKED); in the process with `jobs-memory` |
| Requires | `PgPool` (a Postgres pool): the queue's tables live in the project's database, beside the pages' pool; providers: [`db`](db.md); not with [`jobs-memory`](jobs-memory.md) |
| Uses | none |
| Adapts to | [`jobs-memory`](jobs-memory.md): the queue lives in the process, and [jobs] has no migrate; [`observability`](observability.md): each job run gets a span, a log line and a counter from the `Observer`; [`observability-metrics`](observability-metrics.md): /metrics shows the queue's depth and the age of its oldest job |
| Contributes | `ConfigCheck` `jobs`: `deploy/migrate` for `[jobs]`, while the queue is on Postgres; `AdminPanel` `jobs`, on: a `jobs` tab in the admin: the queues, what runs, the retries and the dead letter |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | `[jobs]`: `migrate`, `worker`, `queues`, `concurrency` |
<!-- end generated: capabilities -->

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
