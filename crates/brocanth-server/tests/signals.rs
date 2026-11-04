//! `Server::stop_on_signals`, with a real SIGTERM.
//!
//! Its own test binary, because the signal goes to the whole process: the
//! handler that `stop_on_signals` installs is the only one here.

#![cfg(unix)]

use std::time::{Duration, Instant};

use brocanth_server::{Response, Server};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// A GET with `Connection: close`, and the whole response.
async fn get(addr: &str, path: &str) -> std::io::Result<String> {
    let mut stream = TcpStream::connect(addr).await?;
    let raw = format!("GET {path} HTTP/1.1\r\nhost: t\r\nconnection: close\r\n\r\n");
    stream.write_all(raw.as_bytes()).await?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await?;
    Ok(String::from_utf8_lossy(&response).into_owned())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sigterm_drains_the_open_request_and_run_returns_ok() {
    let delay = Duration::from_millis(300);
    let server = Server::bind("127.0.0.1:0")
        .await
        .expect("an ephemeral port")
        .stop_on_signals(delay)
        .drain_grace(Duration::from_secs(5));
    let addr = server.local_addr().expect("local addr").to_string();
    let running = tokio::spawn(server.run(|request| async move {
        if request.path == "/slow" {
            tokio::time::sleep(Duration::from_millis(800)).await;
        }
        Response::text(200, format!("done {}", request.path))
    }));

    // One answer proves the handler is installed: it is, before the first
    // accept.
    assert!(get(&addr, "/fast")
        .await
        .expect("served")
        .contains("done /fast"));

    let slow = tokio::spawn({
        let addr = addr.clone();
        async move { get(&addr, "/slow").await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    let signalled = Instant::now();
    let status = std::process::Command::new("kill")
        .args(["-TERM", &std::process::id().to_string()])
        .status()
        .expect("kill runs");
    assert!(status.success());

    // During the delay the listener still answers: the load balancer is
    // still removing this endpoint.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let during = get(&addr, "/fast")
        .await
        .expect("still accepting during the delay");
    assert!(during.contains("done /fast"), "{during}");

    // The open request finishes, and `run` returns `Ok`.
    let slow = slow
        .await
        .expect("task")
        .expect("the slow request was answered");
    assert!(
        slow.contains("200 OK") && slow.contains("done /slow"),
        "{slow}"
    );
    let ran = tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .expect("run returned after the signal")
        .expect("the server task did not panic");
    assert!(ran.is_ok(), "{ran:?}");
    assert!(
        signalled.elapsed() >= delay,
        "the listener stayed open for the delay"
    );

    // And the listener is closed.
    assert!(
        get(&addr, "/fast").await.is_err(),
        "nothing accepts after the stop"
    );
}
