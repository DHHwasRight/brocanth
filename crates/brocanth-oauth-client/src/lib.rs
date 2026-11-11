#![cfg_attr(feature = "runtime", doc = include_str!("../README.md"))]
//!
//! # Design notes
//!
//! * [`provider`]: a provider is data. Four are written down
//!   ([`endpoints`]), and a custom one is a [`provider::Endpoint`] with three
//!   URLs.
//! * [`OauthClient`]: the connect flow, the token source and the
//!   authenticated HTTP client. It holds the store, the key and the
//!   providers.
//! * [`store`]: what a store keeps, and [`MemoryTokenStore`]. A store sees
//!   only sealed tokens: the client encrypts before `put` and decrypts after
//!   `get`, so a store author cannot forget the encryption.
//! * `postgres` (feature `postgres`): [`PgTokenStore`].
//! * [`web`]: the connect pages as a function from a request to an answer,
//!   with no web framework assumed.
//! * `admin` (feature `admin`): the connections tab.
//! * `conformance` (feature `conformance`): the store contract as one
//!   async function.

#![forbid(unsafe_code)]

#[cfg(feature = "runtime")]
mod authorized;
#[cfg(feature = "runtime")]
mod client;
#[cfg(feature = "runtime")]
pub mod clock;
#[cfg(feature = "runtime")]
mod error;
#[cfg(feature = "runtime")]
pub mod http;
#[cfg(feature = "runtime")]
pub mod key;
#[cfg(feature = "runtime")]
pub mod provider;
#[cfg(feature = "runtime")]
pub mod store;
#[cfg(feature = "runtime")]
pub mod web;

#[cfg(feature = "postgres")]
pub mod postgres;

#[cfg(feature = "admin")]
pub mod admin;

#[cfg(feature = "conformance")]
pub mod conformance;

#[cfg(feature = "scaffold")]
pub mod scaffold;

#[cfg(feature = "runtime")]
pub use authorized::Authorized;
#[cfg(feature = "runtime")]
pub use client::{Builder, Completed, OauthClient, Started, Token};
#[cfg(feature = "runtime")]
pub use clock::{Clock, SystemClock};
#[cfg(feature = "runtime")]
pub use error::{Error, FlowError};
#[cfg(feature = "runtime")]
pub use key::Key;
#[cfg(feature = "runtime")]
pub use provider::{endpoints, AuthStyle, Endpoint, Provider};
#[cfg(feature = "runtime")]
pub use store::{Connection, MemoryTokenStore, StoreError, StoredToken, TokenStore};

#[cfg(feature = "postgres")]
pub use postgres::PgTokenStore;

/// The HTTP client this crate calls providers with, so an application
/// builds its API requests with the same version.
#[cfg(feature = "runtime")]
pub use reqwest;
