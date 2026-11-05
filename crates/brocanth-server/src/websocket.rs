//! WebSockets over [`upgrade`](crate::upgrade): the handshake, the Origin
//! check, and a frame codec. Behind the `websocket` feature, which is off.
//!
//! ```no_run
//! use brocanth_server::websocket::{Handshake, Incoming};
//! use brocanth_server::{Response, Server};
//!
//! # async fn run() -> Result<(), brocanth_server::ServeError> {
//! Server::bind("127.0.0.1:0").await?.run(|request| async move {
//!     // The protocol and the Origin, before anything is switched.
//!     let handshake = match Handshake::read(|name| request.header(name), &[]) {
//!         Ok(handshake) => handshake,
//!         Err(refused) => return Response::text(refused.status(), refused.to_string()),
//!     };
//!     handshake.response(|mut socket| async move {
//!         // An echo, until the client leaves or the server stops.
//!         loop {
//!             match socket.recv().await {
//!                 Incoming::Text(text) => {
//!                     if socket.send_text(text).await.is_err() {
//!                         return;
//!                     }
//!                 }
//!                 Incoming::Binary(_) => {}
//!                 Incoming::Stopping => return socket.close(1001, "the server is stopping").await,
//!                 Incoming::Closed(_) => return,
//!             }
//!         }
//!     })
//! }).await
//! # }
//! ```
//!
//! # What this module checks, and what it leaves to the page
//!
//! [`Handshake::read`] checks the protocol (`Upgrade: websocket`,
//! `Connection: upgrade`, version 13, a key) and the `Origin`. A browser
//! sends the cookies of the site with a WebSocket handshake from **any**
//! page, and the same-origin policy does not apply to it, so without the
//! Origin check any site can open a socket as the signed-in user. That
//! check is the CSRF check of a WebSocket: the handshake is a `GET`, so a
//! CSRF token check on unsafe methods never sees it.
//!
//! What the socket may then do is the page's decision, made before it
//! answers: the session, the policy, the topics. The page refuses with a
//! status, which the browser can read, and not with a close frame after the
//! switch.
//!
//! # Back-pressure
//!
//! [`WebSocket::send_text`] waits until the codec has written the frame to
//! the socket. A client that reads slowly fills the TCP buffer, and the send
//! waits. A task that sends what a broker queued then stops taking from the
//! queue, and the queue's own overflow policy decides what is lost. Nothing
//! here buffers without a bound: [`Handshake::max_message`] caps what the
//! client may send, and the codec's write buffer is capped too.
//!
//! # The codec
//!
//! tungstenite, through `tokio-tungstenite`, without its client connector
//! and without TLS. Its types do not appear here, so the codec can change
//! without a change to an application. See `Cargo.toml` for the reason.

use std::net::SocketAddr;
use std::task::Poll;
use std::time::Duration;

use brocanth::plugin_body::Streaming;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Role, WebSocketConfig};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::WebSocketStream;

use crate::upgrade::{self, Socket};
use crate::Response;

/// Close code 1000: the conversation is over.
pub const NORMAL: u16 = 1000;
/// Close code 1001: the server is stopping. A browser reconnects.
pub const GOING_AWAY: u16 = 1001;
/// Close code 1008: the client did something its terms do not allow.
pub const POLICY: u16 = 1008;
/// Close code 1013: the server cannot keep up with this client. Try again
/// later, and fetch what may have been missed.
pub const TRY_AGAIN_LATER: u16 = 1013;

/// The most a client may send in one message, unless
/// [`Handshake::max_message`] says else: 64 KiB.
///
/// Small, because a browser that reads a broker's frames sends commands,
/// not documents. tungstenite's own default is 64 MiB.
pub const MAX_MESSAGE: usize = 64 * 1024;

/// How long [`WebSocket::close`] waits for the client's close frame.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// A handshake that passed: the protocol is right and the Origin is
/// allowed.
#[derive(Debug, Clone)]
pub struct Handshake {
    accept: String,
    max_message: usize,
}

