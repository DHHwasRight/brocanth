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
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-realtime` |
| Without it | no broker; a write tells nobody, and a client finds out by asking again |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/realtime.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | [`realtime-sse`](realtime-sse.md): it writes the one `authorise` both transports share; [`realtime-websocket`](realtime-websocket.md): it writes the one `authorise` both transports share |
| Contributes | none |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | `[realtime]`: `max_clients`, `max_subscriptions_per_client`, `queue`, `overflow` |
<!-- end generated: capabilities -->

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
