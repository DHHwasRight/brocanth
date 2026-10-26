//! The backend that matters today: one that records what was sent.
//!
//! `brocanth-auth` says in four places that it sends no mail. It produces a
//! `ResetOutcome`, a `RegistrationNotice`, an `OtpDelivery` and a `LoginAlert`
//! and hands each one back. Until something sends them, the commonest journey
//! in that crate -- a password reset -- ends in a value nobody consumes, and
//! the test that proves the reset link reached the right address cannot be
//! written at all.
//!
//! This is that something, for the test. [`Memory`] accepts every message,
//! keeps it, and lets a test ask what was sent, to whom, and what the body
//! said. An auth test asserts that a reset for an address with no account sent
//! nothing, and that one for an address with an account sent exactly one
//! message, to that address, carrying that token -- which is the assertion the
//! whole enumeration argument in `accounts.rs` rests on and which nothing can
//! make today.
//!
//! # It is not a development mailer
//!
//! `brocanth-jobs` refuses to ship an in-memory queue, because a queue that is
//! a different implementation in development is the trap it exists to avoid.
//! The same argument does not apply here, and it is worth saying why: a queue
//! in memory is a *second implementation of the hard part* -- claiming,
//! retrying, dead-lettering -- and it will diverge. A mailer in memory
//! implements none of the hard part. It is a `Vec`. The message it records is
//! the same [`Message`], rendered by the same renderer, that the SMTP backend
//! would have written to the socket.
//!
//! It is still not where mail should go in development, because mail in
//! development should be *looked at*. Point the SMTP backend at a local catcher
//! on port 1025 for that.

use std::sync::Mutex;

use async_trait::async_trait;

use crate::error::MailError;
use crate::mailer::Mailer;
use crate::message::Message;

/// One message as it was sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sent {
    /// The message, with every field as the caller built it.
    pub message: Message,
    /// The bytes the wire would have carried.
    ///
    /// Kept as well as the message, because the two answer different
    /// questions. The message answers "did the reset go to the right
    /// address"; the bytes answer "is there a `Bcc:` in this header block",
    /// which is the question a header-injection test has to ask.
    pub rendered: Vec<u8>,
}

impl Sent {
    /// The rendered message as text.
    ///
    /// Always valid UTF-8: the headers are ASCII by construction and the
    /// bodies are base64.
    pub fn as_text(&self) -> &str {
        std::str::from_utf8(&self.rendered).unwrap_or_default()
    }

    /// The header block, up to the blank line that ends it.
    pub fn headers(&self) -> &str {
        let text = self.as_text();
        match text.find("\r\n\r\n") {
            Some(end) => &text[..end],
            None => text,
        }
    }
}

/// A mailer that sends nothing and remembers everything.
#[derive(Default)]
pub struct Memory {
    sent: Mutex<Vec<Sent>>,
}

impl Memory {
    /// An empty mailbox.
    pub fn new() -> Self {
        Self::default()
    }

    /// Everything sent so far, oldest first.
    pub fn sent(&self) -> Vec<Sent> {
        self.lock().clone()
    }

    /// The most recent message.
    pub fn last(&self) -> Option<Sent> {
        self.lock().last().cloned()
    }

    /// How many messages were sent.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether nothing was sent.
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// Everything addressed to one mailbox, in `To` or in `Cc`.
    ///
    /// Compares the address and ignores the display name, which is the
    /// comparison a test means.
    pub fn to(&self, address: &str) -> Vec<Sent> {
        self.lock()
            .iter()
            .filter(|sent| {
                sent.message
                    .recipients()
                    .iter()
                    .any(|a| a.address() == address)
            })
            .cloned()
            .collect()
    }

    /// Forgets everything, so one test fixture can serve several cases.
    pub fn clear(&self) {
        self.lock().clear();
    }

    /// The lock, with a poisoned one taken anyway.
    ///
    /// A panic in a test that held this lock must not turn every later
    /// assertion into a second panic about poisoning. There is no invariant
    /// across the `Vec` to protect.
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Sent>> {
        self.sent.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl std::fmt::Debug for Memory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Memory").field("sent", &self.len()).finish()
    }
}

#[async_trait]
impl Mailer for Memory {
    async fn send(&self, message: &Message) -> Result<(), MailError> {
        let sent = Sent {
            message: message.clone(),
            // Rendered here rather than on demand, so a bug in the renderer
            // fails the test that sent the message rather than the one that
            // looked at it.
            rendered: message.render(),
        };
        self.lock().push(sent);
        Ok(())
    }
}
