//! What one client may cost: a stalled request is closed, a body has a size
//! and a deadline, and the number of open connections has a cap.

use std::time::Duration;

use brocanth_server::{Response, Server, Stopper};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

struct Running {
    addr: String,
    stopper: Stopper,
    task: tokio::task::JoinHandle<()>,
}

async fn start(server: Server) -> Running {
    let addr = server.local_addr().expect("local addr").to_string();
    let stopper = server.stopper();
    let task = tokio::spawn(async move {
        server
            .run(|request| async move { Response::text(200, format!("{}", request.body.len())) })
            .await
            .expect("the server ran");
    });
    Running {
        addr,
        stopper,
        task,
    }
}

async fn bind() -> Server {
    Server::bind("127.0.0.1:0").await.expect("an ephemeral port")
}

/// Reads until the server closes the socket, within `limit`.
async fn read_to_close(stream: &mut TcpStream, limit: Duration) -> String {
    let mut out = Vec::new();
    tokio::time::timeout(limit, stream.read_to_end(&mut out))
        .await
        .expect("the server closed the connection in time")
        .expect("read");
    String::from_utf8_lossy(&out).into_owned()
}

#[tokio::test]
async fn a_client_that_stalls_in_its_headers_is_closed() {
    let server = start(bind().await.header_read_timeout(Duration::from_millis(200))).await;
    let mut client = TcpStream::connect(&server.addr).await.expect("connect");
    // Half a request line, then nothing: slowloris.
    client.write_all(b"GET / HTTP/1.1\r\nHo").await.expect("write");
    let _ = read_to_close(&mut client, Duration::from_secs(5)).await;
    server.stopper.stop();
    server.task.await.expect("no panic");
}

#[tokio::test]
async fn a_body_that_stalls_is_a_408() {
    let server = start(bind().await.body_read_timeout(Duration::from_millis(200))).await;
    let mut client = TcpStream::connect(&server.addr).await.expect("connect");
    client
        .write_all(b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 10\r\n\r\nab")
        .await
        .expect("write");
    let answer = read_to_close(&mut client, Duration::from_secs(5)).await;
    assert!(answer.starts_with("HTTP/1.1 408"), "{answer}");
    server.stopper.stop();
    server.task.await.expect("no panic");
}

#[tokio::test]
async fn the_body_cap_is_the_one_the_server_was_given() {
    let server = start(bind().await.max_body(4)).await;
    let mut client = TcpStream::connect(&server.addr).await.expect("connect");
    client
        .write_all(b"POST / HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: 5\r\n\r\nabcde")
        .await
        .expect("write");
    let answer = read_to_close(&mut client, Duration::from_secs(5)).await;
    assert!(answer.starts_with("HTTP/1.1 413"), "{answer}");

    let mut client = TcpStream::connect(&server.addr).await.expect("connect");
    client
        .write_all(b"POST / HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: 4\r\n\r\nabcd")
        .await
        .expect("write");
    let answer = read_to_close(&mut client, Duration::from_secs(5)).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert!(answer.ends_with("\r\n\r\n4"), "{answer}");
    server.stopper.stop();
    server.task.await.expect("no panic");
}

#[tokio::test]
async fn at_the_connection_cap_the_next_client_waits_for_a_slot() {
    let server = start(bind().await.max_connections(1)).await;
    // The first connection holds the one slot.
    let mut first = TcpStream::connect(&server.addr).await.expect("connect");
    first
        .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .expect("write");
    let mut buf = [0u8; 64];
    let n = first.read(&mut buf).await.expect("read");
    assert!(buf[..n].starts_with(b"HTTP/1.1 200"));

    // The second is in the kernel's backlog and is not served yet.
    let mut second = TcpStream::connect(&server.addr).await.expect("connect");
    second
        .write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await
        .expect("write");
    let waited = tokio::time::timeout(Duration::from_millis(300), second.read(&mut buf)).await;
    assert!(waited.is_err(), "the second client was served past the cap");

    // The first leaves; the second gets its slot.
    drop(first);
    let answer = read_to_close(&mut second, Duration::from_secs(5)).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    server.stopper.stop();
    server.task.await.expect("no panic");
}
