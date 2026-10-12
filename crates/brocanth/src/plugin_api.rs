//! The plugin contract: five lifecycle hooks, one typed configuration value,
//! and nothing that happens behind the application's back.
//!
//! Two rules shape this file, both inherited from the Crystal original's
//! author.
//!
//! **Plugins do not autowire.** *"I do not want plugins to register middleware
//! themselves. I want to manually inject middlewares since I may want to
//! configure the pattern."* So a plugin can *offer* middleware
//! ([`MiddlewareCatalog`]) and cannot mount it. Mounting takes a pattern, the
//! pattern is a decision, and the decision belongs to the application.
//!
//! **A plugin arrives already configured.** A plugin names the type it is built
//! from ([`Plugin::Config`]) and the application constructs that value and
//! hands it over: `app.plugin(LoggerPlugin::new(LoggerConfig { .. }))`. The
//! core never parses anything, never reads a file and never looks a section up
//! by name — a service configured entirely from its environment does not link
//! a TOML parser to say so. Reading those values out of a file is
//! `brocanth-config`, which is a plugin like any other.

use std::any::{type_name, Any, TypeId};
use std::collections::HashMap;

use thiserror::Error;

use crate::plugin_context::{ContextBuilder, Provided};

/// Anything a hook can go wrong with.
#[derive(Debug, Error)]
#[error("{0}")]
pub struct PluginError(pub String);

impl PluginError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl From<&str> for PluginError {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for PluginError {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// The stages of an application build, in the order they run.
///
/// `Listen` is the HTTP server's, not this crate's: the build stops with a
/// finished application and the server layer takes it from there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Configure,
    Register,
    AfterConfig,
    UserBlock,
    ConfigureRouter,
    BeforeStart,
    Listen,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Configure => "configure",
            Stage::Register => "register",
            Stage::AfterConfig => "after_config",
            Stage::UserBlock => "user_block",
            Stage::ConfigureRouter => "configure_router",
            Stage::BeforeStart => "before_start",
            Stage::Listen => "listen",
        }
    }
}

impl std::fmt::Display for Stage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Services: the shared, per-application store
// ---------------------------------------------------------------------------

/// A typed store that plugins use to hand each other values.
///
/// It belongs to one application. The Crystal original kept the equivalent in
/// class variables, so two applications in one process shared one store and
/// its own test suite needed a `reset!` between cases. There is nothing here to
/// reset.
#[derive(Default)]
pub struct Services {
    map: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl Services {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert<T: Send + Sync + 'static>(&mut self, value: T) -> Option<T> {
        self.map
            .insert(TypeId::of::<T>(), Box::new(value))
            .and_then(|b| b.downcast::<T>().ok().map(|b| *b))
    }

    pub fn get<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.map
            .get(&TypeId::of::<T>())
            .and_then(|b| b.downcast_ref::<T>())
    }

    pub fn get_mut<T: Send + Sync + 'static>(&mut self) -> Option<&mut T> {
        self.map
            .get_mut(&TypeId::of::<T>())
            .and_then(|b| b.downcast_mut::<T>())
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

impl std::fmt::Debug for Services {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Services")
            .field("len", &self.len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// What a hook can see
// ---------------------------------------------------------------------------

/// What a plugin instance can reach during a lifecycle hook.
///
/// Everything here hangs off one application. There is no path from a hook to
/// another application's state.
pub struct Lifecycle<'a> {
    stage: Stage,
    instance: &'a str,
    primary: bool,
    instances: &'a [String],
    services: &'a mut Services,
}

impl<'a> Lifecycle<'a> {
    pub(crate) fn new(
        stage: Stage,
        instance: &'a str,
        primary: bool,
        instances: &'a [String],
        services: &'a mut Services,
    ) -> Self {
        Self {
            stage,
            instance,
            primary,
            instances,
            services,
        }
    }

    /// The stage currently running.
    pub fn stage(&self) -> Stage {
        self.stage
    }

    /// The name this instance was registered under.
    pub fn instance(&self) -> &str {
        self.instance
    }

    /// Whether this is the first registered instance of its plugin type.
    pub fn is_primary(&self) -> bool {
        self.primary
    }

    /// Every registered instance name, in registration order.
    ///
    /// A plugin that polices the application as a whole needs the roster:
    /// `brocanth-config` reads it to reject a configuration section that no
    /// instance claims.
    pub fn instance_names(&self) -> &[String] {
        self.instances
    }

    pub fn services(&self) -> &Services {
        self.services
    }

    pub fn services_mut(&mut self) -> &mut Services {
        self.services
    }
}

// ---------------------------------------------------------------------------
// Middleware: offered, never mounted
// ---------------------------------------------------------------------------

/// Middleware a plugin makes available. The application decides whether and
/// where to mount it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MiddlewareOffer {
    /// The name the application mounts by.
    pub name: String,
    /// The plugin instance that offered it.
    pub offered_by: String,
    /// One line for humans reading an error or a `brocanth routes` dump.
    pub description: String,
}

/// The only router-facing thing a plugin gets.
///
/// It can add to the catalogue. It cannot mount, because mounting needs a
/// pattern and the pattern is the application's call.
pub struct MiddlewareCatalog<'a> {
    offers: &'a mut Vec<MiddlewareOffer>,
    instance: &'a str,
}

