# `jobs-memory`

The queue in this process's memory; for development and tests, gone at restart.

| | |
|---|---|
| Categories | **Enhancer** |
| Wizard question | Behaviour |
| Default | off |
| Presets | none |
| Part of | [`jobs`](jobs.md), off until you tick it |
| Needs | none |
| After | none |
| Adapts to | none |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-jobs` (feature `memory`) |
| Development only | yes: `brocanth check --deploy` reports `deploy/development-only` |

## What it writes

Files: none. The `jobs` module reads this key and writes
`Queue::memory()` in `src/plugins/jobs/mod.rs`, and drops the `url` and
`migrate` settings from `[jobs]`.

Slots:

- AGENTS.md: a section that says the jobs do not survive a restart.

## Why it exists

A laptop with no Postgres can run a project that has jobs. The memory
backend passes the same conformance suite as Postgres
(`brocanth-queue-conformance`), so a job that works here works there.

It loses every waiting and every dead job when the process stops, and two
processes do not share it. So `brocanth check --deploy` fails on it. Run
`brocanth remove jobs-memory` before you deploy: the queue then uses
Postgres again.

## Settings

None.

## Read more

- [`jobs`](jobs.md)
- [brocanth-jobs](../../crates/brocanth-jobs/README.md)
- [The queue design, Part 6](../../crates/brocanth-queue/docs/DECISIONS.md)
- Source: [`crates/brocanth-jobs/src/scaffold.rs`](../../crates/brocanth-jobs/src/scaffold.rs)
