//! The application: an explicit value that owns its configuration, its plugin
//! instances, its services and its router plan.
//!
//! There is no global registry here, and that is the point. The Crystal
//! original kept plugin instances in class variables, so two applications in
//! one process shared them and its own test suite needed a `reset!` between
//! cases. Everything below hangs off one [`App`] value;
//! `two_applications_in_one_process_do_not_interfere` in this file is the
//! specification of that property.
//!
//! # Order
//!
//! `build()` runs the recorded lifecycle:
//!
//! 1. `configure` — every instance, with its typed section
//! 2. `register` — every instance publishes its services
//! 3. `after_config` — every instance, cross-plugin lookups now valid
//! 4. the application's own setup block
//! 5. `configure_router` — every instance offers middleware
//! 6. `before_start` — every instance
//! 7. `listen` — not here; the HTTP server consumes the finished [`App`]
//!
//! Each stage completes for all instances before the next one starts.

use std::any::TypeId;
use std::marker::PhantomData;

use thiserror::Error;

use crate::config::{nearest_name, Config, ConfigError};
use crate::plugin_api::{
    AnyInstance, Instance, Lifecycle, MiddlewareCatalog, MiddlewareOffer, Plugin, PluginError,
    Services, Stage,
};
use crate::plugin_context::{Base, Context, ContextBuilder, Provided};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum BuildError {
    #[error(transparent)]
    Config(#[from] ConfigError),

    #[error("plugin `{instance}` failed during `{stage}`: {source}")]
    Hook {
        instance: String,
        stage: Stage,
        #[source]
        source: PluginError,
    },

    #[error("the application setup block failed: {0}")]
    Setup(#[source] PluginError),

    #[error(
        "two plugin instances are registered as `{name}`; instance names must be unique because \
         each one claims the configuration section of the same name"
    )]
    DuplicateInstance { name: String },

    #[error(
        "`{pattern}` mounts middleware `{name}`, which no registered plugin offers{}; offered: {}",
        crate::plugin_app::mount_hint(.suggestion),
        if .offered.is_empty() { "nothing".to_string() } else { .offered.join(", ") }
    )]
    UnknownMiddleware {
        pattern: String,
        name: String,
        suggestion: Option<String>,
        offered: Vec<String>,
    },
}

