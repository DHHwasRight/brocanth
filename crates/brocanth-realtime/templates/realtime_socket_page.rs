//! `GET /realtime/socket?topics=posts/*,posts/42` -- the broker's frames
//! over a WebSocket.
//!
//! ```js
//! const socket = new WebSocket(`ws://${location.host}/realtime/socket?topics=posts/*`);
//! socket.onmessage = (e) => console.log(e.data); // "id:1\nevent:posts/*\ndata:..."
//! socket.onopen = () => socket.send("subscribe posts/42");
//! ```
//!
//! Three checks run before the connection switches, and each refusal is a
//! status the browser can read:
//!
//! 1. The handshake: a WebSocket, version 13. Else 426.
//! 2. The `Origin`. A browser sends the site's cookies with a WebSocket from
//!    any page, and no CORS rule applies, so the Origin check is this
//!    route's CSRF check: the page must be on this host, or in
//!    `ALLOWED_ORIGINS`. Else 403.
//! 3. Every topic in the URL passes `authorise`, and one refusal refuses
//!    the socket. Else 403.
//!
//! A topic the browser asks for later (`subscribe <topic>`) passes the same
//! `authorise`. The loop is `crate::plugins::realtime_websocket`.

use brocanth_realtime::{Gate, Subscription, Topic};
use brocanth_server::websocket::Handshake;

use crate::pages::{Ctx, Error, IntoResponse, Response};
use crate::plugins::realtime::Broadcast;
use crate::plugins::realtime_websocket::{self, Filed};

/// Collections any visitor may follow: every row, every change.
///
/// Empty, so nothing is public until somebody says so here.
const PUBLIC: &[&str] = &[];

/// Sites other than this host whose pages may open a socket, spelled as a
/// browser sends its `Origin`: `"https://app.example.com"`.
const ALLOWED_ORIGINS: &[&str] = &[];

/// Who is asking: what `authorise` needs, read from the request once.
///
/// The socket outlives the request, so `authorise` cannot read the request
/// when the browser subscribes later. Put here what it decides on: the
/// session's user id, a role.
pub struct Viewer;

fn viewer(_ctx: &Ctx) -> Viewer {
    Viewer
}

/// Whether `viewer` may follow `topic`, and gated by what.
///
/// The one place a topic is authorised, and the place to edit. A public
/// collection takes `Gate::everyone()`. Rows that a policy decides take the
/// policy: `from_policy` gates `posts/*` with the list rule and `posts/42`
/// with the view rule, and the broker inverts the rule into an index key, so
/// a write is a map lookup and not a check per subscriber.
///
/// ```ignore
/// return Ok(Subscription::from_policy(topic, &PostPolicy, &viewer.user_id));
/// ```
fn authorise(_viewer: &Viewer, topic: Topic) -> Result<Subscription, String> {
    if PUBLIC.contains(&topic.collection_name()) {
        return Ok(Subscription::new(topic, Gate::everyone()));
    }
    Err(format!(
        "no rule lets this socket follow {topic}\n\
         help: add one to `authorise` in src/pages/realtime/socket.rs"
    ))
}

pub fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let handshake = Handshake::read(|name| ctx.header(name), ALLOWED_ORIGINS)
        .map_err(|refused| Error::new(refused.status(), refused.to_string()))?;
    let viewer = viewer(ctx);
    let mut subscriptions = Vec::new();
    for topic in topics(&ctx.query)? {
        let name = topic.to_string();
        let subscription = authorise(&viewer, topic).map_err(|why| Error::new(403, why))?;
        subscriptions.push((name, subscription));
    }
    let broker = &ctx
        .provided::<Broadcast, _>()
        .ok_or_else(|| Error::internal("the realtime plugin put no broker on the context"))?
        .broker;

    // A browser that reconnects sends the id of the last frame it read. The
    // broker keeps no history to replay, so the honest answer to "did I miss
    // anything" is yes whenever a frame was published since.
    let missed = value(&ctx.query, "last")
        .is_some_and(|seen| seen.trim().parse::<u64>().ok() != Some(broker.last_frame_id()));

    // At the cap the broker refuses rather than evicting somebody.
    let client = broker
        .connect()
        .map_err(|e| Error::new(503, e.to_string()).with_header("retry-after", "5"))?;
    let mut filed = Filed::new();
    for (name, subscription) in subscriptions {
        let subscribed = client
            .subscribe(subscription)
            .map_err(|e| Error::new(400, e.to_string()))?;
        filed.insert(name, subscribed.id);
    }

    let headers = handshake.headers().into_iter().collect();
    let rule: realtime_websocket::Authorise = Box::new(move |topic| authorise(&viewer, topic));
    let stream = handshake
        .switch(move |socket| realtime_websocket::run(socket, client, rule, filed, missed));
    Ok(Response {
        status: 101,
        headers,
        stream: Some(stream),
        ..Default::default()
    })
}

