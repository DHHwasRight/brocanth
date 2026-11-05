# Realtime transports

`brocanth-realtime` is the broker. It makes encoded frames and stops one
layer short of the socket (PLAN.md §10). Two modules carry the frames to a
browser:

| Module | Route | Direction | Built on |
|---|---|---|---|
| `realtime-sse` | `GET /realtime/events` | server to browser | a streaming body (G1) |
| `realtime-websocket` | `GET /realtime/socket` | both ways | an upgrade, and a frame codec |

This page is the design of the second one. SSE is described in
`crates/brocanth-realtime/src/lib.rs` and `docs/framework-gaps.md` §G1.

## 1. The problem

A WebSocket starts as a `GET` and is answered `101 Switching Protocols`.
After the headers the connection is not HTTP any more. Every exit from
`brocanth-server` was a response with a body, so a page had no way to say
"switch, and then give me the connection". The module was blocked on that,
and on a frame codec.

## 2. How a page answers with an upgrade

**Decision: the upgrade is a `Streaming` that holds no bytes.**

- The core (`brocanth::plugin_body`) has `Upgrade`, an opaque
  `Box<dyn Any + Send>`, and `Streaming::upgrade(Upgrade)`. The core has no
  runtime and no socket type, so it carries the value and does not read it.
- `brocanth-server` puts its own callback in it:
  `upgrade::switch(|socket| async { .. })`. The page answers status 101 with
  that as its stream. For an application on `Server::run`,
  `Response::upgrade(protocol, on_socket)` builds the whole 101.
- The server takes the `OnUpgrade` from hyper before it reads the request.
  When the answer is a 101 with an upgrade, it writes the headers, waits for
  hyper to hand over the connection, and runs the callback on a task of its
  own with an `upgrade::Socket` (tokio `AsyncRead` + `AsyncWrite`). hyper's
  types stay inside the crate.
- A 101 with no connection to switch (HTTP/2, a test with no socket) and an
  upgrade under another status are answered 500. Saying 101 anyway would
  leave the client with a dead connection.

**Why a stream, and not a new `Body` variant or a new field.** A generated
project's `pages::Response` already has `stream: Option<Streaming>`, and the
bridge, the hooks and `from_core` already carry a stream untouched. A new
`Body` variant would break every exhaustive match on `Body`. A new field on
`pages::Response` would change the response type of every project, with or
without a socket. The stream form changes neither. The cost is a flag on
`Streaming` and one defaulted trait method, `BodyStream::take_upgrade`. The
flag sits beside the box, so `Body`'s destructor stays one indirect call
(`tests/hooks_cost_nothing.rs`).

## 3. Hooks

The response hooks see a 101 like any streaming response:

- `ResponseHook::on_headers` runs. A security-headers or CORS hook can add
  a header to the 101.
- `ResponseHook::on_body` does not run: there are no bytes.
- `Streaming::wrap` does nothing on an upgrade. A compressing hook cannot
  put itself around a socket, and a wrapper that does not forward
  `take_upgrade` cannot lose it.
- `Streaming::watch` does nothing on an upgrade. The server registers the
  socket itself, for as long as the callback runs (section 4).
- The bridge does not add its default `content-type: text/html` to a 101.

The request hooks run before the page as for any `GET`. The CSRF hook does
not check a `GET`, so the Origin check (section 6) is the CSRF check of a
socket.

## 4. Shutdown (ADR 2)

An upgraded connection is not hyper's any more, so hyper's graceful drain
does not wait for it. The server registers each socket with its
`StreamGate` instead: one `StreamTicket` per socket, dropped when the
callback returns. `open_streams` counts sockets with streams.

1. `StreamGate::close` (the listener stops, or `App::shutdown` phase
   `Begin`) wakes each socket. `StreamTicket::poll_closing` is new in the
   core for this; `Socket::closing` and `WebSocket::recv` read it. `recv`
   returns `Incoming::Stopping` once.
2. The generated loop sends the frames already queued, then a close frame
   with code 1001 (going away), and returns. The browser reconnects.
3. `Server::run` waits for hyper's connections and then for
   `StreamGate::drained`, both inside `drain_grace`. A socket that ignores
   the stop holds the drain until the grace runs out, as a stream without
   `finish` does.

## 5. The frame codec

**Decision: `tokio-tungstenite` 0.30, `default-features = false`, feature
`handshake`, behind `brocanth-server`'s `websocket` feature (off).**

- RFC 6455 is small to state and easy to get wrong: masking, fragments,
  control frames between fragments, the close handshake, UTF-8 checked
  across fragments, and limits on all of it. tungstenite passes the
  Autobahn suite and is maintained. A hand-written codec would be about 600
  lines with weaker tests.
- `default-features = false` keeps out the client connector and every TLS
  stack. The server brings the socket; the codec frames bytes.
- `handshake` adds the `Sec-WebSocket-Accept` digest (`sha1`,
  `data-encoding`) and the client handshake that tests use. `http` and
  `httparse` are in the tree already. `futures-util` is in the tree through
  tokio-tungstenite already.
- The feature is off by default, so an application without a socket links
  no codec (PLAN.md §8). `realtime-websocket` turns it on; the CLI merges
  the feature into the one `brocanth-server` line of `Cargo.toml`.
