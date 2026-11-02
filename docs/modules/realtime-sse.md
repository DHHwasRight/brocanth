# `realtime-sse`

One long response per subscriber, frames written as they are made.

| | |
|---|---|
| Categories | **Routes** |
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
| Blocked | the core can stream now; this transport is simply not written yet |

## What it writes

Nothing. The module is blocked, so `create` never calls it.

## Settings

None.

## Read more

- [brocanth-realtime](../../crates/brocanth-realtime/README.md)
- Source: [`crates/brocanth-realtime/src/scaffold.rs`](../../crates/brocanth-realtime/src/scaffold.rs)
