//! The tab data contract over HTTP (docs/backoffice.md §7.5).
//!
//! The application serves it with a [`WireServer`]: `POST` [`PATH`], each
//! request signed with the service secret, each call run by
//! `InProcess::enforcing_grants()` as the claim's database identity. The
//! backoffice binary calls it with a [`WireClient`], a [`TabData`] like any
//! other, so `Contracted` puts the admin on it unchanged.
//!
//! ```text
//! backoffice binary                          application
//! Contracted -> WireClient --POST /_backoffice/wire--> WireServer -> InProcess -> AdminData
//!                 signed: HMAC-SHA256(secret, time, nonce, path, body)
//! ```
//!
//! The body is an [`Ask`] in JSON; the answer is `Result<Reply, TabError>`
//! in JSON, signed over the request's nonce. The codec is the one
//! [`crate::Json`] proves in one process.
//!
//! [`TabData`]: crate::TabData

mod client;
mod server;
mod sign;

pub use client::{WireClient, WirePanels};
pub use server::WireServer;
pub use sign::{sign, ServiceSecret, Signed};

use std::time::Duration;

use brocanth_admin::discovery::RegistryEntry;
use serde::{Deserialize, Serialize};

use crate::contract::{Answer, Call, Caller, Claim, TabError, Write};

/// The path the application answers the wire at.
pub const PATH: &str = "/_backoffice/wire";

/// The protocol's version. `Hello` carries it; the client refuses another.
pub const VERSION: u32 = 1;

/// The header that carries the time of signing, in Unix seconds.
pub const TIME_HEADER: &str = "x-backoffice-time";
/// The header that carries the request's nonce: 32 hex digits.
pub const NONCE_HEADER: &str = "x-backoffice-nonce";
/// The header that carries the signature: 64 hex digits.
pub const SIGNATURE_HEADER: &str = "x-backoffice-signature";

/// How far the time of a request may be from the server's clock. A nonce
/// is remembered for as long.
pub const WINDOW: Duration = Duration::from_secs(60);

/// How long the server keeps a transaction nobody uses, when nothing says
/// else: `[backoffice] tx_timeout_secs`.
pub const TX_TIMEOUT: Duration = Duration::from_secs(30);

/// How many transactions the server keeps open at most, when nothing says
/// else: `[backoffice] tx_max`.
pub const TX_MAX: usize = 32;

/// What the backoffice asks, as the body of one request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Ask {
    /// The first request: the registry and the data side's check (W7).
    Hello {
        /// The client's [`VERSION`].
        version: u32,
    },
    /// One call outside a transaction.
    Call(Call),
    /// Opens a transaction for the claim.
    Begin(Claim),
    /// One write inside a transaction the server holds.
    InTx {
        /// The id `Begin` answered.
        tx: String,
        /// Who asks. The subject must be the one that opened it.
        caller: Caller,
        /// The write.
        write: Write,
    },
    /// Keeps every write of a transaction, and ends it.
    Commit {
        /// The id `Begin` answered.
        tx: String,
    },
    /// Discards every write of a transaction, and ends it.
    Rollback {
        /// The id `Begin` answered.
        tx: String,
    },
    /// One call of a panel's own data contract (§7.6), as its JSON text.
    Panel {
        /// The panel's tab: its RBAC resource.
        tab: String,
        /// Who asks.
        claim: Claim,
        /// The panel's call, as the panel's crate encodes it.
        call: String,
    },
}

/// What the application answers, when the answer is not an error.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Reply {
    /// To `Hello`.
    Hello {
        /// The server's [`VERSION`].
        version: u32,
        /// Every model the data side holds.
        registry: Vec<RegistryEntry>,
        /// The data side's start-time check: `None` when it passed.
        check: Option<TabError>,
    },
    /// To a call, a write in a transaction, a commit or a rollback.
    Answer(Answer),
    /// To `Begin`: the transaction's id.
    Opened {
        /// The id.
        tx: String,
    },
    /// To a panel's call: the panel's answer, as its JSON text.
    Panel(String),
}

/// What a call through the wire answers the client with.
pub type Replied = Result<Reply, TabError>;

