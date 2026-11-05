//! The second exit from this crate: a connection that leaves HTTP.
//!
//! Every other response ends with its body. A `101 Switching Protocols`
//! ends with the headers, and the connection then belongs to another
//! protocol. A WebSocket is the case that matters, and
//! [`websocket`](crate::websocket) is built on this module.
//!
//! # How a page answers with an upgrade
//!
//! The page returns status 101 and, as its body,
//! [`switch`]`(on_socket)`. That is a core
//! [`Streaming`](brocanth::plugin_body::Streaming) that holds no bytes, so it
//! travels through the bridge and every response hook like any other
//! stream: `on_headers` runs and can add a header, `on_body` does not run,
//! and no hook can wrap it. The server writes the 101, waits for hyper to
//! hand over the connection, and calls `on_socket` on a task of its own with
//! the raw [`Socket`].
//!
//! ```no_run
//! use brocanth_server::{upgrade, Response, Server};
//! use tokio::io::AsyncWriteExt;
//!
//! # async fn run() -> Result<(), brocanth_server::ServeError> {
//! Server::bind("127.0.0.1:0").await?.run(|_request| async {
//!     Response::upgrade("echo", |mut socket: upgrade::Socket| async move {
//!         let _ = socket.write_all(b"hello\n").await;
//!     })
//! }).await
//! # }
//! ```
//!
//! # Shutdown
//!
//! An upgraded connection is no longer hyper's, so hyper's drain does not
//! wait for it. The server registers each socket with its
//! [`StreamGate`](brocanth::plugin_body::StreamGate) instead, for as long as
//! `on_socket` runs. [`Server::run`](crate::Server::run) closes the gate when
//! the listener stops, [`Socket::closing`] resolves, and the protocol sends
//! its last frame (a WebSocket close frame, code 1001). The drain then waits
//! for the gate to empty, inside [`Server::drain_grace`](crate::Server::drain_grace).
//!
//! # HTTP/2
//!
//! An HTTP/2 request has no `Upgrade` header, so a WebSocket handshake
//! refuses it before it reaches this module. A 101 that reaches the server on
//! a connection that cannot switch is answered 500, with no socket.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use brocanth::plugin_body::{StreamTicket, Streaming, Upgrade};
use hyper::upgrade::Upgraded;
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

/// What the server calls once the connection has switched.
pub(crate) type OnSocket =
    Box<dyn FnOnce(Socket) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

/// A body that is not a body: the connection switches after the headers,
/// and the server calls `on_socket` with it.
///
/// Answer with status 101 and the headers of the protocol. See the module
/// documentation.
pub fn switch<F, Fut>(on_socket: F) -> Streaming
where
    F: FnOnce(Socket) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let boxed: OnSocket = Box::new(move |socket| Box::pin(on_socket(socket)));
    Streaming::upgrade(Upgrade::new(boxed))
}

/// The function a page put in its 101, or `None` when the body is not an
/// upgrade this crate made.
pub(crate) fn taken(stream: &mut Streaming) -> Option<OnSocket> {
    let upgrade = stream.take_upgrade()?;
    upgrade.downcast::<OnSocket>().ok().map(|boxed| *boxed)
}

/// One connection after the switch: bytes in, bytes out, and the shutdown
/// signal.
///
/// It implements tokio's [`AsyncRead`] and [`AsyncWrite`], so any protocol
/// codec on tokio can run over it. hyper's types stay inside this crate.
pub struct Socket {
    io: Io,
    /// The registration with the server's gate. Dropped with the socket,
    /// which is what lets the drain finish.
    ticket: Option<StreamTicket>,
}

enum Io {
    /// What hyper handed over after a 101.
    Upgraded(TokioIo<Upgraded>),
    /// A plain TCP connection: the client side of a test, or a protocol
    /// that never was HTTP.
    Tcp(TcpStream),
}

impl Socket {
    pub(crate) fn upgraded(io: Upgraded, ticket: StreamTicket) -> Self {
        Self {
            io: Io::Upgraded(TokioIo::new(io)),
            ticket: Some(ticket),
        }
    }

    /// A socket over a TCP connection, with no gate.
    ///
    /// The client end of a connection, for a test or for a client this
    /// application runs itself. [`Socket::closing`] never resolves for it.
    pub fn tcp(stream: TcpStream) -> Self {
        Self {
            io: Io::Tcp(stream),
            ticket: None,
        }
    }

    /// Whether the server is stopping.
    pub fn is_closing(&self) -> bool {
        self.ticket.as_ref().is_some_and(StreamTicket::is_closing)
    }

    /// Resolves when the server begins to stop, and never for a socket
    /// that has no gate.
    ///
    /// Await it beside the reads: when it resolves, send the protocol's last
    /// frame and return. A socket that ignores it holds the drain open until
    /// [`Server::drain_grace`](crate::Server::drain_grace) runs out.
    pub async fn closing(&self) {
        match &self.ticket {
            Some(ticket) => std::future::poll_fn(|cx| ticket.poll_closing(cx)).await,
            None => std::future::pending().await,
        }
    }

    /// The same as [`Socket::closing`], as a poll function, for a protocol
    /// that implements its own future.
    pub fn poll_closing(&self, cx: &mut Context<'_>) -> Poll<()> {
        match &self.ticket {
            Some(ticket) => ticket.poll_closing(cx),
            None => Poll::Pending,
        }
    }
}

impl std::fmt::Debug for Socket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self.io {
            Io::Upgraded(_) => "upgraded",
            Io::Tcp(_) => "tcp",
        };
        f.debug_struct("Socket")
            .field("io", &kind)
            .field("closing", &self.is_closing())
            .finish()
    }
}

impl AsyncRead for Socket {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match &mut self.get_mut().io {
            Io::Upgraded(io) => Pin::new(io).poll_read(cx, buf),
            Io::Tcp(io) => Pin::new(io).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Socket {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match &mut self.get_mut().io {
            Io::Upgraded(io) => Pin::new(io).poll_write(cx, buf),
            Io::Tcp(io) => Pin::new(io).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.get_mut().io {
            Io::Upgraded(io) => Pin::new(io).poll_flush(cx),
            Io::Tcp(io) => Pin::new(io).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.get_mut().io {
            Io::Upgraded(io) => Pin::new(io).poll_shutdown(cx),
            Io::Tcp(io) => Pin::new(io).poll_shutdown(cx),
        }
    }
}
