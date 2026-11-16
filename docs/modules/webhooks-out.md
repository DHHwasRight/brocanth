# `webhooks-out`

Signed outgoing webhooks: endpoints and a delivery log in Postgres, retries through jobs.

The wizard calls it: *Send signed webhooks, retried through jobs*.

| | |
|---|---|
| Categories | **Enhancer** |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | [`webhooks`](webhooks.md), off until you tick it |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-webhooks` (feature `out`; `config` and `admin` with those modules), `serde_json` |
| Without it | no outgoing webhooks; a partner polls for what changed |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/webhooks_out.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | [`jobs`](jobs.md) |
| Provides | none |
| Requires | `PgPool` (a Postgres pool): the endpoints and the delivery log are rows in the `db` module's database; providers: [`db`](db.md) |
| Uses | none |
| Adapts to | none |
| Contributes | `ConfigCheck` `webhooks-out`: `deploy/migrate` for `[webhooks_out]`; `AgentsSection` `webhooks-out`: how to emit an event inside the caller's transaction; `AdminPanel` `webhooks`, on: a `webhooks` tab in the admin, with the endpoints, the deliveries and a Send again button |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | `[webhooks_out]`: `migrate`, `worker`, `concurrency`, `timeout_secs`, `allow_private`, `keep_days` |
<!-- end generated: capabilities -->

## What it does

An endpoint is a URL, a secret and the event patterns it wants
(`invoice.*`). `Outgoing::emit` sends an event to every active endpoint that
wants it: one row in the delivery log and one job per endpoint, written
together. The job posts the event, signed with Standard Webhooks. The queue
retries a failure with backoff, eight times over about a day.

```rust,ignore
let hooks = ctx.provided::<brocanth_webhooks::Outgoing, _>().expect("dispatched");
let endpoint = hooks
    .add_endpoint("https://partner.example/hooks", &["invoice.*"], "billing")
    .await?;
hooks.emit("invoice.paid", &serde_json::json!({ "id": 42 })).await?;
```

A row change becomes `<collection>.created`, `.updated` or `.deleted`
through `hooks.sink(&["invoices"])`, a `ChangeSink` the model layer writes
through. A change sink cannot fail the write. An event that must not be
lost is emitted by the code that wrote the row.

**Why a job per delivery:** the retries, the backoff and the dead letter
already exist in `brocanth-jobs`. The delivery row is the log a person
reads; the job is the attempt.

**Why `allow_private` is off in production:** whoever registers an endpoint
chooses its URL. Off, an endpoint on a loopback or private address is
refused, and so is a name that resolves only to such addresses when the
delivery connects. A webhook cannot reach your own network. The development file
turns it on, because the receiver is often on the same machine.

## What it writes

Files:

- `src/plugins/webhooks_out/mod.rs`: names the crate's plugin
  (`OutgoingPlugin`) and its settings (`OutgoingConfig`), with how a page
  uses them.
- `src/modules/webhooks_out.rs`.

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs: `pub mod webhooks_out;`
- config/<env>.toml: `[webhooks_out]`
- the admin panels, with `admin`: `brocanth_webhooks::admin::DeliveriesPanel`
- AGENTS.md: `## Outgoing webhooks`

## Choices

| Choice | Default | What it does |
|---|---|---|
| integration `admin` | on | A tab at `/admin/webhooks` with the endpoints, the deliveries and a Send again button. Without `admin` it does nothing. |

## Settings

Section `[webhooks_out]`. Environment variable: `$<APP>_WEBHOOKS_OUT__<KEY>`. Flag: `--webhooks_out.<key>`. Rename or hide a flag in `src/modules/webhooks_out.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `migrate` | bool | `true` | `--webhooks_out.migrate` | Create the two tables on the first readiness check. Off, for a project that keeps `brocanth_webhooks::Outgoing::SCHEMA` in a migration. |
| `worker` | bool | `true` | `--webhooks_out.worker` | Run the delivery worker in this process. |
| `concurrency` | integer | `4` | `--webhooks_out.concurrency` | How many deliveries run at once. |
| `timeout_secs` | integer | `10` | `--webhooks_out.timeout_secs` | Seconds an attempt may take. |
| `allow_private` | bool | `true` | `--webhooks_out.allow_private` | Accept an endpoint on a loopback or private address. The plugin's default is `false`. |

## Read more

- [`webhooks`](webhooks.md), [`webhooks-in`](webhooks-in.md), [`jobs`](jobs.md)
- [brocanth-webhooks](../../crates/brocanth-webhooks/README.md)
- Source: [`crates/brocanth-webhooks/src/scaffold.rs`](../../crates/brocanth-webhooks/src/scaffold.rs), [`crates/brocanth-webhooks/src/outgoing.rs`](../../crates/brocanth-webhooks/src/outgoing.rs)
