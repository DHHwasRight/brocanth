# `realtime-websocket`

A duplex socket per subscriber, frames both ways.

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
| Crates | `brocanth-server` (feature `websocket`), `brocanth-events` |

## What it writes

Files:

- `src/plugins/realtime_websocket/mod.rs`: `run`, the loop over one
  socket (frames out, `subscribe` and `unsubscribe` in, a ping every
  `HEARTBEAT`, a close frame on shutdown), and `Authorise`.
- `src/pages/realtime/socket.rs`: `GET /realtime/socket?topics=posts/*`.
  It checks the handshake and the `Origin`, authorises each topic, opens the
  broker's `Client`, and answers 101. `PUBLIC` and `ALLOWED_ORIGINS` are
  empty, so every topic is refused (403) until you list a collection or
  gate it by a policy, and only pages on this host may open a socket.

Slots:

- src/plugins/mod.rs: `pub mod realtime_websocket;`
- src/pages/mod.rs (the table's test): `/realtime/socket`
- AGENTS.md: the section "Realtime over a WebSocket"

`Cargo.toml`: the `brocanth-server` line gains the `websocket` feature
(the handshake, the Origin check and the frame codec).

## What goes over the socket

The server sends one text message per record, in the server-sent-events
format, so one parser reads both transports:

| Message | When |
|---|---|
| `event:lagged` / `data:` | first, when `?last=<id>` is not the broker's last frame id |
| `id:<n>` / `event:<topic>` / `data:..` | each frame the broker addressed to this client |
| `event:lagged` / `data:<n>` | the mailbox was full and the overflow policy dropped `n` frames |
| `event:subscribed` / `data:<topic>` | the answer to `subscribe <topic>` |
| `event:unsubscribed` / `data:<topic>` | the answer to `unsubscribe <topic>` |
| `event:refused` / `data:<topic>: <why>` | a command that was not obeyed |

The browser sends `subscribe <topic>` and `unsubscribe <topic>`. Each
topic passes the same `authorise` as the topics in the URL. A message
larger than 64 KiB ends the connection.

| Close code | Why |
|---|---|
| 1001 | the server is stopping; the frames already queued go first |
| 1008 | a binary message |
| 1013 | `[realtime] overflow = "disconnect"` fired: the client fell behind |

A client that leaves, or does not answer a ping by the next one, ends the
loop. That drops the broker's `Client`: its subscriptions leave the index
and its mailbox closes.

## Refused before the switch

| Status | Why |
|---|---|
| 426 | not a WebSocket handshake, or not version 13 |
| 403 | the `Origin` is another site, or a topic in the URL is not allowed |
| 503 | the broker is at `max_clients`; `retry-after: 5` |

The `Origin` check is the CSRF check of a socket. See
[`docs/realtime.md`](../realtime.md) §6.

## Settings

None. Change `HEARTBEAT` in `src/plugins/realtime_websocket/mod.rs`, and
the lists in `src/pages/realtime/socket.rs`.

## Read more

- [Realtime transports](../realtime.md): the design of the upgrade, the
  codec, the shutdown and the Origin check
- [brocanth-realtime](../../crates/brocanth-realtime/README.md)
- Source: [`crates/brocanth-realtime/src/scaffold.rs`](../../crates/brocanth-realtime/src/scaffold.rs)
  and the two files it writes, in
  [`crates/brocanth-realtime/templates/`](../../crates/brocanth-realtime/templates/)
