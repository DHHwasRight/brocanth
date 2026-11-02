# `realtime-websocket`

A duplex socket per subscriber, frames both ways.

| | |
|---|---|
| Categories | Routes |
| Wizard question | Behaviour |
| Default | off |
| Presets | none |
| Part of | [`realtime`](realtime.md), off until you tick it |
| Needs | none |
| After | none |
| Adapts to | none |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | none |
| Blocked | an upgrade out of the request/response shape, plus a frame codec |

## What it writes

Nothing. The module is blocked, so `create` never calls it.

## Settings

None.

## Read more

- [brocanth-realtime](../../crates/brocanth-realtime/README.md)
- Source: [`crates/brocanth-realtime/src/scaffold.rs`](../../crates/brocanth-realtime/src/scaffold.rs)
