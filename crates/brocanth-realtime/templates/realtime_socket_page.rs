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
//! `authorise`, in `src/plugins/realtime/authorise.rs`. The loop is
//! `crate::plugins::realtime_websocket`.

use brocanth_server::websocket::Handshake;

use crate::pages::{Ctx, Error, IntoResponse, Response};
use crate::plugins::realtime::authorise::{authorise, topics, value, viewer};
use crate::plugins::realtime::Broadcast;
use crate::plugins::realtime_websocket::{self, Filed};

/// Sites other than this host whose pages may open a socket, spelled as a
/// browser sends its `Origin`: `"https://app.example.com"`.
const ALLOWED_ORIGINS: &[&str] = &[];

pub fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let handshake = Handshake::read(|name| ctx.header(name), ALLOWED_ORIGINS)
        .map_err(|refused| Error::new(refused.status(), refused.to_string()))?;
    let viewer = viewer(ctx);
    let mut subscriptions = Vec::new();
    for topic in topics(&ctx.query).map_err(|why| Error::new(400, why))? {
        let name = topic.to_string();
        let subscription = authorise(&viewer, topic).map_err(|why| Error::new(403, why))?;
        subscriptions.push((name, subscription));
    }
    let broker = &ctx
        .provided::<Broadcast, _>()
        .ok_or_else(|| Error::internal("the realtime plugin put no broker on the context"))?
        .broker;

    // At the cap the broker refuses rather than evicting somebody.
    let client = broker
        .connect()
        .map_err(|e| Error::new(503, e.to_string()).with_header("retry-after", "5"))?;
    let mut filed = Filed::new();
    for (name, subscription) in subscriptions {
        if filed.contains_key(&name) {
            continue;
        }
        let subscribed = client
            .subscribe(subscription)
            .map_err(|e| Error::new(400, e.to_string()))?;
        filed.insert(name, subscribed.id);
    }

    // A browser that reconnects sends the id of the last frame it read. The
    // broker keeps no history to replay, so the honest answer to "did I miss
    // anything" is yes whenever a frame was published since. Read after the
    // subscriptions exist: a frame published before this read reached no
    // mailbox, and this read then counts it as missed. An id this broker
    // cannot compare (another process, or not a number) is missed too.
    let missed = value(&ctx.query, "last")
        .is_some_and(|seen| seen.trim().parse::<u64>().ok() != Some(broker.last_frame_id()));

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