/// Why a handshake was refused, with the status to answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// Not a WebSocket handshake at all: no `Upgrade: websocket` or no
    /// `Connection: upgrade`. Also every HTTP/2 request.
    NotAnUpgrade,
    /// A version other than 13, the only one RFC 6455 defines.
    Version(String),
    /// No `Sec-WebSocket-Key`, or one that is not 16 bytes of base64.
    BadKey,
    /// The page that opened the socket is on another site.
    CrossOrigin {
        /// The `Origin` the browser sent.
        origin: String,
    },
}

impl Refused {
    /// The status to answer with: 426 for the protocol, 400 for a bad key,
    /// 403 for an Origin.
    pub fn status(&self) -> u16 {
        match self {
            Self::NotAnUpgrade | Self::Version(_) => 426,
            Self::BadKey => 400,
            Self::CrossOrigin { .. } => 403,
        }
    }
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAnUpgrade => write!(
                f,
                "this address is a WebSocket\n\
                 help: open it with `new WebSocket(url)`, which sends `Upgrade: websocket` over HTTP/1.1"
            ),
            Self::Version(version) => write!(
                f,
                "WebSocket version {version:?} is not supported\n\
                 help: send `Sec-WebSocket-Version: 13`"
            ),
            Self::BadKey => write!(
                f,
                "the Sec-WebSocket-Key header is missing or is not 16 bytes of base64\n\
                 help: let the WebSocket client make the key"
            ),
            Self::CrossOrigin { origin } => write!(
                f,
                "a page on {origin} may not open a socket here\n\
                 help: add the origin to the allowed list of this page, if that site is yours"
            ),
        }
    }
}

impl std::error::Error for Refused {}

impl Handshake {
    /// Reads a handshake from the request headers, and checks its Origin.
    ///
    /// `header` looks a header up by name, case-insensitively, as
    /// [`Request::header`](crate::Request::header) and a page's
    /// `ctx.header` do. `allowed` lists the origins other than this host
    /// that may open a socket, spelled as a browser sends them:
    /// `https://app.example.com`.
    ///
    /// The Origin rule: no `Origin` header passes, because only a browser
    /// carries the user's cookies and a browser always sends one. An
    /// `Origin` whose host and port equal the `Host` header passes. One in
    /// `allowed` passes. Everything else, `null` included, is refused.
    pub fn read<'a, F>(header: F, allowed: &[&str]) -> Result<Self, Refused>
    where
        F: Fn(&str) -> Option<&'a str>,
    {
        if !has_token(header("upgrade"), "websocket") || !has_token(header("connection"), "upgrade")
        {
            return Err(Refused::NotAnUpgrade);
        }
        match header("sec-websocket-version").map(str::trim) {
            Some("13") => {}
            other => return Err(Refused::Version(other.unwrap_or("").to_string())),
        }
        let key = header("sec-websocket-key")
            .map(str::trim)
            .filter(|key| is_key(key))
            .ok_or(Refused::BadKey)?;
        if let Some(origin) = header("origin").map(str::trim) {
            if !same_origin(origin, header("host")) && !allowed.contains(&origin) {
                return Err(Refused::CrossOrigin {
                    origin: origin.to_string(),
                });
            }
        }
        Ok(Self {
            accept: derive_accept_key(key.as_bytes()),
            max_message: MAX_MESSAGE,
        })
    }

    /// The most a client may send in one message. [`MAX_MESSAGE`] unless
    /// set. A larger message ends the connection.
    #[must_use]
    pub fn max_message(mut self, bytes: usize) -> Self {
        self.max_message = bytes;
        self
    }

    /// The three headers of the 101, beside `status: 101`.
    pub fn headers(&self) -> [(String, String); 3] {
        [
            ("upgrade".to_string(), "websocket".to_string()),
            ("connection".to_string(), "upgrade".to_string()),
            ("sec-websocket-accept".to_string(), self.accept.clone()),
        ]
    }

