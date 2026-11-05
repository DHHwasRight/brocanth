//! `realtime-websocket`: the broker's frames over a WebSocket, and commands
//! back.
//!
//! The transport is here, in the application, and not in
//! `brocanth-realtime`, because that crate does not depend on `brocanth`:
//! nothing in it touches a request, a response or a route.
//! `src/pages/realtime/socket.rs` is the route; it checks the handshake and
//! the Origin, authorises the first topics, and hands the socket to [`run`].
//!
//! # What the server sends
//!
//! One text message per record, in the same format as server-sent events,
//! so one parser reads both transports:
//!
//! ```text
//! id:12 event:posts/* data:..   each frame the broker addressed to this client
//! event:lagged data:3           the queue was full and 3 frames were dropped
//! event:lagged data:            first, when a reconnect may have missed frames
//! event:subscribed data:posts/42      the answer to `subscribe posts/42`
//! event:unsubscribed data:posts/42    the answer to `unsubscribe posts/42`
//! event:refused data:posts/42: why    a command that was not obeyed
//! ```
//!
//! A frame is the broker's bytes as it encoded them, shared by every
//! subscriber; nothing is encoded twice.
//!
//! # What the browser sends
//!
//! `subscribe <topic>` and `unsubscribe <topic>`, as text. Each topic passes
//! the same `authorise` as the topics in the URL. A binary message, or one
//! larger than `websocket::MAX_MESSAGE` (64 KiB), ends the connection.
//!
//! # Back-pressure
//!
//! The broker never waits for this socket. Each send waits until the frame
//! is written, so a browser that reads slowly holds this loop, the loop
//! stops taking from the mailbox, and `[realtime] overflow` decides what is
//! lost: `drop-oldest` and `drop-newest` drop frames and the browser reads
//! `lagged`; `disconnect` closes the mailbox, and this loop closes the
//! socket with code 1013.
//!
//! # Ending
//!
//! * The browser closes the socket, or stops answering pings for one
//!   `HEARTBEAT`. The loop ends, which drops the `Client`, which unsubscribes
//!   it and closes its mailbox.
//! * The server stops. The loop sends the frames already queued, then a
//!   close frame with code 1001, and ends, so a deploy does not wait for a
//!   subscriber. The browser reconnects with `?last=<id>`.

use std::collections::HashMap;
use std::time::Duration;

use brocanth_realtime::{Client, Event, Subscription, SubscriptionId, Topic};
use brocanth_server::websocket::{self, Incoming, WebSocket};
use tokio::time::{Instant, MissedTickBehavior};

/// How often the socket pings the browser. A browser that has not answered
/// by the next ping is gone, so this is also the longest a dead subscriber
/// holds its mailbox.
pub const HEARTBEAT: Duration = Duration::from_secs(15);

/// Who may follow what: the page's `authorise`, with what it read from the
/// request already captured.
pub type Authorise = Box<dyn Fn(Topic) -> Result<Subscription, String> + Send>;

/// The topics this socket follows, by name, so `unsubscribe` can find them.
pub type Filed = HashMap<String, SubscriptionId>;

/// Serves one socket until the browser leaves or the server stops.
///
/// `client` holds the subscriptions the page filed, and `filed` names them.
/// `missed` is true when the browser reconnected and frames were published
/// since the last one it read; the broker keeps no history, so the socket
/// says `lagged` instead.
pub async fn run(
    mut socket: WebSocket,
    client: Client,
    authorise: Authorise,
    mut filed: Filed,
    missed: bool,
) {
    if missed && socket.send_text(lagged(None)).await.is_err() {
        return;
    }
    let mut ticker = tokio::time::interval_at(Instant::now() + HEARTBEAT, HEARTBEAT);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            event = client.mailbox().recv() => match event {
                Some(event) => {
                    if socket.send_text(encoded(event)).await.is_err() {
                        return;
                    }
                }
                // This loop holds the client, so only the overflow policy
                // closes its mailbox: `disconnect` fired.
                None => {
                    return socket
                        .close(websocket::TRY_AGAIN_LATER, "too far behind; reconnect")
                        .await;
                }
            },
            incoming = socket.recv() => match incoming {
                Incoming::Text(command) => {
                    let answer = obey(&client, &authorise, &mut filed, &command);
                    if socket.send_text(answer).await.is_err() {
                        return;
                    }
                }
                Incoming::Binary(_) => {
                    return socket.close(websocket::POLICY, "send text commands").await;
                }
                Incoming::Stopping => {
                    for event in client.mailbox().drain() {
                        if socket.send_text(encoded(event)).await.is_err() {
                            return;
                        }
                    }
                    return socket
                        .close(websocket::GOING_AWAY, "the server is stopping")
                        .await;
                }
                Incoming::Closed(_) => return,
            },
            _ = ticker.tick() => {
                if socket.keepalive().await.is_err() {
                    return;
                }
            }
        }
    }
}

