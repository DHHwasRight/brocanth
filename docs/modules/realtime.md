# `realtime`

One write is a map lookup, not a query per subscriber. `realtime-sse` carries it to a browser.

The wizard calls it: *A broadcast broker*.

| | |
|---|---|
| Categories | **Enhancer**, Middleware |
| Wizard question | Behaviour |
| Default | off |
| Presets | none |
| Part of | nothing |
| Needs | none |
| After | [`config`](config.md) |
| Adapts to | [`config`](config.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-realtime` |
| Without it | no broker; a write tells nobody, and a client finds out by asking again |

## What it writes

Files:

- `src/plugins/realtime/mod.rs`

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs (the context): `let realtime: &crate::plugins::realtime::Broadcast = cx.extract();`
- src/plugins/mod.rs: `pub mod realtime;`
- src/plugins/mod.rs (the provider list): `realtime crate::plugins::realtime::Realtime`
- config/<env>.toml: `[realtime]`

## Settings

Section `[realtime]`. Environment variable: `$<APP>_REALTIME__<KEY>`. Flag: `--realtime.<key>`. Rename or hide a flag in `src/modules/realtime.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `max_clients` | integer | `1000` | `--realtime.max_clients` | How many connections the broker holds. |
| `max_subscriptions_per_client` | integer | `64` | `--realtime.max_subscriptions_per_client` | How many topics one client may hold. |
| `queue` | integer | `32` | `--realtime.queue` | How many frames a mailbox holds before the overflow policy fires. |
| `overflow` | text | `"drop-oldest"` | `--realtime.overflow` | What a full mailbox does: drop-oldest, drop-newest or disconnect. |

## Read more

- [brocanth-realtime](../../crates/brocanth-realtime/README.md)
- Source: [`crates/brocanth-realtime/src/scaffold.rs`](../../crates/brocanth-realtime/src/scaffold.rs)