    /// The body of the 101: the connection switches after the headers, and
    /// `on_open` runs with the socket on a task of its own.
    ///
    /// For a page whose response type is not [`Response`]: answer status
    /// 101 with [`Handshake::headers`] and this as the stream.
    pub fn switch<F, Fut>(self, on_open: F) -> Streaming
    where
        F: FnOnce(WebSocket) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let config = config(self.max_message);
        upgrade::switch(move |socket| async move {
            let stream = WebSocketStream::from_raw_socket(socket, Role::Server, Some(config)).await;
            on_open(WebSocket::new(stream)).await;
        })
    }

    /// The whole 101, for an application on [`Server::run`](crate::Server::run).
    pub fn response<F, Fut>(self, on_open: F) -> Response
    where
        F: FnOnce(WebSocket) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let headers = self.headers();
        let mut response = Response {
            status: 101,
            body: crate::Body::Stream(self.switch(on_open)),
            headers: Vec::new(),
        };
        for (name, value) in headers {
            response = response.with_header(&name, &value);
        }
        response
    }
}

fn config(max_message: usize) -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(max_message))
        .max_frame_size(Some(max_message))
        // What one slow client may make this process hold before a send
        // fails rather than waits: a few broker frames, not a file.
        .max_write_buffer_size(1024 * 1024)
}

/// Whether a comma-separated header holds `token`, ignoring case.
fn has_token(value: Option<&str>, token: &str) -> bool {
    value.is_some_and(|value| {
        value
            .split(',')
            .any(|part| part.trim().eq_ignore_ascii_case(token))
    })
}

/// Whether `key` is 16 bytes of base64: 24 characters, the last two `=`.
fn is_key(key: &str) -> bool {
    let bytes = key.as_bytes();
    bytes.len() == 24
        && bytes[22..] == *b"=="
        && bytes[..22]
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'+' || *b == b'/')
}

/// Whether `origin` (`scheme://host[:port]`) names the host the request
/// was sent to.
///
/// The scheme is not compared: behind a TLS proxy this server sees `http`
/// and the browser says `https`, and the host is what an attacker cannot
/// choose.
fn same_origin(origin: &str, host: Option<&str>) -> bool {
    let Some(host) = host.map(str::trim).filter(|h| !h.is_empty()) else {
        return false;
    };
    let Some((scheme, authority)) = origin.split_once("://") else {
        return false;
    };
    let default_port = match scheme {
        "https" => "443",
        "http" => "80",
        _ => return false,
    };
    let with_port = |a: &str| -> String {
        let a = a.to_ascii_lowercase();
        // `[::1]:3000`, `[::1]`, `example.com:3000`, `example.com`.
        let has_port = match a.rfind(']') {
            Some(close) => a[close..].contains(':'),
            None => a.contains(':'),
        };
        if has_port {
            a
        } else {
            format!("{a}:{default_port}")
        }
    };
    with_port(authority) == with_port(host)
}

/// What [`WebSocket::recv`] read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incoming {
    /// A text message.
    Text(String),
    /// A binary message.
    Binary(Vec<u8>),
    /// The server is stopping. Returned once; send what is left, then
    /// [`WebSocket::close`] with [`GOING_AWAY`].
    Stopping,
    /// The connection is over: the client closed it, with its close code
    /// when it sent one, or it failed. Nothing more can be sent.
    Closed(Option<u16>),
}

/// The connection is gone: the client left, or a write failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gone(String);

impl std::fmt::Display for Gone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the WebSocket is gone: {}", self.0)
    }
}

impl std::error::Error for Gone {}

impl From<WsError> for Gone {
    fn from(e: WsError) -> Self {
        Self(e.to_string())
    }
}

/// One open WebSocket: messages both ways, and the shutdown signal.
pub struct WebSocket {
    stream: WebSocketStream<Socket>,
    /// Whether [`Incoming::Stopping`] was returned.
    told_to_stop: bool,
    /// Whether a ping is out with no frame heard since.
    pinged: bool,
}

impl std::fmt::Debug for WebSocket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSocket")
            .field("socket", self.stream.get_ref())
            .finish()
    }
}

impl WebSocket {
    fn new(stream: WebSocketStream<Socket>) -> Self {
        Self {
            stream,
            told_to_stop: false,
            pinged: false,
        }
    }

