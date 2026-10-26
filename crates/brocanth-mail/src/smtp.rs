//! The SMTP backend, behind a feature that is off.
//!
//! # Why it is off
//!
//! §8.2 asks that a feature which is off means the code is absent rather than
//! branched around. With `smtp` off there is no `lettre`, no `rustls`, no
//! `tokio` and no connection pool in this crate's tree: `cargo tree -p
//! brocanth-mail -e normal --depth 1` lists six crates. The backend that an
//! application wants in its test suite is [`Memory`](crate::Memory), and it
//! must not pay a TLS stack for one.
//!
//! # Why `lettre` and not a socket
//!
//! Because the hard part of SMTP is not the dialogue. It is STARTTLS, the
//! AUTH mechanisms, certificate verification against a root store, the
//! connection pool, and the long tail of relays that answer a `250` with a
//! continuation line nobody expected. That is a wire protocol library, and
//! there is a maintained one.
//!
//! What `lettre` does **not** do here is build the message. This crate renders
//! it ([`Message::render`]), and hands `lettre` the finished bytes through
//! `send_raw`. That is why `default-features = false` drops lettre's
//! `builder`: the header rules in [`crate::header`] are the security case of
//! this crate, and a message that went out through a second builder would not
//! have been through them. One renderer, used by every backend, exercised by
//! every test.
//!
//! `rustls` and not `native-tls`, because a native TLS stack is the other half
//! of the OpenSSL link failure nobody can fix from their own `Cargo.toml`, and
//! because `brocanth-auth` made the same call for `reqwest`.
//!
//! # The envelope is not the headers
//!
//! The recipients `lettre` is given come from [`Message::recipients`], which
//! is `To` then `Cc`. They are not read back out of the rendered header block.
//! A mailer that parses its own output to find out where to send it is a
//! mailer with a parser in the one place an injection would land.

use std::time::Duration;

use async_trait::async_trait;
use lettre::address::Envelope;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Address as LettreAddress, AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

use crate::error::MailError;
use crate::mailer::Mailer;
use crate::message::Message;

/// How the connection to the relay is protected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Security {
    /// TLS from the first byte, port 465. The default, and the one to use.
    ImplicitTls,
    /// Plain connection upgraded with STARTTLS, port 587.
    ///
    /// Weaker than [`Security::ImplicitTls`] in one specific way: the upgrade
    /// is announced in cleartext, so an attacker on the path can strip the
    /// announcement and watch the session fall back. `lettre` refuses to send
    /// when the upgrade does not happen, which closes that, but implicit TLS
    /// never had the window.
    StartTls,
    /// No TLS at all.
    ///
    /// For a mail catcher on `localhost:1025` in development, and for nothing
    /// else. The name is long on purpose.
    PlaintextForLocalCatcherOnly,
}

/// How to reach the relay.
///
/// Its `Debug` prints whether there are credentials and never what they are.
/// A relay password in a boot log is a relay password in a log aggregator.
#[derive(Clone)]
pub struct SmtpConfig {
    /// The relay host.
    pub relay: String,
    /// The port, when it is not the default for [`SmtpConfig::security`].
    pub port: Option<u16>,
    /// How the connection is protected.
    pub security: Security,
    /// The username and password, when the relay wants them.
    ///
    /// A plain `String` pair, because that is what the SASL exchange sends.
    /// Keeping it out of `Debug` is this struct's job and it is done below.
    pub credentials: Option<(String, String)>,
    /// How long to wait on the relay before giving up.
    pub timeout: Duration,
}

impl SmtpConfig {
    /// A relay reached over implicit TLS, with no credentials.
    pub fn new(relay: impl Into<String>) -> Self {
        Self {
            relay: relay.into(),
            port: None,
            security: Security::ImplicitTls,
            credentials: None,
            timeout: Duration::from_secs(30),
        }
    }

    /// Adds the username and password.
    #[must_use]
    pub fn with_credentials(mut self, username: impl Into<String>, password: impl Into<String>) -> Self {
        self.credentials = Some((username.into(), password.into()));
        self
    }

    /// Changes how the connection is protected.
    #[must_use]
    pub fn with_security(mut self, security: Security) -> Self {
        self.security = security;
        self
    }

    /// Changes the port.
    #[must_use]
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }
}

impl std::fmt::Debug for SmtpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpConfig")
            .field("relay", &self.relay)
            .field("port", &self.port)
            .field("security", &self.security)
            .field("credentials", &self.credentials.is_some())
            .finish()
    }
}

/// A mailer that talks to a relay.
pub struct Smtp {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    relay: String,
}

impl Smtp {
    /// Opens the transport.
    ///
    /// Nothing connects here: `lettre` pools lazily, so a relay that is down
    /// at boot does not stop the application from starting. What this does do
    /// is refuse a relay name that is not usable, at startup, in the
    /// application's own code.
    ///
    /// # Errors
    /// [`MailError::Transport`] when the relay name or the TLS parameters are
    /// not usable.
    pub fn connect(config: &SmtpConfig) -> Result<Self, MailError> {
        let builder = match config.security {
            Security::ImplicitTls => AsyncSmtpTransport::<Tokio1Executor>::relay(&config.relay),
            Security::StartTls => {
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.relay)
            }
            Security::PlaintextForLocalCatcherOnly => Ok(
                AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(config.relay.clone()),
            ),
        }
        .map_err(|e| MailError::transport(e.to_string()))?;

        let mut builder = builder.timeout(Some(config.timeout));
        if let Some(port) = config.port {
            builder = builder.port(port);
        }
        if let Some((username, password)) = &config.credentials {
            builder = builder.credentials(Credentials::new(username.clone(), password.clone()));
        }
        Ok(Self {
            transport: builder.build(),
            relay: config.relay.clone(),
        })
    }
}

impl std::fmt::Debug for Smtp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Smtp").field("relay", &self.relay).finish()
    }
}

#[async_trait]
impl Mailer for Smtp {
    async fn send(&self, message: &Message) -> Result<(), MailError> {
        let from = parse_for_envelope(message.from().address())?;
        let mut to = Vec::with_capacity(message.to().len() + message.cc().len());
        for recipient in message.recipients() {
            to.push(parse_for_envelope(recipient.address())?);
        }
        let envelope = Envelope::new(Some(from), to)
            .map_err(|e| MailError::transport(format!("the envelope is not usable: {e}")))?;

        self.transport
            .send_raw(&envelope, &message.render())
            .await
            .map(|_| ())
            .map_err(|e| MailError::transport(e.to_string()))
    }
}

/// Re-parses an address this crate already checked, into lettre's own type.
///
/// It cannot fail in practice -- [`crate::Address`] is strictly narrower than
/// what `lettre` accepts -- and it is still a `Result` rather than an
/// `expect`, because "cannot fail in practice" is how a mailer panics in
/// production.
fn parse_for_envelope(address: &str) -> Result<LettreAddress, MailError> {
    address
        .parse::<LettreAddress>()
        .map_err(|_| MailError::address("the relay would not accept it"))
}
