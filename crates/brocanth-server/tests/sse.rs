//! `sse::channel` over a real socket: events arrive as they are sent, a
//! client that leaves closes the sender, and a stopping server ends the
//! stream and closes the sender too.
//!
//! The handler parks the sender in a slot the test reads, so the test is the
//! producer and can watch what the producer sees.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use brocanth_server::sse::{self, Closed, Event, Sender};
use brocanth_server::{Request, Response, Server, Stopper};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

type Slot = Arc<Mutex<Option<Sender>>>;

struct Running {
    addr: String,
    stopper: Stopper,
    task: tokio::task::JoinHandle<()>,
    sender: Slot,
}

impl Running {
    async fn start(heartbeat: Option<Duration>) -> Self {
        let server = Server::bind("127.0.0.1:0").await.expect("a port");
        let addr = server.local_addr().expect("addr").to_string();
        let stopper = server.stopper();
        let sender: Slot = Arc::default();
        let slot = Arc::clone(&sender);
        let task = tokio::spawn(async move {
            server
                .run(move |_: Request| {
                    let slot = Arc::clone(&slot);
                    async move {
                        let (events, stream) = sse::channel(8);
                        *slot.lock().expect("slot") = Some(events);
                        match heartbeat {
                            Some(every) => Response::events(stream.heartbeat(every)),
                            None => Response::events(stream),
                        }
                    }
                })
                .await
                .expect("the server ran");
        });
        Self {
            addr,
            stopper,
            task,
            sender,
        }
    }

    /// The sender of the one open stream, once the handler has run.
    async fn sender(&self) -> Sender {
        for _ in 0..500 {
            if let Some(sender) = self.sender.lock().expect("slot").clone() {
                return sender;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the handler never ran");
    }
}

async fn subscribe(addr: &str) -> TcpStream {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    stream
        .write_all(b"GET /events HTTP/1.1\r\nHost: x\r\nAccept: text/event-stream\r\n\r\n")
        .await
        .expect("write");
    stream
}

async fn read_until(stream: &mut TcpStream, seen: &mut Vec<u8>, needle: &str) {
    let found = tokio::time::timeout(Duration::from_secs(5), async {
        while !String::from_utf8_lossy(seen).contains(needle) {
            let mut buf = [0u8; 512];
            match stream.read(&mut buf).await {
                Ok(0) => return,
                Ok(n) => seen.extend_from_slice(&buf[..n]),
                Err(e) => panic!("read: {e}"),
            }
        }
    })
    .await;
    assert!(
        found.is_ok() && String::from_utf8_lossy(seen).contains(needle),
        "{needle:?} did not arrive; what did:\n{}",
        String::from_utf8_lossy(seen)
    );
}

#[tokio::test]
async fn events_reach_the_client_as_they_are_sent() {
    let server = Running::start(None).await;
    let mut client = subscribe(&server.addr).await;
    let events = server.sender().await;

    let mut seen = Vec::new();
    events
        .send(Event::new("one").with_event("tick").with_id("1"))
        .await
        .expect("open");
    read_until(&mut client, &mut seen, "data:one").await;
    let head = String::from_utf8_lossy(&seen).to_lowercase();
    assert!(head.contains("content-type: text/event-stream"), "{head}");
    assert!(head.contains("cache-control: no-cache"), "{head}");
    assert!(head.contains("id:1\nevent:tick\ndata:one\n\n"), "{head}");

    events.comment("still here").await.expect("open");
    read_until(&mut client, &mut seen, ": still here").await;

    drop(client);
    server.stopper.stop();
    let _ = tokio::time::timeout(Duration::from_secs(5), server.task).await;
}

#[tokio::test]
async fn a_client_that_leaves_closes_the_sender() {
    // The heartbeat is the write that tells the server the socket is gone.
    let server = Running::start(Some(Duration::from_millis(50))).await;
    let mut client = subscribe(&server.addr).await;
    let events = server.sender().await;
    let mut seen = Vec::new();
    read_until(&mut client, &mut seen, ":\n\n").await;

    drop(client);
    let left = tokio::time::timeout(Duration::from_secs(5), events.closed()).await;
    assert!(left.is_ok(), "the sender never learned the client left");
    assert_eq!(events.send(Event::new("late")).await, Err(Closed));

    server.stopper.stop();
    let _ = tokio::time::timeout(Duration::from_secs(5), server.task).await;
}

#[tokio::test]
async fn stopping_the_server_ends_the_stream_and_closes_the_sender() {
    let server = Running::start(None).await;
    let mut client = subscribe(&server.addr).await;
    let events = server.sender().await;
    let mut seen = Vec::new();
    events.send(Event::new("before")).await.expect("open");
    read_until(&mut client, &mut seen, "data:before").await;

    server.stopper.stop();
    tokio::time::timeout(Duration::from_secs(5), events.closed())
        .await
        .expect("the sender sees the shutdown");
    // The chunked body ends with its zero-length chunk: a clean end, not a
    // dropped connection.
    read_until(&mut client, &mut seen, "0\r\n\r\n").await;
    tokio::time::timeout(Duration::from_secs(5), server.task)
        .await
        .expect("the server stopped")
        .expect("the server task");
}