    /// The next message, the stop signal, or the end.
    ///
    /// Pings and pongs are answered and read here, and never returned. A
    /// close frame from the client is answered and returned as
    /// [`Incoming::Closed`]. Safe to cancel: a `select!` that drops this
    /// future loses no message.
    pub async fn recv(&mut self) -> Incoming {
        loop {
            let polled = std::future::poll_fn(|cx| {
                if !self.told_to_stop && self.stream.get_ref().poll_closing(cx).is_ready() {
                    self.told_to_stop = true;
                    return Poll::Ready(None);
                }
                self.stream.poll_next_unpin(cx).map(Some)
            })
            .await;
            let Some(next) = polled else {
                return Incoming::Stopping;
            };
            match next {
                Some(Ok(message)) => {
                    self.pinged = false;
                    match message {
                        Message::Text(text) => return Incoming::Text(text.as_str().to_string()),
                        Message::Binary(bytes) => return Incoming::Binary(bytes.to_vec()),
                        Message::Close(frame) => {
                            // The codec queued the reply. Reading on writes
                            // it and then ends the stream.
                            let code = frame.map(|f| u16::from(f.code));
                            while let Some(Ok(_)) = self.stream.next().await {}
                            return Incoming::Closed(code);
                        }
                        Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
                    }
                }
                Some(Err(_)) | None => return Incoming::Closed(None),
            }
        }
    }

    /// Sends a text message, and waits until it is written to the socket.
    pub async fn send_text(&mut self, text: impl Into<String>) -> Result<(), Gone> {
        Ok(self.stream.send(Message::text(text.into())).await?)
    }

    /// Sends a binary message, and waits until it is written to the socket.
    pub async fn send_binary(&mut self, bytes: impl Into<Vec<u8>>) -> Result<(), Gone> {
        Ok(self.stream.send(Message::binary(bytes.into())).await?)
    }

    /// Sends a ping. Fails when the previous ping got no answer: no frame
    /// at all arrived since, so the client is gone or cannot keep up.
    ///
    /// Call it on a timer. The server learns that a TCP peer is gone only
    /// when a write fails, so this is also what frees a dead client.
    pub async fn keepalive(&mut self) -> Result<(), Gone> {
        if self.pinged {
            return Err(Gone("no answer to the last ping".to_string()));
        }
        self.pinged = true;
        Ok(self.stream.send(Message::Ping(Vec::new().into())).await?)
    }

    /// Whether the server is stopping.
    pub fn is_stopping(&self) -> bool {
        self.stream.get_ref().is_closing()
    }

    /// Sends a close frame with `code` and `reason`, waits a short time for
    /// the client's reply, and drops the connection.
    pub async fn close(mut self, code: u16, reason: &str) {
        let frame = CloseFrame {
            code: CloseCode::from(code),
            reason: reason.to_string().into(),
        };
        if self.stream.close(Some(frame)).await.is_err() {
            return;
        }
        let _ = tokio::time::timeout(CLOSE_WAIT, async {
            while let Some(Ok(_)) = self.stream.next().await {}
        })
        .await;
    }
}

/// Why [`connect`] did not open a socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectError {
    /// The status the server answered instead of 101, when it answered.
    pub status: Option<u16>,
    /// What went wrong.
    pub message: String,
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.status {
            Some(status) => write!(f, "the server answered {status}: {}", self.message),
            None => write!(f, "cannot open the WebSocket: {}", self.message),
        }
    }
}

impl std::error::Error for ConnectError {}