/// Every `topics=` in the query, split at commas and parsed. None is
/// allowed: the browser can subscribe after the socket opens.
fn topics(query: &str) -> Result<Vec<Topic>, Error> {
    let mut out = Vec::new();
    for pair in query.split('&') {
        let Some(("topics", value)) = pair.split_once('=') else {
            continue;
        };
        for text in decoded(value)?.split(',').filter(|t| !t.is_empty()) {
            let topic =
                Topic::parse(text).map_err(|e| Error::new(400, format!("topic {text:?}: {e}")))?;
            out.push(topic);
        }
    }
    Ok(out)
}

/// The first value of `name` in the query, as sent.
fn value<'q>(query: &'q str, name: &str) -> Option<&'q str> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value)
}

/// A query value with its `%XX` and `+` undone.
fn decoded(value: &str) -> Result<String, Error> {
    let bad = || {
        Error::new(
            400,
            format!("the query value {value:?} is not percent-encoded UTF-8"),
        )
    };
    let mut bytes = Vec::with_capacity(value.len());
    let mut rest = value.bytes();
    while let Some(byte) = rest.next() {
        bytes.push(match byte {
            b'+' => b' ',
            b'%' => {
                let hex = [rest.next().ok_or_else(bad)?, rest.next().ok_or_else(bad)?];
                let hex = std::str::from_utf8(&hex).map_err(|_| bad())?;
                u8::from_str_radix(hex, 16).map_err(|_| bad())?
            }
            other => other,
        });
    }
    String::from_utf8(bytes).map_err(|_| bad())
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::time::Duration;

    use brocanth_server::websocket::{self, Incoming};
    use brocanth_server::{Server, Stopper};

    use super::*;

    /// The whole application on a real port.
    async fn serve() -> (SocketAddr, Stopper) {
        let app = Arc::new(crate::app::build().expect("the application builds"));
        let server = Server::bind("127.0.0.1:0")
            .await
            .expect("a port")
            .with_streams(app.core.streams().clone());
        let addr = server.local_addr().expect("an address");
        let stopper = server.stopper();
        tokio::spawn(server.run(move |request| {
            let app = Arc::clone(&app);
            async move { crate::app::handle(&app, request).await }
        }));
        (addr, stopper)
    }

    #[test]
    fn topics_are_read_from_the_query_encoded_or_not() {
        let read = topics("topics=posts/*,posts%2F42&x=1").expect("two");
        let names: Vec<String> = read.iter().map(ToString::to_string).collect();
        assert_eq!(names, ["posts/*", "posts/42"]);
        assert!(topics("").expect("none is allowed").is_empty());
        assert!(topics("topics=posts").is_err(), "a topic has a slash");
        assert_eq!(value("a=1&last=7", "last"), Some("7"));
    }

    #[tokio::test]
    async fn a_socket_opens_and_a_topic_no_rule_allows_is_refused_over_it() {
        let (addr, stopper) = serve().await;
        let origin = format!("http://{addr}");
        let mut socket = websocket::connect(addr, "/realtime/socket", Some(&origin))
            .await
            .expect("this host may open a socket");
        socket.send_text("subscribe posts/*").await.expect("sent");
        let answer = tokio::time::timeout(Duration::from_secs(5), socket.recv())
            .await
            .expect("an answer in time");
        let Incoming::Text(answer) = answer else {
            panic!("a text answer, not {answer:?}");
        };
        assert!(
            answer.starts_with("event:refused\ndata:posts/*: "),
            "{answer}"
        );
        socket.close(websocket::NORMAL, "done").await;
        stopper.stop();
    }

    #[tokio::test]
    async fn a_page_on_another_site_is_refused_before_the_switch() {
        let (addr, stopper) = serve().await;
        let refused = websocket::connect(addr, "/realtime/socket", Some("https://evil.example"))
            .await
            .expect_err("a cross-site socket");
        assert_eq!(refused.status, Some(403));
        stopper.stop();
    }

    #[tokio::test]
    async fn a_topic_in_the_url_no_rule_allows_is_refused_before_the_switch() {
        let (addr, stopper) = serve().await;
        let refused = websocket::connect(addr, "/realtime/socket?topics=posts/*", None)
            .await
            .expect_err("no rule allows posts");
        assert_eq!(refused.status, Some(403));
        stopper.stop();
    }
}