/// Checks the wire's settings at start, in both binaries (W3), and
/// returns the secret when either half is on.
///
/// `wire` is `[backoffice] wire`, `data_url` is `[backoffice] data_url`,
/// `secret` is `[backoffice] service_secret`.
///
/// # Errors
///
/// A missing or short secret, a `data_url` that is not an `http://` or
/// `https://` URL, a zero timeout or cap; each with a `help:` line.
pub fn check_settings(
    wire: bool,
    data_url: Option<&str>,
    secret: Option<&str>,
    tx_timeout_secs: u64,
    tx_max: usize,
) -> Result<Option<ServiceSecret>, String> {
    let data_url = data_url.filter(|url| !url.trim().is_empty());
    if let Some(url) = data_url {
        check_url(url)?;
    }
    if tx_timeout_secs == 0 {
        return Err(
            "`[backoffice] tx_timeout_secs` is 0, so a transaction would roll back \
                    before its first write\nhelp: set it to a whole number of seconds, such as 30"
                .to_string(),
        );
    }
    if tx_max == 0 {
        return Err(
            "`[backoffice] tx_max` is 0, so the wire could open no transaction\n\
                    help: set it to the number of transactions it may hold, such as 32"
                .to_string(),
        );
    }
    if !wire && data_url.is_none() {
        return Ok(None);
    }
    let needed_by = if wire {
        "`[backoffice] wire` is true"
    } else {
        "`[backoffice] data_url` is set"
    };
    match secret {
        None => Err(format!(
            "{needed_by}, and `[backoffice] service_secret` is not set\n\
             help: give both binaries the same secret of 32 bytes or more: \
             --backoffice.service_secret_file=PATH or --backoffice.service_secret_env=VAR \
             (make one with `openssl rand -hex 32`)"
        )),
        Some(secret) => ServiceSecret::parse(secret).map(Some),
    }
}

/// Checks `[backoffice] data_url`: an `http://` or `https://` URL with a
/// host.
///
/// # Errors
///
/// The problem, with a `help:` line.
pub fn check_url(url: &str) -> Result<(), String> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"));
    match rest {
        Some(host) if !host.is_empty() && !host.starts_with('/') => Ok(()),
        _ => Err(format!(
            "`[backoffice] data_url` is {url:?}, which is not an http:// or https:// URL\n\
             help: set it to where the application listens, such as http://127.0.0.1:3000"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn no_half_on_needs_no_secret() {
        assert!(matches!(
            check_settings(false, None, None, 30, 32),
            Ok(None)
        ));
        assert!(matches!(
            check_settings(false, Some(""), None, 30, 32),
            Ok(None)
        ));
    }

    #[test]
    fn either_half_needs_the_secret() {
        let error = check_settings(true, None, None, 30, 32).expect_err("no secret");
        assert!(error.contains("`[backoffice] wire` is true"), "{error}");
        assert!(error.contains("service_secret_file"), "{error}");
        assert!(error.contains("help:"), "{error}");
        let error =
            check_settings(false, Some("http://app:3000"), None, 30, 32).expect_err("no secret");
        assert!(error.contains("data_url"), "{error}");
        assert!(check_settings(true, None, Some(SECRET), 30, 32)
            .expect("a secret")
            .is_some());
    }

    #[test]
    fn a_short_secret_and_a_bad_url_are_refused() {
        let error = check_settings(true, None, Some("short"), 30, 32).expect_err("short");
        assert!(error.contains("32 bytes"), "{error}");
        let error =
            check_settings(false, Some("app:3000"), Some(SECRET), 30, 32).expect_err("no scheme");
        assert!(error.contains("http://"), "{error}");
        assert!(check_url("https://admin.example.com").is_ok());
        assert!(check_url("http:///path").is_err());
    }

    #[test]
    fn a_zero_timeout_or_cap_is_refused() {
        assert!(check_settings(true, None, Some(SECRET), 0, 32).is_err());
        assert!(check_settings(true, None, Some(SECRET), 30, 0).is_err());
    }

    #[test]
    fn every_ask_and_reply_survives_json() {
        let asks = [
            Ask::Hello { version: VERSION },
            Ask::Begin(Claim::staff("7", false, ["post.edit"])),
            Ask::Commit { tx: "a".into() },
            Ask::Rollback { tx: "a".into() },
            Ask::Panel {
                tab: "flags".into(),
                claim: Claim::anonymous(),
                call: "\"Read\"".into(),
            },
        ];
        for ask in asks {
            assert_eq!(crate::json::trip(&ask).expect("trips"), ask);
        }
        let reply: Replied = Ok(Reply::Opened { tx: "id".into() });
        assert_eq!(crate::json::trip(&reply).expect("trips"), reply);
    }
}
