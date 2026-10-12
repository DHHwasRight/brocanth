//! The application: an explicit value that owns its plugin instances, its
//! services and its router plan.
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
//! 1. `configure` — every instance settles its own state
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

use crate::plugin_api::{
    AnyInstance, Instance, Lifecycle, MiddlewareCatalog, MiddlewareOffer, Plugin, PluginError,
    Services, Stage,
};
use crate::plugin_context::{Base, Context, ContextBuilder, Provided};
use crate::suggest::nearest_name;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum BuildError {
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
         a name is how the application and every other plugin refer to one instance"
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
    services: &'a mut Services,
    router: &'a mut RouterPlan,
}

impl Setup<'_> {
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
    instances: Vec<Registered>,
    services: Services,
    setup: Option<SetupBlock>,
    _providers: PhantomData<fn() -> E>,
}

impl<E> std::fmt::Debug for AppBuilder<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let plugins: Vec<&str> = self.instances.iter().map(|r| r.inner.name()).collect();
        f.debug_struct("AppBuilder")
            .field("plugins", &plugins)
            .field("has_setup_block", &self.setup.is_some())
            .finish()
    }
}

impl AppBuilder<Base> {
    /// Starts an application with no plugins installed.
    ///
    /// There is no configuration argument, and that is the point: the core
    /// takes already-constructed values, so an application that configures
    /// itself from its environment links no parser. `brocanth-config` adds
    /// `.plugin_from_config::<P>("section")` for applications that want files.
    pub fn new() -> Self {
        Self {
            instances: Vec::new(),
            services: Services::new(),
            setup: None,
            _providers: PhantomData,
        }
    }
}

impl Default for AppBuilder<Base> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E> AppBuilder<E> {
    /// Registers a plugin under its default name, `P::NAME`.
    ///
    /// The plugin arrives built, which means it arrives configured:
    /// `app.plugin(LoggerPlugin::new(LoggerConfig { level: Level::Warn }))`.
    ///
    /// This is the registration that widens the context provider list, so a
    /// handler that asks for what the plugin provides compiles only after this
    /// call exists. Register the same provider plugin twice this way and the
    /// list holds its item twice, which makes extraction ambiguous; use
    /// [`AppBuilder::plugin_as`] for extra instances.
    pub fn plugin<P: Plugin>(self, plugin: P) -> AppBuilder<<P::Provides as Provided>::Install<E>> {
        self.add(P::NAME.to_string(), plugin, true).retype()
    }

    /// Registers a plugin under a name of the application's choosing, widening
    /// the context provider list exactly as [`AppBuilder::plugin`] does.
    ///
    /// For the first instance of a plugin that should not carry its default
    /// name. Extra instances go through [`AppBuilder::plugin_as`], for the
    /// reason given there.
    pub fn plugin_named<P: Plugin>(
        self,
        name: impl Into<String>,
        plugin: P,
    ) -> AppBuilder<<P::Provides as Provided>::Install<E>> {
        self.add(name.into(), plugin, true).retype()
    }

    /// Registers another instance of a plugin under `name`.
    ///
    /// So `logger` and `db_log` can both be the logging plugin at different
    /// levels. The provider list is not widened: a context item is one value,
    /// and two instances would only fight over it.
    pub fn plugin_as<P: Plugin>(self, name: impl Into<String>, plugin: P) -> AppBuilder<E> {
        self.add(name.into(), plugin, false)
    }

    /// The services the plugins will share, before any of them runs.
    ///
    /// An application seeds a value here that plugins read in their hooks, and
    /// a crate extending the builder stores its own state here rather than in
    /// the core. `brocanth-config` keeps the loaded configuration in it.
    pub fn services(&self) -> &Services {
        &self.services
    }

    pub fn services_mut(&mut self) -> &mut Services {
        &mut self.services
    }

