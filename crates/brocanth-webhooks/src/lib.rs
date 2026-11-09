#![cfg_attr(feature = "runtime", doc = include_str!("../README.md"))]
//!
//! # Design notes
//!
//! Two directions, one crate, one signing module.
//!
//! * [`sign`]: Standard Webhooks, Stripe and plain HMAC, verified in
//!   constant time with a replay window. The outgoing side signs with the
//!   same code the incoming side verifies.
//! * [`incoming`]: a request hook that answers `POST {path}/{source}`
//!   before the CSRF check, so a webhook needs no exempt path. A typed
//!   [`Handler`] per source, written by the application.
//! * [`seen`]: the idempotency keys. In memory, or in Postgres with the
//!   `postgres` feature ([`PgSeen`]).
//! * `outgoing` (feature `out`): endpoints, deliveries, and a
//!   `brocanth-jobs` job per delivery, so retries and backoff are the
//!   queue's.
//! * `admin` (feature `admin`): the deliveries tab.

#![forbid(unsafe_code)]

#[cfg(feature = "runtime")]
pub mod incoming;
#[cfg(feature = "runtime")]
pub mod seen;
#[cfg(feature = "runtime")]
pub mod sign;

#[cfg(feature = "postgres")]
pub mod postgres;

#[cfg(feature = "out")]
pub mod outgoing;

#[cfg(feature = "admin")]
pub mod admin;

#[cfg(feature = "scaffold")]
pub mod scaffold;

#[cfg(feature = "runtime")]
pub use incoming::{Answer, Delivery, Handler, Incoming, Receiver, Rejection};
#[cfg(feature = "runtime")]
pub use seen::{MemorySeen, Seen, SeenError};
#[cfg(feature = "runtime")]
pub use sign::{Refused, Scheme, SecretError, Verified};

#[cfg(feature = "postgres")]
pub use postgres::PgSeen;

#[cfg(feature = "out")]
pub use outgoing::{
    Deliver, DeliveryRow, Endpoint, OutError, Outgoing, OutgoingConfig, OutgoingPlugin, WebhookSink,
};
