# `webhooks-in`

`POST /webhooks/{source}`: signature, replay window and idempotency key, then your typed handler.

The wizard calls it: *Receive webhooks: signature, replay window, idempotency key*.

| | |
|---|---|
| Categories | **Middleware** |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | [`webhooks`](webhooks.md), ticked with its parent |
| Needs | none |
| After | [`db`](db.md), [`config`](config.md) |
| Adapts to | [`config`](config.md), [`db`](db.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-webhooks` (feature `postgres` with `db`), `serde` |
| Without it | no incoming webhooks; a sender's POST meets the CSRF check and is refused |

## What it does

A delivery is a `POST` to `{path}/{source}`. A request hook answers it in
this order:

1. Refuse another verb (`405`) and a source the application did not name
   (`404`).
2. Refuse a body larger than `max_bytes` (`413`).
3. Check the signature with the source's `Scheme`, and refuse a signed
   timestamp outside the replay window, `tolerance` seconds (`401`). The
   comparison takes constant time.
4. Decode the body into the handler's event type (`400`).
5. Claim the idempotency key. A key that already ran gets `200` and
   `already received`, and the handler does not run.
6. Run the handler. `Err` gives the key back and answers the rejection's
   status, `500` by default, so the sender's retry runs it again.

Schemes: `Scheme::standard` (Standard Webhooks, svix, and what
`webhooks-out` sends), `Scheme::stripe`, `Scheme::github` and
`Scheme::hmac_sha256` (one header with a hex MAC of the body). A scheme with
no timestamp has no replay window; only the idempotency key stops a replay.

**Why a hook before the CSRF check, not an exempt path:** an exempt path
lets every POST through, signed or not. The hook answers only for its own
path, and only a verified delivery reaches the handler. Nothing else under
the path skips the CSRF check.

**The idempotency keys:** with `db`, they are rows in the `db` module's
database (`brocanth_webhooks::PgSeen`), so every instance shares them and a
restart forgets none. Without `db`, they are in this process's memory.

## What it writes

Files:

- `src/plugins/webhooks_in/mod.rs`: the settings, an `example` source with
  a typed `ExampleEvent` and its handler, and the plugin that installs the
  crate's `Receiver`. Add a source in `configure`.
- `src/modules/webhooks_in.rs`.

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs (the provider list): `webhooks-in crate::plugins::webhooks_in::WebhooksIn`
- src/plugins/mod.rs (the hook record): `crate::plugins::webhooks_in::WebhooksIn`
- src/plugins/mod.rs: `pub mod webhooks_in;`
- config/<env>.toml: `[webhooks_in]`
- AGENTS.md: `## Incoming webhooks`

The generated file has tests: an unsigned delivery is refused, and, without
`db`, a delivery signed with the development secret runs once.

## Choices

| Choice | Default | What it does |
|---|---|---|
| hook `receive` | always on | Answers `POST {path}/{source}` before the CSRF check. The module is this hook. |

## Settings

Section `[webhooks_in]`, the plugin's instance name, not the module key. Environment variable: `$<APP>_WEBHOOKS_IN__<KEY>`. Flag: `--webhooks_in.<key>`. Rename or hide a flag in `src/modules/webhooks_in.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `path` | text | `"/webhooks"` | `--webhooks_in.path` | Where deliveries arrive: `{path}/{source}`. |
| `tolerance` | integer | `300` | `--webhooks_in.tolerance` | Seconds a signed timestamp may be off: the replay window. |
| `max_bytes` | integer | `1048576` | `--webhooks_in.max_bytes` | The largest delivery accepted, in bytes. |
| `secret` | text | `"whsec_..."` (a development secret) | none (secret) | The `example` source's Standard Webhooks secret. |
| `migrate` | bool | `true` | `--webhooks_in.migrate` | With `db` only. Create the idempotency table on the first readiness check. |

## Read more

- [`webhooks`](webhooks.md), [`webhooks-out`](webhooks-out.md)
- [brocanth-webhooks](../../crates/brocanth-webhooks/README.md)
- Source: [`crates/brocanth-webhooks/src/scaffold.rs`](../../crates/brocanth-webhooks/src/scaffold.rs), [`crates/brocanth-webhooks/src/incoming.rs`](../../crates/brocanth-webhooks/src/incoming.rs)