#[doc(hidden)]
pub fn mount_hint(suggestion: &Option<String>) -> String {
    match suggestion {
        Some(s) => format!("; did you mean `{s}`?"),
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Router plan
// ---------------------------------------------------------------------------

/// One mounted middleware: a name from the catalogue, at a pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    pub pattern: String,
    pub middleware: String,
}

/// What the application decided about the router, ready for the routing layer
/// to apply.
///
/// Plugins fill `offers`. Only the application fills `mounts`, and the type
/// system is what says so: a plugin's `configure_router` hook receives a
/// [`MiddlewareCatalog`], which has no `mount`.
#[derive(Default, Debug)]
pub struct RouterPlan {
    offers: Vec<MiddlewareOffer>,
    mounts: Vec<Mount>,
}

impl RouterPlan {
    /// Mounts an offered middleware at a pattern.
    ///
    /// The name is resolved against the catalogue at the end of the build, so
    /// the application may mount before the plugins have offered — which it
    /// always does, since the setup block runs before `configure_router`.
    pub fn mount(&mut self, pattern: impl Into<String>, middleware: impl Into<String>) {
        self.mounts.push(Mount {
            pattern: pattern.into(),
            middleware: middleware.into(),
        });
    }

    pub fn mounts(&self) -> &[Mount] {
        &self.mounts
    }

    pub fn offers(&self) -> &[MiddlewareOffer] {
        &self.offers
    }

    pub fn offer_named(&self, name: &str) -> Option<&MiddlewareOffer> {
        self.offers.iter().find(|o| o.name == name)
    }

    fn resolve(&self) -> Result<(), BuildError> {
        let offered: Vec<String> = self.offers.iter().map(|o| o.name.clone()).collect();
        for mount in &self.mounts {
            if offered.iter().any(|n| n == &mount.middleware) {
                continue;
            }
            return Err(BuildError::UnknownMiddleware {
                pattern: mount.pattern.clone(),
                name: mount.middleware.clone(),
                suggestion: nearest_name(&mount.middleware, &offered),
                offered,
            });
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// The setup block
// ---------------------------------------------------------------------------

/// What the application's own setup block can reach.
pub struct Setup<'a> {
    config: &'a Config,
    services: &'a mut Services,
    router: &'a mut RouterPlan,
}

impl Setup<'_> {
    pub fn config(&self) -> &Config {
        self.config
    }

    pub fn services(&self) -> &Services {
        self.services
    }

    pub fn services_mut(&mut self) -> &mut Services {
        self.services
    }

    /// The router plan. Mounting middleware happens here, with a pattern,
    /// because the pattern is the application's decision and never the
    /// plugin's.
    pub fn router(&mut self) -> &mut RouterPlan {
        self.router
    }
}

type SetupBlock = Box<dyn FnOnce(&mut Setup<'_>) -> Result<(), PluginError>>;

// ---------------------------------------------------------------------------
// Registered instances
// ---------------------------------------------------------------------------

struct Registered {
    inner: Box<dyn AnyInstance>,
    /// First instance of its plugin type.
    primary: bool,
    /// This registration widened the provider list, so this instance is the
    /// one that fills the context.
    provides_context: bool,
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Builds an [`App`]. `E` is the type-level list of installed context
/// providers; it grows as provider plugins are registered.
pub struct AppBuilder<E = Base> {
    config: Config,
    instances: Vec<Registered>,
    services: Services,
    setup: Option<SetupBlock>,
    _providers: PhantomData<fn() -> E>,
}

impl<E> std::fmt::Debug for AppBuilder<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let plugins: Vec<&str> = self.instances.iter().map(|r| r.inner.name()).collect();
        f.debug_struct("AppBuilder")
            .field("app", &self.config.app().as_str())
            .field("env", &self.config.env())
            .field("plugins", &plugins)
            .field("has_setup_block", &self.setup.is_some())
            .finish()
    }
}

impl AppBuilder<Base> {
    /// Starts an application from a loaded configuration.
    pub fn new(config: Config) -> Self {
        Self {
            config,
            instances: Vec::new(),
            services: Services::new(),
            setup: None,
            _providers: PhantomData,
        }
    }
}

impl<E> AppBuilder<E> {
    /// Registers a plugin under its default name, `P::NAME`, which is also the
    /// configuration section it reads.
    ///
    /// This is the registration that widens the context provider list, so a
    /// handler that asks for what the plugin provides compiles only after this
    /// call exists. Register the same provider plugin twice this way and the
    /// list holds its item twice, which makes extraction ambiguous; use
    /// [`AppBuilder::plugin_as`] for extra instances.
    pub fn plugin<P: Plugin>(self, plugin: P) -> AppBuilder<<P::Provides as Provided>::Install<E>> {
        self.add(P::NAME.to_string(), plugin, true).retype()
    }

    /// Registers another instance of a plugin under `name`, which is also the
    /// configuration section it reads.
    ///
    /// So `[logger]` and `[db_log]` can both be the logging plugin at
    /// different levels. The provider list is not widened: a context item is
    /// one value, and two instances would only fight over it.
    pub fn plugin_as<P: Plugin>(self, name: impl Into<String>, plugin: P) -> AppBuilder<E> {
        self.add(name.into(), plugin, false)
    }

    /// The application's own setup block, run after `after_config` and before
    /// `configure_router`.
    pub fn setup<F>(mut self, f: F) -> Self
    where
        F: FnOnce(&mut Setup<'_>) -> Result<(), PluginError> + 'static,
    {
        self.setup = Some(Box::new(f));
        self
    }

    fn add<P: Plugin>(mut self, name: String, plugin: P, provides_context: bool) -> Self {
        let primary = !self
            .instances
            .iter()
            .any(|r| r.inner.plugin_type() == TypeId::of::<P>());
        self.instances.push(Registered {
            inner: Box::new(Instance {
                name,
                plugin,
                config: P::Config::default(),
            }),
            primary,
            provides_context,
        });
        self
    }

    fn retype<F>(self) -> AppBuilder<F> {
        AppBuilder {
            config: self.config,
            instances: self.instances,
            services: self.services,
            setup: self.setup,
            _providers: PhantomData,
        }
    }

    /// Runs the lifecycle and returns the finished application.
    pub fn build(mut self) -> Result<App<E>, BuildError> {
        // Names are section names, so they must be unique.
        let names: Vec<String> = self.instances.iter().map(|r| r.inner.name().to_string()).collect();
        for (i, name) in names.iter().enumerate() {
            if names[..i].contains(name) {
                return Err(BuildError::DuplicateInstance { name: name.clone() });
            }
        }

        // A section nobody claimed is a typo, and a typo must never build
        // clean. This is the check that Hugo does not make.
        self.config.validate_sections(&names)?;

        for reg in &mut self.instances {
            reg.inner.load_config(&self.config)?;
        }

        let mut router = RouterPlan::default();

        run_stage(
            Stage::Configure,
            &mut self.instances,
            &self.config,
            &mut self.services,
            |inst, lc| inst.configure(lc),
        )?;
        run_stage(
            Stage::Register,
            &mut self.instances,
            &self.config,
            &mut self.services,
            |inst, lc| inst.register(lc),
        )?;
        run_stage(
            Stage::AfterConfig,
            &mut self.instances,
            &self.config,
            &mut self.services,
            |inst, lc| inst.after_config(lc),
        )?;

        if let Some(setup) = self.setup.take() {
            let mut cx = Setup {
                config: &self.config,
                services: &mut self.services,
                router: &mut router,
            };
            setup(&mut cx).map_err(BuildError::Setup)?;
        }

        for reg in &mut self.instances {
            let name = reg.inner.name().to_string();
            let mut catalog = MiddlewareCatalog::new(&mut router.offers, &name);
            reg.inner
                .configure_router(&mut catalog)
                .map_err(|source| BuildError::Hook {
                    instance: name,
                    stage: Stage::ConfigureRouter,
                    source,
                })?;
        }

        router.resolve()?;

        run_stage(
            Stage::BeforeStart,
            &mut self.instances,
            &self.config,
            &mut self.services,
            |inst, lc| inst.before_start(lc),
        )?;

        Ok(App {
            config: self.config,
            instances: self.instances,
            services: self.services,
            router,
            _providers: PhantomData,
        })
    }
}

fn run_stage<F>(
    stage: Stage,
    instances: &mut [Registered],
    config: &Config,
    services: &mut Services,
    mut hook: F,
) -> Result<(), BuildError>
where
    F: FnMut(&mut dyn AnyInstance, &mut Lifecycle<'_>) -> Result<(), PluginError>,
{
    for reg in instances.iter_mut() {
        let name = reg.inner.name().to_string();
        let mut lc = Lifecycle::new(stage, &name, reg.primary, config, services);
        hook(reg.inner.as_mut(), &mut lc).map_err(|source| BuildError::Hook {
            instance: name,
            stage,
            source,
        })?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

/// A built application.
///
/// `E` is the list of installed context providers, carried in the type so that
/// [`App::context`] produces a [`Context`] a handler can extract from without
/// a run-time check.
pub struct App<E = Base> {
    config: Config,
    instances: Vec<Registered>,
    services: Services,
    router: RouterPlan,
    _providers: PhantomData<fn() -> E>,
}

impl<E> std::fmt::Debug for App<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let plugins: Vec<String> = self
            .instances
            .iter()
            .map(|r| format!("{} ({})", r.inner.name(), r.inner.plugin_type_name()))
            .collect();
        f.debug_struct("App")
            .field("app", &self.config.app().as_str())
            .field("env", &self.config.env())
            .field("plugins", &plugins)
            .field("offers", &self.router.offers.len())
            .field("mounts", &self.router.mounts.len())
            .finish()
    }
}

impl<E> App<E> {
    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn services(&self) -> &Services {
        &self.services
    }

    pub fn services_mut(&mut self) -> &mut Services {
        &mut self.services
    }

    pub fn router_plan(&self) -> &RouterPlan {
        &self.router
    }

    /// Every instance name, in registration order.
    pub fn instance_names(&self) -> impl Iterator<Item = &str> {
        self.instances.iter().map(|r| r.inner.name())
    }

    /// The names of every instance of `P`, in registration order. The first is
    /// the primary.
    pub fn instances_of<P: Plugin>(&self) -> Vec<&str> {
        self.instances
            .iter()
            .filter(|r| r.inner.plugin_type() == TypeId::of::<P>())
            .map(|r| r.inner.name())
            .collect()
    }

    /// The primary instance of `P`: the first one registered.
    pub fn plugin<P: Plugin>(&self) -> Option<&P> {
        self.find::<P>(None).and_then(|r| r.inner.plugin_any().downcast_ref::<P>())
    }

    pub fn plugin_mut<P: Plugin>(&mut self) -> Option<&mut P> {
        let idx = self.index_of::<P>(None)?;
        self.instances[idx].inner.plugin_any_mut().downcast_mut::<P>()
    }

    /// One named instance of `P`.
    pub fn plugin_named<P: Plugin>(&self, name: &str) -> Option<&P> {
        self.find::<P>(Some(name))
            .and_then(|r| r.inner.plugin_any().downcast_ref::<P>())
    }

    /// The typed configuration the primary instance of `P` was built from.
    pub fn plugin_config<P: Plugin>(&self) -> Option<&P::Config> {
        self.find::<P>(None)
            .and_then(|r| r.inner.config_any().downcast_ref::<P::Config>())
    }

    /// The typed configuration of one named instance of `P`.
    pub fn plugin_config_named<P: Plugin>(&self, name: &str) -> Option<&P::Config> {
        self.find::<P>(Some(name))
            .and_then(|r| r.inner.config_any().downcast_ref::<P::Config>())
    }

    /// Whether `name` is the primary instance of its plugin type.
    pub fn is_primary(&self, name: &str) -> bool {
        self.instances
            .iter()
            .find(|r| r.inner.name() == name)
            .is_some_and(|r| r.primary)
    }

    /// A fresh per-request context, filled by the installed providers.
    pub fn context(&self) -> Context<E> {
        let mut builder = ContextBuilder::new();
        for reg in self.instances.iter().filter(|r| r.provides_context) {
            reg.inner.provide(&mut builder);
        }
        builder.build::<E>()
    }

    fn find<P: Plugin>(&self, name: Option<&str>) -> Option<&Registered> {
        self.index_of::<P>(name).map(|i| &self.instances[i])
    }

    fn index_of<P: Plugin>(&self, name: Option<&str>) -> Option<usize> {
        self.instances.iter().position(|r| {
            r.inner.plugin_type() == TypeId::of::<P>()
                && name.is_none_or(|n| r.inner.name() == n)
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_context::{ContextItem, NoContext, ProvidesContext, With};
    use serde::Deserialize;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    // -- a recorder, owned by the test, never global ------------------------

    #[derive(Clone, Default)]
    struct Recorder(Arc<Mutex<Vec<String>>>);

    impl Recorder {
        fn push(&self, line: impl Into<String>) {
            self.0.lock().unwrap().push(line.into());
        }
        fn take(&self) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }
    }

    // -- a plugin with a typed section --------------------------------------

    #[derive(Clone, Debug, Deserialize, PartialEq)]
    #[serde(default, deny_unknown_fields)]
    struct LoggerConfig {
        level: String,
        json: bool,
    }

    impl Default for LoggerConfig {
        fn default() -> Self {
            Self {
                level: "info".into(),
                json: false,
            }
        }
    }

    struct LoggerPlugin {
        rec: Recorder,
        level: String,
        services_seen: usize,
    }

    impl LoggerPlugin {
        fn new(rec: &Recorder) -> Self {
            Self {
                rec: rec.clone(),
                level: String::new(),
                services_seen: 0,
            }
        }
    }

    struct LogSink(String);

    impl Plugin for LoggerPlugin {
        type Config = LoggerConfig;
        type Provides = NoContext;
        const NAME: &'static str = "logger";

        fn configure(
            &mut self,
            config: &Self::Config,
            lc: &mut Lifecycle<'_>,
        ) -> Result<(), PluginError> {
            self.level = config.level.clone();
            self.rec.push(format!("{}:{}", lc.instance(), lc.stage()));
            Ok(())
        }

        fn register(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
            if lc.is_primary() {
                lc.services_mut().insert(LogSink(self.level.clone()));
            }
            self.rec.push(format!("{}:{}", lc.instance(), lc.stage()));
            Ok(())
        }

        fn after_config(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
            self.services_seen = lc.services().len();
            self.rec.push(format!("{}:{}", lc.instance(), lc.stage()));
            Ok(())
        }

        fn configure_router(
            &mut self,
            catalog: &mut MiddlewareCatalog<'_>,
        ) -> Result<(), PluginError> {
            // Offered, not mounted. The pattern is the application's call.
            catalog.offer(
                format!("{}.request_log", catalog.instance()),
                "logs one line per request",
            );
            self.rec
                .push(format!("{}:{}", catalog.instance(), Stage::ConfigureRouter));
            Ok(())
        }

        fn before_start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
            self.rec.push(format!("{}:{}", lc.instance(), lc.stage()));
            Ok(())
        }
    }

    // -- a plugin that extends the context ----------------------------------

    #[derive(Clone, Debug, Deserialize, PartialEq)]
    #[serde(default)]
    struct RequestIdConfig {
        prefix: String,
    }

    impl Default for RequestIdConfig {
        fn default() -> Self {
            Self {
                prefix: "req".into(),
            }
        }
    }

    #[derive(Debug, PartialEq)]
    struct RequestId(String);
    impl ContextItem for RequestId {}

    #[derive(Default)]
    struct RequestIdPlugin {
        prefix: String,
        counter: AtomicU64,
    }

    impl Plugin for RequestIdPlugin {
        type Config = RequestIdConfig;
        type Provides = ProvidesContext<RequestId>;
        const NAME: &'static str = "request_id";

        fn configure(
            &mut self,
            config: &Self::Config,
            _lc: &mut Lifecycle<'_>,
        ) -> Result<(), PluginError> {
            self.prefix = config.prefix.clone();
            Ok(())
        }

        fn provide(&self, cx: &mut ContextBuilder) {
            let n = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
            cx.insert(RequestId(format!("{}-{}", self.prefix, n)));
        }
    }

    struct FailingPlugin;

    impl Plugin for FailingPlugin {
        type Config = ();
        type Provides = NoContext;
        const NAME: &'static str = "failing";

        fn after_config(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
            Err(PluginError::new("no database reachable"))
        }
    }

    fn config(text: &str) -> Config {
        Config::from_toml("my_app", "test", text).unwrap()
    }

    // -- lifecycle order ----------------------------------------------------

    #[test]
    fn lifecycle_hooks_run_in_the_recorded_order() {
        let rec = Recorder::default();
        let block_rec = rec.clone();

        let app = AppBuilder::new(config(
            r#"
            [logger]
            level = "warn"
            [db_log]
            level = "debug"
            "#,
        ))
        .plugin(LoggerPlugin::new(&rec))
        .plugin_as("db_log", LoggerPlugin::new(&rec))
        .setup(move |setup| {
            block_rec.push(format!("app:{}", Stage::UserBlock));
            setup.router().mount("/admin", "logger.request_log");
            Ok(())
        })
        .build()
        .unwrap();

        // The sequence is the assertion, not an inspection of final state.
        assert_eq!(
            rec.take(),
            vec![
                "logger:configure",
                "db_log:configure",
                "logger:register",
                "db_log:register",
                "logger:after_config",
                "db_log:after_config",
                "app:user_block",
                "logger:configure_router",
                "db_log:configure_router",
                "logger:before_start",
                "db_log:before_start",
            ]
        );

        assert_eq!(app.router_plan().mounts().len(), 1);
    }

    #[test]
    fn a_hook_that_fails_names_the_instance_and_the_stage() {
        let err = AppBuilder::new(config(""))
            .plugin(FailingPlugin)
            .build()
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("`failing`"), "{msg}");
        assert!(msg.contains("`after_config`"), "{msg}");
        assert!(msg.contains("no database reachable"), "{msg}");
    }

    // -- typed sections through the plugin ----------------------------------

    #[test]
    fn each_instance_reads_the_section_named_after_it() {
        let rec = Recorder::default();
        let app = AppBuilder::new(config(
            r#"
            [logger]
            level = "warn"
            [db_log]
            level = "debug"
            json = true
            "#,
        ))
        .plugin(LoggerPlugin::new(&rec))
        .plugin_as("db_log", LoggerPlugin::new(&rec))
        .build()
        .unwrap();

        assert_eq!(app.plugin_config::<LoggerPlugin>().unwrap().level, "warn");
        assert_eq!(
            app.plugin_config_named::<LoggerPlugin>("db_log").unwrap(),
            &LoggerConfig {
                level: "debug".into(),
                json: true
            }
        );
    }

    #[test]
    fn an_unknown_key_inside_a_plugin_section_is_that_plugins_business() {
        // LoggerConfig opted into deny_unknown_fields, so it rejects. The core
        // never looked.
        let rec = Recorder::default();
        let err = AppBuilder::new(config("[logger]\nlevel = \"warn\"\nverbose = true\n"))
            .plugin(LoggerPlugin::new(&rec))
            .build()
            .unwrap_err();
        assert!(matches!(err, BuildError::Config(ConfigError::Section { .. })), "{err:?}");
    }

    #[test]
    fn an_unclaimed_top_level_section_stops_the_build() {
        let rec = Recorder::default();
        let err = AppBuilder::new(config("[loggr]\nlevel = \"warn\"\n"))
            .plugin(LoggerPlugin::new(&rec))
            .build()
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("`[loggr]`"), "{msg}");
        assert!(msg.contains("did you mean `[logger]`?"), "{msg}");
    }

    // -- multiple instances -------------------------------------------------

    #[test]
    fn two_instances_of_one_plugin_coexist_and_the_first_is_primary() {
        let rec = Recorder::default();
        let app = AppBuilder::new(config(
            r#"
            [logger]
            level = "warn"
            [db_log]
            level = "debug"
            "#,
        ))
        .plugin(LoggerPlugin::new(&rec))
        .plugin_as("db_log", LoggerPlugin::new(&rec))
        .build()
        .unwrap();

        assert_eq!(app.instances_of::<LoggerPlugin>(), vec!["logger", "db_log"]);
        assert!(app.is_primary("logger"));
        assert!(!app.is_primary("db_log"));

        // The bare lookup is the primary.
        assert_eq!(app.plugin::<LoggerPlugin>().unwrap().level, "warn");
        assert_eq!(
            app.plugin_named::<LoggerPlugin>("db_log").unwrap().level,
            "debug"
        );

        // Only the primary published the shared service.
        assert_eq!(app.services().get::<LogSink>().unwrap().0, "warn");

        // Both are real instances with their own state.
        assert_eq!(app.router_plan().offers().len(), 2);
        assert!(app.router_plan().offer_named("db_log.request_log").is_some());
    }

    #[test]
    fn registering_two_instances_under_one_name_is_an_error() {
        let rec = Recorder::default();
        let err = AppBuilder::new(config("[logger]\nlevel = \"warn\"\n"))
            .plugin(LoggerPlugin::new(&rec))
            .plugin_as("logger", LoggerPlugin::new(&rec))
            .build()
            .unwrap_err();
        assert!(matches!(err, BuildError::DuplicateInstance { .. }), "{err:?}");
    }

    // -- middleware is offered, never autowired -----------------------------

    #[test]
    fn a_plugin_offers_middleware_and_only_the_application_mounts_it() {
        let rec = Recorder::default();
        let app = AppBuilder::new(config("[logger]\nlevel = \"warn\"\n"))
            .plugin(LoggerPlugin::new(&rec))
            .setup(|setup| {
                setup.router().mount("/admin/*rest", "logger.request_log");
                Ok(())
            })
            .build()
            .unwrap();

        // The plugin offered; it did not mount. Without the setup block there
        // would be no mount at all.
        assert_eq!(app.router_plan().offers()[0].offered_by, "logger");
        assert_eq!(
            app.router_plan().mounts(),
            &[Mount {
                pattern: "/admin/*rest".into(),
                middleware: "logger.request_log".into(),
            }]
        );
    }

    #[test]
    fn mounting_middleware_nobody_offers_is_an_error() {
        let rec = Recorder::default();
        let err = AppBuilder::new(config("[logger]\nlevel = \"warn\"\n"))
            .plugin(LoggerPlugin::new(&rec))
            .setup(|setup| {
                setup.router().mount("/admin", "logger.request_logs");
                Ok(())
            })
            .build()
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("logger.request_logs"), "{msg}");
        assert!(msg.contains("did you mean `logger.request_log`?"), "{msg}");
    }

    // -- context extension --------------------------------------------------

    #[test]
    fn a_handler_extracts_what_a_registered_plugin_provides() {
        let app = AppBuilder::new(config("[request_id]\nprefix = \"web\"\n"))
            .plugin(RequestIdPlugin::default())
            .build()
            .unwrap();

        // The provider list is in the type of the application, so this
        // signature is checked at compile time.
        fn handler(cx: &Context<With<RequestId, Base>>) -> String {
            let id: &RequestId = cx.extract();
            id.0.clone()
        }

        assert_eq!(handler(&app.context()), "web-1");
        assert_eq!(handler(&app.context()), "web-2");
    }

    // -- the specification: no shared state ---------------------------------

    #[test]
    fn two_applications_in_one_process_do_not_interfere() {
        // The Crystal original stored plugin instances in class variables, so
        // this test could not pass and its own suite needed `reset!` between
        // cases. Nothing below is reset, and nothing below is shared.
        let rec_a = Recorder::default();
        let rec_b = Recorder::default();

        let mut app_a = AppBuilder::new(
            Config::from_toml("my_app", "production", "[logger]\nlevel = \"warn\"\n").unwrap(),
        )
        .plugin(LoggerPlugin::new(&rec_a))
        .plugin(RequestIdPlugin::default())
        .build()
        .unwrap();

        let app_b = AppBuilder::new(
            Config::from_toml(
                "other_app",
                "development",
                "[logger]\nlevel = \"debug\"\n[db_log]\nlevel = \"trace\"\n[request_id]\nprefix = \"b\"\n",
            )
            .unwrap(),
        )
        .plugin(LoggerPlugin::new(&rec_b))
        .plugin_as("db_log", LoggerPlugin::new(&rec_b))
        .plugin(RequestIdPlugin::default())
        .build()
        .unwrap();

        // 1. Configuration does not leak.
        assert_eq!(app_a.config().env(), "production");
        assert_eq!(app_b.config().env(), "development");
        assert_eq!(app_a.config().app().as_str(), "my_app");
        assert_eq!(app_b.config().app().as_str(), "other_app");

        // 2. Plugin instance state does not leak.
        assert_eq!(app_a.plugin::<LoggerPlugin>().unwrap().level, "warn");
        assert_eq!(app_b.plugin::<LoggerPlugin>().unwrap().level, "debug");

        // 3. The instance lists differ; a section claimed by B is unknown to
        //    A, which is why A would have rejected it.
        assert_eq!(app_a.instances_of::<LoggerPlugin>(), vec!["logger"]);
        assert_eq!(app_b.instances_of::<LoggerPlugin>(), vec!["logger", "db_log"]);

        // 4. Services are per application.
        assert_eq!(app_a.services().get::<LogSink>().unwrap().0, "warn");
        assert_eq!(app_b.services().get::<LogSink>().unwrap().0, "debug");
        app_a.services_mut().insert(LogSink("mutated".into()));
        assert_eq!(app_a.services().get::<LogSink>().unwrap().0, "mutated");
        assert_eq!(app_b.services().get::<LogSink>().unwrap().0, "debug");

        // 5. Router plans are per application.
        assert_eq!(app_a.router_plan().offers().len(), 1);
        assert_eq!(app_b.router_plan().offers().len(), 2);

        // 6. Per-request state is per application: B's traffic never advances
        //    A's counter.
        for _ in 0..5 {
            let _ = app_b.context();
        }
        assert_eq!(app_a.context().get::<RequestId, _>().0, "req-1");
        assert_eq!(app_b.context().get::<RequestId, _>().0, "b-6");

        // 7. Even the lifecycle recorders saw only their own application.
        assert!(rec_a.take().iter().all(|l| l.starts_with("logger:")));
        assert!(rec_b.take().iter().any(|l| l.starts_with("db_log:")));
        assert!(!rec_a.take().iter().any(|l| l.starts_with("db_log:")));
    }
}
