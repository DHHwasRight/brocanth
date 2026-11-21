//! The backoffice as a service of its own (docs/backoffice.md §7).
//!
//! Two things, one per half of the decision that the backoffice is a
//! second service, embedded first and split later.
//!
//! **The tab data contract** ([`contract`]). The admin's model screens
//! reach their rows through [`TabData`]: one call of owned serde types,
//! with the caller's [`Claim`] (the RBAC grants and the identity) on each.
//! [`InProcess`] is the data side over any `AdminData`; [`Contracted`] is
//! the admin's side, an `AdminData` over any `TabData`. A project puts its
//! admin on the contract with one line:
//!
//! ```
//! use brocanth_admin::{AdminConfig, Fixture};
//! use brocanth_backoffice::{Contracted, InProcess};
//!
//! let config = AdminConfig::new()
//!     .data_through(|data| Contracted::new(InProcess::new(data)).shared());
//! let admin = brocanth_admin::Admin::try_new(std::sync::Arc::new(Fixture::blog()), config);
//! assert!(admin.is_ok());
//! ```
//!
//! [`Json`] puts a JSON codec between the two ends, so a test proves that
//! every call and answer crosses a wire. The wire itself comes later.
//!
//! **The second binary** ([`gate`]). A project with the `backoffice` module
//! builds `src/bin/backoffice.rs`, which runs the application itself and
//! serves only the admin's mount. [`Gate`] decides which paths a process
//! answers, from its [`Role`].
//!
//! The module that writes all of it into a project is [`scaffold`], behind
//! the off-by-default `scaffold` feature.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

#[cfg(feature = "runtime")]
pub mod client;
#[cfg(feature = "runtime")]
pub mod contract;
#[cfg(feature = "runtime")]
pub mod gate;
#[cfg(feature = "runtime")]
pub mod json;
#[cfg(feature = "runtime")]
pub mod server;

/// What `brocanth create` writes for `backoffice`. Only the CLI reads it.
#[cfg(feature = "scaffold")]
pub mod scaffold;

#[cfg(feature = "runtime")]
pub use brocanth_admin::policy::Level;
#[cfg(feature = "runtime")]
pub use client::{ClaimFn, Contracted};
#[cfg(feature = "runtime")]
pub use contract::{
    Answer, Call, Caller, Claim, ErrorKind, Op, Read, Scope, Search, SharedTabs, TabData, TabError,
    TabTransaction, Write,
};
#[cfg(feature = "runtime")]
pub use gate::{check_addr, role, serve_as, Gate, Role};
#[cfg(feature = "runtime")]
pub use json::Json;
#[cfg(feature = "runtime")]
pub use server::InProcess;
