//! The socket loop `realtime-websocket` writes into a project, compiled and
//! tested here.
//!
//! `templates/realtime_websocket.rs` is the file the module writes as
//! `src/plugins/realtime_websocket/mod.rs`, byte for byte. Its own tests run
//! a real server on a real port: a socket subscribes, reads a change the
//! broker published, closes, and the broker frees its client; a stopping
//! server sends close code 1001. The Origin refusal is in
//! `brocanth-server`'s `tests/websocket.rs`, and again in the generated
//! page's tests.

#[path = "../templates/realtime_websocket.rs"]
mod realtime_websocket;
