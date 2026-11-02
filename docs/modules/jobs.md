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
| After | [`config`](config.md) |
| Adapts to | [`config`](config.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-jobs` |
| Without it | no queue; slow work happens inside the request that asked for it |

## What it writes

Files:

- `src/plugins/jobs/mod.rs`

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs: `pub mod jobs;`
- src/plugins/mod.rs (the provider list): `jobs crate::plugins::jobs::Jobs`
- config/<env>.toml: `[jobs]`

docs/module-flags.md (open question 2) records that `jobs` without `config` does not compile: the generated job derives `serde`, and `serde` is a dependency only with `config`.

## Settings

Section `[jobs]`. Environment variable: `$<APP>_JOBS__<KEY>`. Flag: `--jobs.<key>`. Rename a flag in `src/flags.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `url` | text | `"postgres://localhost/development"` | none (secret) | The database the queue lives in. |
| `migrate` | bool | `true` | `--jobs.migrate` | Create the three tables at startup if they are absent. |
| `worker` | bool | `true` | `--jobs.worker` | Run a worker in this process. |
| `queues` | list | `["default"]` | none (a list) | Which queues the worker claims from. |
| `concurrency` | integer | `5` | `--jobs.concurrency` | How many jobs the worker runs at once. |

## Read more

- [brocanth-jobs](../../crates/brocanth-jobs/README.md)
- Source: [`crates/brocanth-jobs/src/scaffold.rs`](../../crates/brocanth-jobs/src/scaffold.rs)
