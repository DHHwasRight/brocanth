//! WebSockets over a real socket: a message both ways, a stop that sends
//! close code 1001 and lets the drain finish, a cross-site page refused
//! before the switch, and a plain GET told to upgrade.
//!
//! The server is `Server::run` with a hand-written handler, so this tests
//! the transport alone. `brocanth-realtime`'s `tests/websocket.rs` puts the
//! broker behind it.

use std::net::SocketAddr;
use std::time::Duration;

use brocanth_server::websocket::{self, Handshake, Incoming};
use brocanth_server::{Request, Response, Server, Stopper};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Running {
    addr: SocketAddr,
    stopper: Stopper,
    task: tokio::task::JoinHandle<()>,
    gate: brocanth::plugin_body::StreamGate,
}

/// An echo server: each text message comes back upper-cased. Only
/// `https://friend.example` may open a socket from another site.
async fn echo() -> Running {
    let server = Server::bind("127.0.0.1:0")
        .await
        .expect("a port")
        .drain_grace(Duration::from_secs(5));
    let addr = server.local_addr().expect("addr");
    let stopper = server.stopper();
    let gate = server.streams().clone();
    let task = tokio::spawn(async move {
        server
            .run(|request: Request| async move {
                let allowed = ["https://friend.example"];
                let handshake = match Handshake::read(|name| request.header(name), &allowed) {
                    Ok(handshake) => handshake,
                    Err(refused) => return Response::text(refused.status(), refused.to_string()),
                };
                handshake.response(|mut socket| async move {
                    loop {
                        match socket.recv().await {
                            Incoming::Text(text) => {
                                if socket.send_text(text.to_uppercase()).await.is_err() {
                                    return;
                                }
                            }
                            Incoming::Binary(_) => {}
                            Incoming::Stopping => {
                                let _ = socket.send_text("bye").await;
                                return socket.close(websocket::GOING_AWAY, "stopping").await;
                            }
                            Incoming::Closed(_) => return,
                        }
                    }
                })
            })
            .await
            .expect("the server ran");
    });
    Running {
        addr,
        stopper,
        task,
        gate,
    }
}

async fn within<T>(what: &str, future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .unwrap_or_else(|_| panic!("timed out: {what}"))
}

#[tokio::test]
async fn a_message_goes_both_ways_and_a_close_frees_the_socket() {
    let server = echo().await;
    let mut client = websocket::connect(
        server.addr,
        "/echo",
        Some(&format!("http://{}", server.addr)),
    )
    .await
    .expect("same origin is allowed");

    client.send_text("hello").await.expect("sent");
    assert_eq!(
        within("the echo", client.recv()).await,
        Incoming::Text("HELLO".into())
    );
    assert_eq!(server.gate.open_streams(), 1, "the socket is on the gate");

    client.close(websocket::NORMAL, "done").await;
    within("the socket leaves the gate", async {
        while server.gate.open_streams() != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    server.stopper.stop();
    within("the server stops", server.task)
        .await
        .expect("joined");
}

#[tokio::test]
async fn stopping_the_server_sends_going_away_and_the_drain_ends() {
    let server = echo().await;
    let mut client = websocket::connect(server.addr, "/echo", None)
        .await
        .expect("no Origin is not a browser, so it passes");
    client.send_text("up").await.expect("sent");
    assert_eq!(
        within("the echo", client.recv()).await,
        Incoming::Text("UP".into())
    );

    server.stopper.stop();
    assert_eq!(
        within("the last message", client.recv()).await,
        Incoming::Text("bye".into())
    );
    assert_eq!(
        within("the close frame", client.recv()).await,
        Incoming::Closed(Some(websocket::GOING_AWAY))
    );
    // The drain waits for the socket and not for the grace period.
    within("the server stops", server.task)
        .await
        .expect("joined");
    assert_eq!(server.gate.open_streams(), 0);
}

#[tokio::test]
async fn a_page_on_another_site_is_refused_before_the_switch() {
    let server = echo().await;
    let refused = websocket::connect(server.addr, "/echo", Some("https://evil.example"))
        .await
        .expect_err("a cross-site socket");
    assert_eq!(refused.status, Some(403));
    assert!(refused.message.contains("help: "), "{}", refused.message);
    assert_eq!(server.gate.open_streams(), 0, "nothing was switched");

    websocket::connect(server.addr, "/echo", Some("https://friend.example"))
        .await
        .expect("an allowed origin");
    server.stopper.stop();
    within("the server stops", server.task)
        .await
        .expect("joined");
}

#[tokio::test]
async fn a_plain_get_is_told_to_upgrade() {
    let server = echo().await;
    let mut tcp = tokio::net::TcpStream::connect(server.addr)
        .await
        .expect("connect");
    tcp.write_all(b"GET /echo HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n")
        .await
        .expect("write");
    let mut answer = String::new();
    within("the answer", tcp.read_to_string(&mut answer))
        .await
        .expect("read");
    assert!(answer.starts_with("HTTP/1.1 426"), "{answer}");
    server.stopper.stop();
    within("the server stops", server.task)
        .await
        .expect("joined");
}