/// One command from the browser, obeyed, and the record that answers it.
fn obey(client: &Client, authorise: &Authorise, filed: &mut Filed, command: &str) -> String {
    let (verb, name) = command
        .trim()
        .split_once(' ')
        .unwrap_or((command.trim(), ""));
    let name = name.trim();
    let topic = match Topic::parse(name) {
        Ok(topic) => topic,
        Err(e) => return record("refused", &format!("{name}: {e}")),
    };
    match verb {
        "subscribe" if filed.contains_key(name) => record("subscribed", name),
        "subscribe" => {
            let subscription = match authorise(topic) {
                Ok(subscription) => subscription,
                Err(why) => return record("refused", &format!("{name}: {why}")),
            };
            match client.subscribe(subscription) {
                Ok(subscribed) => {
                    filed.insert(name.to_string(), subscribed.id);
                    record("subscribed", name)
                }
                Err(e) => record("refused", &format!("{name}: {e}")),
            }
        }
        "unsubscribe" => {
            if let Some(id) = filed.remove(name) {
                client.unsubscribe(id);
            }
            record("unsubscribed", name)
        }
        other => record(
            "refused",
            &format!(
                "{other:?} is not a command; send `subscribe <topic>` or `unsubscribe <topic>`"
            ),
        ),
    }
}

/// A record with no id: an event name and one data line.
fn record(event: &str, data: &str) -> String {
    let data = data.replace(['\r', '\n'], " ");
    format!("event:{event}\ndata:{data}\n\n")
}

/// `event:lagged`, with how many frames were dropped when that is known.
fn lagged(skipped: Option<u64>) -> String {
    match skipped {
        Some(n) => format!("event:lagged\ndata:{n}\n\n"),
        None => "event:lagged\ndata:\n\n".to_string(),
    }
}

/// What a mailbox event looks like on the wire.
fn encoded(event: Event) -> String {
    match event {
        // Encoded once by the broker and shared by every subscriber.
        Event::Frame(frame) => String::from_utf8_lossy(frame.bytes()).into_owned(),
        Event::Lagged { skipped } => lagged(Some(skipped)),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use brocanth_events::RowChange;
    use brocanth_realtime::{Broker, Gate};
    use brocanth_server::websocket::Handshake;
    use brocanth_server::{Request, Server, Stopper};

    use super::*;

    /// A server whose one route is this loop, with every topic public.
    async fn open(broker: &Arc<Broker>) -> (WebSocket, Stopper) {
        let server = Server::bind("127.0.0.1:0").await.expect("a port");
        let addr = server.local_addr().expect("an address");
        let stopper = server.stopper();
        let broker = Arc::clone(broker);
        tokio::spawn(server.run(move |request: Request| {
            let broker = Arc::clone(&broker);
            async move {
                let handshake =
                    Handshake::read(|name| request.header(name), &[]).expect("a handshake");
                let client = broker.connect().expect("room");
                let everyone: Authorise =
                    Box::new(|topic| Ok(Subscription::new(topic, Gate::everyone())));
                handshake.response(move |socket| run(socket, client, everyone, Filed::new(), false))
            }
        }));
        let socket = websocket::connect(addr, "/", None).await.expect("open");
        (socket, stopper)
    }

    async fn next(socket: &mut WebSocket) -> Incoming {
        tokio::time::timeout(Duration::from_secs(5), socket.recv())
            .await
            .expect("a message in time")
    }

    #[tokio::test]
    async fn a_subscribed_socket_reads_a_published_change_and_a_close_frees_it() {
        let broker = Broker::new();
        let (mut socket, stopper) = open(&broker).await;
        socket.send_text("subscribe posts/*").await.expect("sent");
        assert_eq!(
            next(&mut socket).await,
            Incoming::Text("event:subscribed\ndata:posts/*\n\n".into())
        );

        broker.publish_change(&RowChange::created("posts", 42i64));
        let Incoming::Text(frame) = next(&mut socket).await else {
            panic!("a frame");
        };
        assert!(frame.starts_with("id:1\nevent:posts/*\n"), "{frame}");
        assert!(frame.contains("data:id=42\n"), "{frame}");

        socket.close(websocket::NORMAL, "done").await;
        for _ in 0..500 {
            if broker.clients() == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(broker.clients(), 0, "the loop ended and dropped the client");
        stopper.stop();
    }

    #[tokio::test]
    async fn a_bad_command_is_refused_and_the_socket_stays_open() {
        let broker = Broker::new();
        let (mut socket, stopper) = open(&broker).await;
        socket.send_text("follow posts/*").await.expect("sent");
        let Incoming::Text(answer) = next(&mut socket).await else {
            panic!("an answer");
        };
        assert!(answer.starts_with("event:refused\n"), "{answer}");
        socket
            .send_text("unsubscribe posts/*")
            .await
            .expect("still open");
        assert_eq!(
            next(&mut socket).await,
            Incoming::Text("event:unsubscribed\ndata:posts/*\n\n".into())
        );
        stopper.stop();
    }

    #[tokio::test]
    async fn a_stopping_server_sends_going_away() {
        let broker = Broker::new();
        let (mut socket, stopper) = open(&broker).await;
        socket.send_text("subscribe posts/*").await.expect("sent");
        let _ = next(&mut socket).await;
        stopper.stop();
        assert_eq!(
            next(&mut socket).await,
            Incoming::Closed(Some(websocket::GOING_AWAY))
        );
    }
}