impl<'a> MiddlewareCatalog<'a> {
    pub(crate) fn new(offers: &'a mut Vec<MiddlewareOffer>, instance: &'a str) -> Self {
        Self { offers, instance }
    }

    /// Publishes middleware under `name`.
    pub fn offer(&mut self, name: impl Into<String>, description: impl Into<String>) {
        self.offers.push(MiddlewareOffer {
            name: name.into(),
            offered_by: self.instance.to_string(),
            description: description.into(),
        });
    }

    /// The instance publishing.
    pub fn instance(&self) -> &str {
        self.instance
    }

    /// What has been offered so far.
    pub fn offers(&self) -> &[MiddlewareOffer] {
        self.offers
    }
}

// ---------------------------------------------------------------------------
// The trait
// ---------------------------------------------------------------------------

/// A plugin.
///
/// Registration is explicit and typed: the application names the plugin type,
/// so the compiler knows the configuration type and the context item before
/// anything runs.
///
/// Hooks run in the recorded order — `configure`, `register`, `after_config`,
/// then the application's own setup block, then `configure_router`, then
/// `before_start` — and every hook runs for every instance before the next
/// hook starts.
pub trait Plugin: Sized + Send + Sync + 'static {
    /// The value this plugin is built from.
    ///
    /// A plain value with no bounds on it: the core neither parses it nor
    /// stores it, so it is free to be an enum, a connection pool or `()`. Where
    /// it comes from is the application's business — a literal in `main`, the
    /// environment, or a TOML section read by `brocanth-config`.
    type Config;

    /// What this plugin puts in the per-request context: [`NoContext`], or
    /// [`ProvidesContext<T>`].
    ///
    /// [`NoContext`]: crate::plugin_context::NoContext
    /// [`ProvidesContext<T>`]: crate::plugin_context::ProvidesContext
    type Provides: Provided;

    /// Default instance name.
    const NAME: &'static str;

    /// Builds the plugin from its configuration.
    ///
    /// The application calls this itself — `LoggerPlugin::new(LoggerConfig {
    /// level: Level::Warn })` — and so does any crate that resolves
    /// configuration on the application's behalf. It is the only way a
    /// configuration value reaches a plugin, which is what lets the core know
    /// nothing about configuration at all.
    fn new(config: Self::Config) -> Self;

    /// The application is building. Settle internal state before any other
    /// plugin can look.
    ///
    /// The configuration is not a parameter here: the plugin was constructed
    /// from it and already holds whatever it kept.
    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        Ok(())
    }

    /// Publish services other plugins may want.
    fn register(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        Ok(())
    }

    /// Every plugin is configured and registered. Cross-plugin lookups are
    /// valid from here.
    fn after_config(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        Ok(())
    }

    /// Offer middleware. Mounting it is the application's decision, taken with
    /// a pattern, in its own setup block.
    fn configure_router(
        &mut self,
        _catalog: &mut MiddlewareCatalog<'_>,
    ) -> Result<(), PluginError> {
        Ok(())
    }

    /// Last chance before the server is handed the application.
    fn before_start(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        Ok(())
    }

    /// Fill the per-request context.
    ///
    /// Called once per context, and only for the instance whose registration
    /// widened the provider list — a context item is one value, so extra
    /// instances of the same plugin do not fight over it.
    fn provide(&self, _cx: &mut ContextBuilder) {}
}

// ---------------------------------------------------------------------------
// Erasure, so the application can hold a heterogeneous list
// ---------------------------------------------------------------------------

pub(crate) struct Instance<P: Plugin> {
    pub(crate) name: String,
    pub(crate) plugin: P,
}

/// Object-safe view of one registered instance.
pub(crate) trait AnyInstance: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn plugin_type(&self) -> TypeId;
    fn plugin_type_name(&self) -> &'static str;
    fn configure(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError>;
    fn register(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError>;
    fn after_config(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError>;
    fn configure_router(&mut self, catalog: &mut MiddlewareCatalog<'_>) -> Result<(), PluginError>;
    fn before_start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError>;
    fn provide(&self, cx: &mut ContextBuilder);
    fn plugin_any(&self) -> &dyn Any;
    fn plugin_any_mut(&mut self) -> &mut dyn Any;
}

impl<P: Plugin> AnyInstance for Instance<P> {
    fn name(&self) -> &str {
        &self.name
    }

    fn plugin_type(&self) -> TypeId {
        TypeId::of::<P>()
    }

    fn plugin_type_name(&self) -> &'static str {
        type_name::<P>()
    }

    fn configure(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        self.plugin.configure(lc)
    }

    fn register(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        self.plugin.register(lc)
    }

    fn after_config(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        self.plugin.after_config(lc)
    }

    fn configure_router(&mut self, catalog: &mut MiddlewareCatalog<'_>) -> Result<(), PluginError> {
        self.plugin.configure_router(catalog)
    }

    fn before_start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        self.plugin.before_start(lc)
    }

    fn provide(&self, cx: &mut ContextBuilder) {
        self.plugin.provide(cx);
    }

    fn plugin_any(&self) -> &dyn Any {
        &self.plugin
    }

    fn plugin_any_mut(&mut self) -> &mut dyn Any {
        &mut self.plugin
    }
}