/// Opens a WebSocket to `addr` as a client, with `origin` as the browser
/// would send it.
///
/// For a test, or for a client this application runs itself: plain TCP,
/// no TLS. `target` is the path and the query, `/realtime/socket?topics=..`.
/// The returned socket has no gate, so it never reads
/// [`Incoming::Stopping`].
pub async fn connect(
    addr: SocketAddr,
    target: &str,
    origin: Option<&str>,
) -> Result<WebSocket, ConnectError> {
    let failed = |message: String| ConnectError {
        status: None,
        message,
    };
    let mut request = format!("ws://{addr}{target}")
        .into_client_request()
        .map_err(|e| failed(e.to_string()))?;
    if let Some(origin) = origin {
        let value = HeaderValue::from_str(origin).map_err(|e| failed(e.to_string()))?;
        request.headers_mut().insert("origin", value);
    }
    let tcp = tokio::net::TcpStream::connect(addr)
        .await
        .map_err(|e| failed(e.to_string()))?;
    match tokio_tungstenite::client_async(request, Socket::tcp(tcp)).await {
        Ok((stream, _)) => Ok(WebSocket::new(stream)),
        Err(WsError::Http(response)) => Err(ConnectError {
            status: Some(response.status().as_u16()),
            message: response
                .body()
                .as_deref()
                .map(|body| String::from_utf8_lossy(body).into_owned())
                .unwrap_or_default(),
        }),
        Err(e) => Err(failed(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<&'a str> {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| *value)
        }
    }

    const KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";

    fn handshake(extra: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
        let mut pairs = vec![
            ("Upgrade", "websocket"),
            ("Connection", "keep-alive, Upgrade"),
            ("Sec-WebSocket-Version", "13"),
            ("Sec-WebSocket-Key", KEY),
            ("Host", "app.example.com"),
        ];
        pairs.extend_from_slice(extra);
        pairs
    }

    #[test]
    fn the_accept_key_is_the_one_rfc_6455_computes() {
        let pairs = handshake(&[]);
        let accepted = Handshake::read(headers(&pairs), &[]).expect("a handshake");
        // RFC 6455 §1.3, the worked example.
        assert_eq!(accepted.headers()[2].1, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn a_plain_get_or_an_old_version_is_told_to_upgrade() {
        let plain = [("Host", "app.example.com")];
        let refused = Handshake::read(headers(&plain), &[]).expect_err("not an upgrade");
        assert_eq!(refused, Refused::NotAnUpgrade);
        assert_eq!(refused.status(), 426);
        assert!(refused.to_string().contains("\nhelp: "), "{refused}");

        let mut old = handshake(&[]);
        old[2] = ("Sec-WebSocket-Version", "8");
        let refused = Handshake::read(headers(&old), &[]).expect_err("version 8");
        assert_eq!(refused, Refused::Version("8".into()));

        let mut keyless = handshake(&[]);
        keyless[3] = ("Sec-WebSocket-Key", "short");
        assert_eq!(
            Handshake::read(headers(&keyless), &[]).expect_err("bad key"),
            Refused::BadKey
        );
    }

    #[test]
    fn the_origin_must_be_this_host_or_on_the_list() {
        let ok = |origin: &'static str, allowed: &[&str]| {
            let pairs = handshake(&[("Origin", origin)]);
            Handshake::read(headers(&pairs), allowed).is_ok()
        };
        assert!(ok("https://app.example.com", &[]));
        assert!(ok("http://APP.example.com", &[]), "scheme and case aside");
        assert!(!ok("https://app.example.com:8443", &[]), "another port");
        assert!(!ok("https://evil.example", &[]));
        assert!(!ok("null", &[]), "a sandboxed frame or a file");
        assert!(ok(
            "https://admin.example.com",
            &["https://admin.example.com"]
        ));

        let refused = {
            let pairs = handshake(&[("Origin", "https://evil.example")]);
            Handshake::read(headers(&pairs), &[]).expect_err("cross-site")
        };
        assert_eq!(refused.status(), 403);

        // No Origin: not a browser, so not the user's cookies.
        let pairs = handshake(&[]);
        assert!(Handshake::read(headers(&pairs), &[]).is_ok());
    }

    #[test]
    fn a_port_in_the_host_header_is_compared_with_the_origin() {
        assert!(same_origin("http://127.0.0.1:3000", Some("127.0.0.1:3000")));
        assert!(!same_origin(
            "http://127.0.0.1:3001",
            Some("127.0.0.1:3000")
        ));
        assert!(same_origin("https://example.com", Some("example.com:443")));
        assert!(same_origin("http://[::1]:3000", Some("[::1]:3000")));
        assert!(!same_origin("http://example.com", None));
    }
}
