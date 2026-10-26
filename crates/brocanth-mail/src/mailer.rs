//! The mail contract, which is the whole public surface of this crate.
//!
//! One method. A mailer takes a [`Message`] that is already valid and puts it
//! somewhere: a vector in a test, a relay in production, a queue table in
//! between. Everything above it -- a password reset, a login alert, a receipt
//! -- talks to this trait and never to a backend.
//!
//! # Why `send` takes `&Message` and answers nothing
//!
//! There is no `MessageId` in the return type, because only one of the three
//! backends could produce one and a value that is `None` on the backend you
//! actually run is a value nobody can use. A backend that has a provider id
//! worth keeping exposes it on its own type, where the caller already knows
//! which backend it holds.
//!
//! `&Message` rather than `Message`, so the same message can go to a mailer
//! and to a log, and so a retry does not need a clone.
//!
//! # Why it is async
//!
//! Because the useful mailers are. SMTP is a socket, an API relay is an HTTP
//! request, and the one this crate was written for -- hand the message to
//! `brocanth-jobs` and answer the request now -- is an `INSERT`. A synchronous
//! trait would make that last one need a runtime handle smuggled in from
//! somewhere, which is the shape of a dependency this crate refuses to have.
//!
//! This crate does **not** depend on `brocanth-jobs` and never will: §8 says a
//! plugin that needs another depends on it in the open, and a mailer does not
//! need a queue. What it owes the queue is the two things that make one
//! possible from outside -- an async method to implement, and a [`Message`]
//! that can be a payload (the `serde` feature). A queue-backed mailer is
//! twenty lines in the application, or a later crate that depends on both.
//!
//! # Object safety
//!
//! [`Mailer`] is object-safe: no generic method, no `Self` in a return
//! position, and `async_trait` boxes the future. `Arc<dyn Mailer>` is the
//! expected way to hold one, and [`crate::plugin::MailConfig`] takes exactly
//! that -- so swapping `Memory` for `Smtp` is one line in `main` and no change
//! to a single handler.

use async_trait::async_trait;

use crate::error::MailError;
use crate::message::Message;

/// Everything the rest of the system needs from mail.
///
/// Implementations must be safe to call concurrently; every method takes
/// `&self` for the same reason a connection pool does.
#[async_trait]
pub trait Mailer: Send + Sync + 'static {
    /// Delivers the message, or hands it to something that will.
    ///
    /// "Delivered" means the backend accepted it, and nothing stronger. A
    /// relay that answered `250` can still bounce the message an hour later,
    /// and no mailer can promise otherwise.
    ///
    /// # Errors
    /// [`MailError::Transport`] when the backend would not take it.
    async fn send(&self, message: &Message) -> Result<(), MailError>;
}