    /// Every instance name registered so far, in registration order.
    pub fn instance_names(&self) -> impl Iterator<Item = &str> {
        self.instances.iter().map(|r| r.inner.name())
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
            inner: Box::new(Instance { name, plugin }),
            primary,
            provides_context,
        });
        self
    }

    fn retype<F>(self) -> AppBuilder<F> {
        AppBuilder {
            instances: self.instances,
            services: self.services,
            setup: self.setup,
            _providers: PhantomData,
        }
    }

    /// Runs the lifecycle and returns the finished application.
    pub fn build(mut self) -> Result<App<E>, BuildError> {
        // A name identifies one instance, so it must identify only one.
        let names: Vec<String> = self
            .instances
            .iter()
            .map(|r| r.inner.name().to_string())
            .collect();
        for (i, name) in names.iter().enumerate() {
            if names[..i].contains(name) {
                return Err(BuildError::DuplicateInstance { name: name.clone() });
            }
        }

        let mut router = RouterPlan::default();

        run_stage(
            Stage::Configure,
            &mut self.instances,
            &names,
            &mut self.services,
            |inst, lc| inst.configure(lc),
        )?;
        run_stage(
            Stage::Register,
            &mut self.instances,
            &names,
            &mut self.services,
            |inst, lc| inst.register(lc),
        )?;
        run_stage(
            Stage::AfterConfig,
            &mut self.instances,
            &names,
            &mut self.services,
            |inst, lc| inst.after_config(lc),
        )?;

        if let Some(setup) = self.setup.take() {
            let mut cx = Setup {
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
            &names,
            &mut self.services,
            |inst, lc| inst.before_start(lc),
        )?;

        Ok(App {
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
    names: &[String],
    services: &mut Services,
    mut hook: F,
) -> Result<(), BuildError>
where
    F: FnMut(&mut dyn AnyInstance, &mut Lifecycle<'_>) -> Result<(), PluginError>,
{
    for reg in instances.iter_mut() {
        let name = reg.inner.name().to_string();
        let mut lc = Lifecycle::new(stage, &name, reg.primary, names, services);
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
            .field("plugins", &plugins)
            .field("offers", &self.router.offers.len())
            .field("mounts", &self.router.mounts.len())
            .finish()
    }
}

impl<E> App<E> {
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
        self.find::<P>(None)
            .and_then(|r| r.inner.plugin_any().downcast_ref::<P>())
    }

    pub fn plugin_mut<P: Plugin>(&mut self) -> Option<&mut P> {
        let idx = self.index_of::<P>(None)?;
        self.instances[idx]
            .inner
            .plugin_any_mut()
            .downcast_mut::<P>()
    }

    /// One named instance of `P`.
    pub fn plugin_named<P: Plugin>(&self, name: &str) -> Option<&P> {
        self.find::<P>(Some(name))
            .and_then(|r| r.inner.plugin_any().downcast_ref::<P>())
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
            r.inner.plugin_type() == TypeId::of::<P>() && name.is_none_or(|n| r.inner.name() == n)
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

    // -- a plugin with a typed configuration --------------------------------
    //
    // `Plugin::Config` carries no bounds, so the value the application builds
    // can hold anything — here a level, a flag, and the test's own recorder.
    // Nothing in the core looks inside it.

    #[derive(Clone)]
    struct LoggerConfig {
        level: String,
        json: bool,
        rec: Recorder,
    }

    impl LoggerConfig {
        fn new(level: &str, rec: &Recorder) -> Self {
            Self {
                level: level.into(),
                json: false,
                rec: rec.clone(),
            }
        }
    }

    struct LoggerPlugin {
        rec: Recorder,
        level: String,
        json: bool,
        services_seen: usize,
    }

    struct LogSink(String);

    impl Plugin for LoggerPlugin {
        type Config = LoggerConfig;
        type Provides = NoContext;
        const NAME: &'static str = "logger";

        fn new(config: Self::Config) -> Self {
            Self {
                rec: config.rec,
                level: config.level,
                json: config.json,
                services_seen: 0,
            }
        }

        fn configure(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
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

    fn logger(level: &str, rec: &Recorder) -> LoggerPlugin {
        LoggerPlugin::new(LoggerConfig::new(level, rec))
    }

    // -- a plugin that extends the context ----------------------------------

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

    struct RequestIdPlugin {
        prefix: String,
        counter: AtomicU64,
    }

    impl Default for RequestIdPlugin {
        fn default() -> Self {
            Self::new(RequestIdConfig::default())
        }
    }

    impl Plugin for RequestIdPlugin {
        type Config = RequestIdConfig;
        type Provides = ProvidesContext<RequestId>;
        const NAME: &'static str = "request_id";

        fn new(config: Self::Config) -> Self {
            Self {
                prefix: config.prefix,
                counter: AtomicU64::new(0),
            }
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

        fn new(_config: ()) -> Self {
            Self
        }

        fn after_config(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
            Err(PluginError::new("no database reachable"))
        }
    }

    // -- lifecycle order ----------------------------------------------------

    #[test]
    fn lifecycle_hooks_run_in_the_recorded_order() {
        let rec = Recorder::default();
        let block_rec = rec.clone();

        let app = AppBuilder::new()
            .plugin(logger("warn", &rec))
            .plugin_as("db_log", logger("debug", &rec))
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
        let err = AppBuilder::new()
            .plugin(FailingPlugin::new(()))
            .build()
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("`failing`"), "{msg}");
        assert!(msg.contains("`after_config`"), "{msg}");
        assert!(msg.contains("no database reachable"), "{msg}");
    }

    // -- a plugin arrives already configured --------------------------------

    #[test]
    fn each_instance_keeps_the_configuration_it_was_built_with() {
        // No file, no parser, no section lookup: the application built two
        // configuration values and handed one to each instance.
        let rec = Recorder::default();
        let app = AppBuilder::new()
            .plugin(logger("warn", &rec))
            .plugin_as(
                "db_log",
                LoggerPlugin::new(LoggerConfig {
                    level: "debug".into(),
                    json: true,
                    rec: rec.clone(),
                }),
            )
            .build()
            .unwrap();

        let primary = app.plugin::<LoggerPlugin>().unwrap();
        assert_eq!(primary.level, "warn");
        assert!(!primary.json);

        let second = app.plugin_named::<LoggerPlugin>("db_log").unwrap();
        assert_eq!(second.level, "debug");
        assert!(second.json);
    }

    #[test]
    fn a_plugin_can_be_registered_under_a_name_of_the_applications_choosing() {
        let app = AppBuilder::new()
            .plugin_named(
                "primary_logger",
                LoggerPlugin::new(LoggerConfig::new("warn", &Recorder::default())),
            )
            .build()
            .unwrap();

        assert_eq!(app.instances_of::<LoggerPlugin>(), vec!["primary_logger"]);
        assert!(app.is_primary("primary_logger"));
    }

    #[test]
    fn a_plugin_can_read_the_roster_of_registered_instances() {
        // The seam `brocanth-config` uses to reject a section that no instance
        // claims. A plugin sees the names, never the other plugins.
        struct Roster(Arc<Mutex<Vec<String>>>);

        impl Plugin for Roster {
            type Config = Arc<Mutex<Vec<String>>>;
            type Provides = NoContext;
            const NAME: &'static str = "roster";

            fn new(config: Self::Config) -> Self {
                Self(config)
            }

            fn before_start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
                *self.0.lock().unwrap() = lc.instance_names().to_vec();
                Ok(())
            }
        }

        let seen = Arc::new(Mutex::new(Vec::new()));
        let rec = Recorder::default();
        AppBuilder::new()
            .plugin(Roster::new(seen.clone()))
            .plugin(logger("warn", &rec))
            .plugin_as("db_log", logger("debug", &rec))
            .build()
            .unwrap();

        assert_eq!(*seen.lock().unwrap(), vec!["roster", "logger", "db_log"]);
    }

    // -- multiple instances -------------------------------------------------

    #[test]
    fn two_instances_of_one_plugin_coexist_and_the_first_is_primary() {
        let rec = Recorder::default();
        let app = AppBuilder::new()
            .plugin(logger("warn", &rec))
            .plugin_as("db_log", logger("debug", &rec))
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
        assert!(app
            .router_plan()
            .offer_named("db_log.request_log")
            .is_some());
    }

    #[test]
    fn registering_two_instances_under_one_name_is_an_error() {
        let rec = Recorder::default();
        let err = AppBuilder::new()
            .plugin(logger("warn", &rec))
            .plugin_as("logger", logger("warn", &rec))
            .build()
            .unwrap_err();
        assert!(
            matches!(err, BuildError::DuplicateInstance { .. }),
            "{err:?}"
        );
    }

    // -- middleware is offered, never autowired -----------------------------

    #[test]
    fn a_plugin_offers_middleware_and_only_the_application_mounts_it() {
        let rec = Recorder::default();
        let app = AppBuilder::new()
            .plugin(logger("warn", &rec))
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
        let err = AppBuilder::new()
            .plugin(logger("warn", &rec))
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
        let app = AppBuilder::new()
            .plugin(RequestIdPlugin::new(RequestIdConfig {
                prefix: "web".into(),
            }))
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

        let mut app_a = AppBuilder::new()
            .plugin(logger("warn", &rec_a))
            .plugin(RequestIdPlugin::default())
            .build()
            .unwrap();

        let app_b = AppBuilder::new()
            .plugin(logger("debug", &rec_b))
            .plugin_as("db_log", logger("trace", &rec_b))
            .plugin(RequestIdPlugin::new(RequestIdConfig { prefix: "b".into() }))
            .build()
            .unwrap();

        // 1. Configuration does not leak: each instance kept the value its own
        //    application built.
        assert_eq!(app_a.plugin::<LoggerPlugin>().unwrap().level, "warn");
        assert_eq!(app_b.plugin::<LoggerPlugin>().unwrap().level, "debug");

        // 2. Plugin instance state does not leak.
        assert_eq!(app_a.plugin::<RequestIdPlugin>().unwrap().prefix, "req");
        assert_eq!(app_b.plugin::<RequestIdPlugin>().unwrap().prefix, "b");

        // 3. The instance lists differ.
        assert_eq!(app_a.instances_of::<LoggerPlugin>(), vec!["logger"]);
        assert_eq!(
            app_b.instances_of::<LoggerPlugin>(),
            vec!["logger", "db_log"]
        );

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
