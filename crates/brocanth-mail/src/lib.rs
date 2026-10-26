//! Mail for brocanth: one trait, a backend that records what it was given,
//! an SMTP backend behind a feature that is off, and headers that cannot carry
//! a newline.
//!
//! # Why this crate exists
//!
//! `brocanth-auth` says, in its own documentation, in four separate places,
//! that it sends no mail. It produces a `ResetOutcome`, a
//! `RegistrationNotice`, an `OtpDelivery` and a `LoginAlert`, and hands each
//! one back to the application with a note saying what to send. That is the
//! right shape -- the crate that hashes passwords has no business holding an
//! SMTP password -- but it leaves the commonest journey in the whole framework
//! unfinished. A password reset mints a token, puts it in a `ResetOutcome`,
//! and stops. Nothing carries it to the person who asked.
//!
//! This is the other half. It is a seam, not a feature: it takes the values
//! `brocanth-auth` already produces and it has no opinion about what is in
//! them.
//!
//! # Shape
//!
//! [`Mailer`] is the whole public contract. [`Memory`] implements it, and so
//! does [`Smtp`] behind the `smtp` feature. A third backend -- a queue-backed
//! one, an API relay -- is another type that implements the same trait, and
//! nothing above the trait changes when it arrives.
//!
//! That is `brocanth-storage`'s shape, on purpose, down to the argument: one
//! trait, one real backend, and an address type that cannot spell the thing
//! the layer below is afraid of. There it is a path; here it is a header.
//!
//! # Security decisions, each with its reason
//!
//! * **Header injection is impossible rather than filtered.** A header is
//!   built from an [`Address`] and a [`Subject`], and
//!   `victim@example.test\r\nBcc: harvest@attacker.test` is not a value of
//!   either type. There is no sanitising step to forget at a call site,
//!   because no call site takes a `&str`. See [`address`] and [`header`].
//! * **CR and LF are refused, not stripped.** A stripped newline is a value
//!   that silently became a different value; a refused one is an error at the
//!   place that produced it. This crate folds only what it wrote itself -- a
//!   long address list, a long encoded subject.
//! * **Non-ASCII is encoded, never passed through.** A subject or a display
//!   name with an accent becomes an RFC 2047 encoded-word, so a relay, a
//!   filter and a client cannot disagree about what a raw 8-bit header means.
//! * **Every body is base64.** A body is where a template puts a stranger's
//!   name. Base64 means a body cannot spell the multipart boundary, cannot
//!   exceed the 998-octet line limit, and cannot carry a bare CR. See
//!   [`message`].
//! * **The envelope is built from the recipients, never parsed back out of
//!   the headers.** A mailer that reads its own output to find out where to
//!   send it has a parser in the one place an injection would land.
//! * **There is no `Bcc` field.** A `Bcc:` header that reaches the wire is the
//!   bug every mail library has shipped once, and a Bcc done correctly is an
//!   envelope recipient with no header -- a seam worth designing rather than a
//!   fourth `Vec<Address>`.
//! * **An SMTP password is not in a `Debug`.** See [`SmtpConfig`].
//!
//! # Features
//!
//! `smtp` pulls in `lettre` and the TLS stack, pool and reactor under it, and
//! nothing else does. §8.2 asks that a feature which is off means the code is
//! absent, and this is the clearest case in the crate: the backend an
//! application wants in its test suite is [`Memory`], and it must not pay for
//! a relay it does not talk to.
//!
//! `serde` puts `Serialize` and `Deserialize` on [`Message`], [`Address`] and
//! [`Subject`], so a message can be a `brocanth-jobs` payload. This crate does
//! not depend on `brocanth-jobs` and never will -- a mailer does not need a
//! queue -- but a `Message` that cannot serialise would make the queue
//! impossible from outside, and that is a different thing from not needing it.
//! Deserialising re-runs every validator, because a derived `Deserialize` on a
//! validated newtype hands the attacker back the constructor we took away.
//!
//! # What `brocanth-auth` does with it
//!
//! The crate stays as it is. The application wires the two together, which is
//! §8's rule and also the only place that knows what the reset link looks
//! like:
//!
//! ```no_run
//! # use std::sync::Arc;
//! # use brocanth_mail::{Address, Mail, MailError, Memory};
//! # struct Account;
//! # impl Account { fn email(&self) -> Option<String> { None } }
//! # enum ResetOutcome { Issue { account: Account, token: String }, Nothing }
//! # async fn example(mail: &Mail, outcome: ResetOutcome) -> Result<(), MailError> {
//! match outcome {
//!     // The response the browser gets is the same either way. This decides
//!     // only what the mailer does, and the mailer is not an oracle because
//!     // the attacker does not receive it.
//!     ResetOutcome::Nothing => Ok(()),
//!     ResetOutcome::Issue { account, token } => {
//!         let to = Address::parse(&account.email().unwrap_or_default())?;
//!         let message = mail
//!             .message()
//!             .to(to)
//!             .subject("Reset your password")
//!             .text(format!("https://example.test/reset?t={token}"))
//!             .build()?;
//!         mail.send(&message).await
//!     }
//! }
//! # }
//! ```
//!
//! # Example
//!
//! ```
//! use brocanth_mail::{Address, Mailer, Memory, Message};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let message = Message::builder(Address::named("Example", "no-reply@example.test")?)
//!     .to(Address::parse("person@example.test")?)
//!     .subject("Reset your password")
//!     .text("Open https://example.test/reset?t=abc")
//!     .html("<a href=\"https://example.test/reset?t=abc\">Reset</a>")
//!     .build()?;
//!
//! // A newline in a subject is not a subject.
//! assert!(Message::builder(Address::parse("a@example.test")?)
//!     .to(Address::parse("b@example.test")?)
//!     .subject("Hello\r\nBcc: harvest@attacker.test")
//!     .build()
//!     .is_err());
//!
//! // `Mailer::send` is async, so sending needs an executor. What it would
//! // put on the wire needs none.
//! let mailer: &dyn Mailer = &Memory::new();
//! let _ = mailer;
//!
//! let wire = String::from_utf8(message.render())?;
//! assert!(wire.contains("Subject: Reset your password\r\n"));
//! assert!(wire.contains("Content-Type: multipart/alternative"));
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]

pub mod address;
pub mod error;
pub mod header;
pub mod mailer;
pub mod memory;
pub mod message;
pub mod plugin;

#[cfg(feature = "smtp")]
pub mod smtp;

#[cfg(feature = "serde")]
mod serde_wire;

#[cfg(test)]
mod rt;

#[cfg(test)]
mod tests;

pub use address::{Address, MAX_ADDRESS, MAX_DISPLAY_NAME, MAX_DOMAIN, MAX_LOCAL_PART};
pub use error::MailError;
pub use mailer::Mailer;
pub use memory::{Memory, Sent};
pub use message::{Message, MessageBuilder, Subject, MAX_SUBJECT};
pub use plugin::{Mail, MailConfig, MailPlugin};

#[cfg(feature = "smtp")]
pub use smtp::{Security, Smtp, SmtpConfig};