- tungstenite's types do not appear in the public API. `websocket::WebSocket`
  has `recv`, `send_text`, `send_binary`, `keepalive` and `close`, and
  `Incoming` is `Text`, `Binary`, `Stopping` or `Closed(code)`. Pings and
  pongs are answered inside.

## 6. Authentication, CSRF and the Origin

A browser sends the site's cookies with a WebSocket handshake from **any**
page, and the same-origin policy does not apply to it. Without a check,
any site can open a socket as the signed-in user (cross-site WebSocket
hijacking).

`websocket::Handshake::read(header, allowed)` checks the protocol and the
`Origin` together, so a page cannot do one and forget the other:

- No `Origin`: allowed. Only a browser carries the user's cookies, and a
  browser always sends an `Origin`.
- An `Origin` whose host and port equal the `Host` header: allowed. The
  scheme is not compared: behind a TLS proxy the server sees `http` and the
  browser says `https`.
- An `Origin` in `allowed` (the generated `ALLOWED_ORIGINS`): allowed.
- Everything else, `null` included: 403, with a `help:` line.

Authentication is the page's, before the switch: the session reads as on
any `GET`. The generated page reads what `authorise` needs into a `Viewer`
once, because the socket outlives the request and later `subscribe`
commands have no request to read. Each topic, in the URL or in a command,
passes the same `authorise`. A refusal before the switch is a status the
browser can read (403, 426, 503 with `retry-after`); a refusal after it is
a `refused` record.

## 7. Back-pressure

- The broker never waits for a socket. Each mailbox is bounded, and
  `[realtime] overflow` decides what a full one does.
- `WebSocket::send_text` waits until the frame is written. A browser that
  reads slowly fills the TCP buffer, the send waits, and the loop stops
  taking from the mailbox. The overflow policy then drops frames
  (`lagged`), or closes the mailbox (`disconnect`), and the loop closes the
  socket with code 1013 (try again later).
- The codec's write buffer is capped at 1 MiB, and a client message at
  64 KiB (`websocket::MAX_MESSAGE`, `Handshake::max_message`). A larger
  message ends the connection.
- `keepalive` pings every 15 seconds. A browser that has not answered by the
  next ping is gone, and the loop ends, which frees its client.

## 8. What goes over the socket

The server sends one text message per record, in the server-sent-events
format, so one parser reads both transports and the broker encodes each
frame once for every subscriber:

```text
id:12 event:posts/* data:..         a frame the broker addressed to this client
event:lagged data:3                 3 frames were dropped
event:lagged data:                  first, after a reconnect that missed frames
event:subscribed data:posts/42      the answer to `subscribe posts/42`
event:unsubscribed data:posts/42    the answer to `unsubscribe posts/42`
event:refused data:posts/42: why    a command that was not obeyed
```

The browser sends `subscribe <topic>` and `unsubscribe <topic>` as text. A
binary message closes the socket with code 1008. A reconnect sends
`?last=<id>`, the id of the last frame it read; the broker keeps no history,
so the socket says `lagged` when frames were published since.

Close codes: 1000 normal, 1001 the server is stopping, 1008 a message the
socket does not take, 1013 the client fell behind.

## 9. Tests

- `crates/brocanth/src/plugin_body.rs`: an upgrade survives a wrap and a
  watch and comes out once; a ticket reports the close it was parked for.
- `crates/brocanth-server/src/lib.rs`: a 101 with nothing to switch is a 500.
- `crates/brocanth-server/src/websocket.rs`: the RFC 6455 accept key, the
  protocol refusals, the Origin rule.
- `crates/brocanth-server/tests/websocket.rs`, over a real port: a message
  both ways and a close that frees the socket; a stop that sends 1001 and a
  drain that ends; a cross-site page refused with 403 before the switch; a
  plain `GET` answered 426.
- `crates/brocanth-realtime/tests/websocket.rs` compiles the generated loop
  (`templates/realtime_websocket.rs`) and runs its tests over a real port:
  subscribe, read a published change, close, and the broker frees the
  client; a bad command is refused and the socket stays open; a stop sends
  1001.
- The golden shape `realtime-websocket`, built and tested by
  `the_golden_shapes_build_and_pass_their_own_tests`: the generated page
  opens a socket through the whole application, refuses a topic over it,
  and refuses a cross-site page and an unauthorised topic before the switch.

## 10. Open

- **HTTP/2.** RFC 8441 (WebSockets over an HTTP/2 `CONNECT`) is not
  supported. A browser falls back to HTTP/1.1 for a WebSocket, so this
  costs nothing today.
- **No `permessage-deflate`.** Frames are small, and compression over a
  socket that carries a secret beside attacker-chosen text is the CRIME
  shape.
- **No subprotocol negotiation** (`Sec-WebSocket-Protocol`). The format in
  section 8 is the only one.
- **A proxy that rewrites `Host`** makes every same-site `Origin` look
  cross-site. List the public origin in `ALLOWED_ORIGINS`. Reading
  `X-Forwarded-Host` instead would trust a header a client can send.
- **Commands are not rate limited.** Each command is a map operation, and
  `max_subscriptions_per_client` caps what they can hold.
