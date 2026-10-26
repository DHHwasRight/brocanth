//! Mail as a brocanth plugin.
//!
//! The arrow points this way and only this way: this crate depends on
//! `brocanth`, and `brocanth` has never heard of it. An application that sends
//! no mail does not name [`MailPlugin`] in its `main`.
//!
//! # A plugin arrives already configured
//!
//! [`MailConfig`] holds an `Arc<dyn Mailer>` the application built. There is
//! no relay host in it, no port, no password and no `from` address parsed from
//! a string at startup, because the core never parses anything and a plugin
//! that reads its own environment is a plugin that configures itself behind
//! the application's back. Choosing `Memory` or `Smtp` is one line in `main`,
//! it is visible there, and `brocanth-config` can write that line's input from
//! a TOML section exactly as it does for every other plugin.
//!
//! # Two ways to reach it, because there are two kinds of caller
//!
//! * **A handler** extracts [`Mail`] from the per-request context:
//!   `let mail: &Mail = cx.extract();`. A handler in an application that never
//!   registered the plugin does not compile.
//! * **Another plugin** asks the application's services for it:
//!   `lc.require_service::<Mail>()?` in its own `after_config`. That is the
//!   entry `brocanth-auth` needs, and `require_service` is what turns "nobody
//!   registered a mailer" into a startup error that names the missing line,
//!   rather than a panic at the first password reset.
//!
//! The same [`Mail`] value goes in both places, so the two can never be
//! configured differently.
//!
//! # What this plugin does not do
//!
//! It mounts nothing and offers no middleware. Mail is not a request
//! concern, and a plugin that mounted a route would be deciding a pattern,
//! which is the application's call.

use std::sync::Arc;

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::{ContextBuilder, ContextItem, ProvidesContext};

use crate::address::Address;
use crate::error::MailError;
use crate::mailer::Mailer;
use crate::message::{Message, MessageBuilder};

/// What [`MailPlugin`] is built from.
pub struct MailConfig {
    /// The backend. [`crate::Memory`] in a test, [`crate::Smtp`] behind the
    /// `smtp` feature in production, or anything else that implements
    /// [`Mailer`].
    pub mailer: Arc<dyn Mailer>,
    /// Who mail is from when the caller does not say.
    ///
    /// An [`Address`], so a misconfigured sender fails in the application's
    /// own startup code with the application's own error handling, rather than
    /// at the first message.
    pub default_from: Address,
}

struct Inner {
    mailer: Arc<dyn Mailer>,
    default_from: Address,
}

/// The service entry and the context item: the mailer, and the address it
/// sends from.
///
/// Cloning is cheap and every clone shares the one backend, which is the
/// point: an SMTP connection pool per request is not a pool.
#[derive(Clone)]
pub struct Mail(Arc<Inner>);

impl Mail {
    /// Builds the service outside any application, which is what a test wants.
    pub fn new(mailer: Arc<dyn Mailer>, default_from: Address) -> Self {
        Self(Arc::new(Inner {
            mailer,
            default_from,
        }))
    }

    /// The backend, for a caller that wants to hand it somewhere else.
    pub fn mailer(&self) -> &Arc<dyn Mailer> {
        &self.0.mailer
    }

    /// The address mail is from.
    pub fn default_from(&self) -> &Address {
        &self.0.default_from
    }

    /// A message builder with `From` already filled in.
    ///
    /// This is the call site the rest of the framework is written for: a
    /// controller composes a message and never names the sender, so changing
    /// the sender is one line in `main` and not a search across every mail a
    /// template sends.
    pub fn message(&self) -> MessageBuilder {
        Message::builder(self.0.default_from.clone())
    }

    /// Sends through the configured backend.
    ///
    /// # Errors
    /// Whatever the backend answered.
    pub async fn send(&self, message: &Message) -> Result<(), MailError> {
        self.0.mailer.send(message).await
    }
}

impl std::fmt::Debug for Mail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mail")
            .field("default_from", &self.0.default_from)
            .finish()
    }
}

impl ContextItem for Mail {}

/// The plugin.
///
/// ```no_run
/// use std::sync::Arc;
/// use brocanth::plugin_api::Plugin;
/// use brocanth::plugin_app::AppBuilder;
/// use brocanth_mail::{Address, MailConfig, MailPlugin, Memory};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let app = AppBuilder::new()
///     .plugin(MailPlugin::new(MailConfig {
///         mailer: Arc::new(Memory::new()),
///         default_from: Address::named("Example", "no-reply@example.test")?,
///     }))
///     .build()?;
/// # let _ = app;
/// # Ok(())
/// # }
/// ```
pub struct MailPlugin {
    mail: Mail,
}

impl MailPlugin {
    /// The service, for an application that wants it without the context.
    pub fn mail(&self) -> &Mail {
        &self.mail
    }
}

impl Plugin for MailPlugin {
    type Config = MailConfig;
    type Provides = ProvidesContext<Mail>;
    const NAME: &'static str = "mail";

    fn new(config: Self::Config) -> Self {
        Self {
            mail: Mail::new(config.mailer, config.default_from),
        }
    }

    fn register(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        // Published in `register`, which is the hook that exists for it, so
        // that any plugin asking in `after_config` finds it whatever order the
        // two were named in.
        lc.services_mut().insert(self.mail.clone());
        Ok(())
    }

    fn provide(&self, cx: &mut ContextBuilder) {
        cx.insert(self.mail.clone());
    }
}
