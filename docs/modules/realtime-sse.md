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
| Crates | `brocanth-events` |

## What it writes

Files:

- `src/plugins/realtime_sse/mod.rs`: `Frames`, a `BodyStream` over one
  client's mailbox, and `HEARTBEAT`.
- `src/pages/realtime/events.rs`: `GET /realtime/events?topics=posts/*,posts/42`,
  and `authorise`, the one place a topic is allowed. `PUBLIC` is empty, so
  every topic is refused (403) until you list a collection or gate it by a
  policy.

Slots:

- src/plugins/mod.rs: `pub mod realtime_sse;`
- src/pages/mod.rs (the table's test): `/realtime/events`
- AGENTS.md: the section "Realtime over server-sent events"

## What the stream writes

| Line | When |
|---|---|
| `: connected <client id>` | first, so the headers leave at once |
| `event:lagged` / `data:` | first, when a reconnect's `Last-Event-ID` is not the broker's last frame id |
| `id:<n>` / `event:<topic>` / `data:..` | each frame the broker addressed to this client |
| `event:lagged` / `data:<n>` | the mailbox was full and the overflow policy dropped `n` frames |
| `:` | a heartbeat, after 15 s without a frame |
| `event:close` / `data:` | the server is stopping; queued frames go first |

A client that disconnects drops the stream, which drops the broker's
`Client`: its subscriptions leave the index and its mailbox closes. The
server learns that a socket is gone only on a write, so the heartbeat
bounds how long a dead subscriber holds its mailbox. `[realtime] overflow =
"disconnect"` ends the stream; the browser reconnects and is told `lagged`.

## Why the transport is generated

`brocanth-realtime` does not depend on `brocanth`, because nothing in it
touches a request, a response or a route. A transport is a response body,
and a body belongs to the crate that owns the routes: the application.
`brocanth-server` does not get it either, because that would draw an arrow
from the HTTP transport to one capability crate. The application already
depends on all three, and the authorisation of a topic is its decision.

## Settings

None. Change `HEARTBEAT` in `src/plugins/realtime_sse/mod.rs`.

## Read more

- [brocanth-realtime](../../crates/brocanth-realtime/README.md)
- Source: [`crates/brocanth-realtime/src/scaffold.rs`](../../crates/brocanth-realtime/src/scaffold.rs)
