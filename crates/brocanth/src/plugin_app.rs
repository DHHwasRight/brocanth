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
//! 6. hook installation — every instance registered with
//!    [`AppBuilder::plugin_hooking`] fills the request, response, error and
//!    model chains, which are then sorted into their declared order. The
//!    synchronous and the asynchronous request hooks share one chain and one
//!    sort, so the order between the two kinds is declared rather than
//!    implied by which trait a hook implements
//! 7. `before_start` — every instance
//! 8. `listen` — not here; the HTTP server consumes the finished [`App`]
//! 9. `start` — [`App::start`], once the socket is open
//! 10. `begin_shutdown`, then `shutdown` — [`App::shutdown`], both in reverse
//!     registration order
//!
//! Each stage completes for all instances before the next one starts.

use std::any::{Any, TypeId};
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::plugin_api::{
    AnyInstance, Instance, Lifecycle, MiddlewareCatalog, MiddlewareOffer, Plugin, PluginError,
    Services, Stage,
};
use crate::plugin_body::StreamGate;
use crate::plugin_context::{Base, Context, ContextBuilder, Provided};
use crate::plugin_health::Readiness;
use crate::plugin_hooks::{
    Begin, ErrorHook, HandlerError, HookFamilies, HookRegistrar, HookSource, NoHooks, Pipeline,
    Request, RequestState, RequestStep, Response, ResponseHook,
};
use crate::plugin_model::{ModelDispatch, ModelHooks};
use crate::plugin_order::{sort, OrderError, Ordered};
use crate::plugin_routes::{no_such_plugin, RouteBundle, RouteError, RoutePlan, RouteSource};
use crate::plugin_shutdown::{ShutdownCx, ShutdownPhase, ShutdownReason, ShutdownReport};
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

    #[error(
        "`{instance}` ({plugin}) installs hooks, and `plugin(..)` does not install them. \
         Register it with `plugin_hooking(..)`: that is the call that widens the \
         application's hook-family record, and the record is what decides whether the hot \
         path contains a hook at all"
    )]
    HooksNotInstalled { instance: String, plugin: String },

    #[error(
        "`{instance}` ({plugin}) was registered with `plugin_hooking(..)` but leaves \
         `const INSTALLS_HOOKS` at its default of `false`. Set it to `true` in its \
         `impl Plugin`, so that registering it with plain `plugin(..)` one day fails the \
         build instead of silently dropping its hooks"
    )]
    HooksNotDeclared { instance: String, plugin: String },

    #[error(
        "`{instance}` registered the {family} hook `{hook}`, and its `HookSource::Families` \
         does not name `{marker}`. The record is what decides whether that chain survives \
         compilation, so the hook would be sorted, stored, and never run. Write \
         `type Families = {marker};`, or `Both<{marker}, ..>` beside the families it \
         already names"
    )]
    FamilyNotDeclared {
        instance: String,
        hook: String,
        family: &'static str,
        marker: &'static str,
    },

    #[error("the hook order does not resolve: {0}")]
    Order(
        #[from]
        #[source]
        OrderError,
    ),
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
    readiness: &'a Readiness,
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

    /// The readiness list, for a check the application owns rather than a
    /// plugin: `setup.readiness().add("upstream", || async { .. })`.
    pub fn readiness(&self) -> &Readiness {
        self.readiness
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
    /// Set by `plugin_hooking`, which is the only call that knows the instance
    /// implements [`HookSource`]. A plain function pointer, so an instance
    /// that hooks nothing carries one null-sized `Option` and no closure.
    installer: Option<HookInstaller>,
}

/// Re-entry into a `HookSource` from a type-erased instance.
type HookInstaller = fn(&dyn Any, &mut HookRegistrar<'_>) -> Result<(), PluginError>;

fn install_hooks_of<P: HookSource>(
    plugin: &dyn Any,
    registrar: &mut HookRegistrar<'_>,
) -> Result<(), PluginError> {
    plugin
        .downcast_ref::<P>()
        .expect("the installer is stored beside the instance it was made for")
        .install_hooks(registrar)
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Builds an [`App`].
///
/// `E` is the type-level list of installed context providers; it grows as
/// provider plugins are registered. `H` is the type-level record of installed
/// hook families; it grows as hooking plugins are registered with
/// [`AppBuilder::plugin_hooking`], and it is what makes a hook nobody
/// registered cost nothing — see [`crate::plugin_hooks`].
pub struct AppBuilder<E = Base, H = NoHooks> {
    instances: Vec<Registered>,
    services: Services,
    readiness: Readiness,
    setup: Option<SetupBlock>,
    _providers: PhantomData<fn() -> E>,
    _families: PhantomData<fn() -> H>,
}

impl<E, H> std::fmt::Debug for AppBuilder<E, H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let plugins: Vec<&str> = self.instances.iter().map(|r| r.inner.name()).collect();
        f.debug_struct("AppBuilder")
            .field("plugins", &plugins)
            .field("has_setup_block", &self.setup.is_some())
            .finish()
    }
}

impl AppBuilder<Base, NoHooks> {
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
            readiness: Readiness::new(),
            setup: None,
            _providers: PhantomData,
            _families: PhantomData,
        }
    }
}

impl Default for AppBuilder<Base, NoHooks> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E, H> AppBuilder<E, H> {
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
    pub fn plugin<P: Plugin>(
        self,
        plugin: P,
    ) -> AppBuilder<<P::Provides as Provided>::Install<E>, H> {
        self.add(P::NAME.to_string(), plugin, true, None).retype()
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
    ) -> AppBuilder<<P::Provides as Provided>::Install<E>, H> {
        self.add(name.into(), plugin, true, None).retype()
    }

    /// Registers another instance of a plugin under `name`.
    ///
    /// So `logger` and `db_log` can both be the logging plugin at different
    /// levels. The provider list is not widened: a context item is one value,
    /// and two instances would only fight over it.
    pub fn plugin_as<P: Plugin>(self, name: impl Into<String>, plugin: P) -> AppBuilder<E, H> {
        self.add(name.into(), plugin, false, None)
    }

    /// Registers a plugin **and installs its hooks**.
    ///
    /// The one extra call the hook system costs, and the reason it costs it:
    /// `H` is the application's type-level record of which hook families are
    /// installed, and the hot path is generic over it. A request hook nobody
    /// installed is not an empty `Vec` that still has to be looked at — it is
    /// `H::REQUEST == false`, which is a constant, and the loop over it does
    /// not survive compilation. See ADR 1 in PLAN.md, and
    /// `tests/hooks_cost_nothing.rs` for the generated code.
    ///
    /// Everything [`AppBuilder::plugin`] does, this does too: the default
    /// name, the context provider list, the lifecycle.
    pub fn plugin_hooking<P: HookSource>(
        self,
        plugin: P,
    ) -> AppBuilder<<P::Provides as Provided>::Install<E>, crate::plugin_hooks::Both<P::Families, H>>
    {
        self.add(
            P::NAME.to_string(),
            plugin,
            true,
            Some(install_hooks_of::<P>),
        )
        .retype()
    }

    /// [`AppBuilder::plugin_hooking`], under a name of the application's
    /// choosing.
    pub fn plugin_hooking_named<P: HookSource>(
        self,
        name: impl Into<String>,
        plugin: P,
    ) -> AppBuilder<<P::Provides as Provided>::Install<E>, crate::plugin_hooks::Both<P::Families, H>>
    {
        self.add(name.into(), plugin, true, Some(install_hooks_of::<P>))
            .retype()
    }

    /// Another instance of a hooking plugin, which installs its hooks too.
    ///
    /// The provider list is not widened, for the reason given on
    /// [`AppBuilder::plugin_as`]; the hook chains are, because two instances
    /// of a logger really are two hooks, and their ids differ by the instance
    /// name so they can be ordered against each other.
    pub fn plugin_hooking_as<P: HookSource>(
        self,
        name: impl Into<String>,
        plugin: P,
    ) -> AppBuilder<E, crate::plugin_hooks::Both<P::Families, H>> {
        self.add(name.into(), plugin, false, Some(install_hooks_of::<P>))
            .retype()
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

    fn add<P: Plugin>(
        mut self,
        name: String,
        plugin: P,
        provides_context: bool,
        installer: Option<HookInstaller>,
    ) -> Self {
        let primary = !self
            .instances
            .iter()
            .any(|r| r.inner.plugin_type() == TypeId::of::<P>());
        self.instances.push(Registered {
            inner: Box::new(Instance { name, plugin }),
            primary,
            provides_context,
            installer,
        });
        self
    }

    fn retype<F, G>(self) -> AppBuilder<F, G> {
        AppBuilder {
            instances: self.instances,
            services: self.services,
            readiness: self.readiness,
            setup: self.setup,
            _providers: PhantomData,
            _families: PhantomData,
        }
    }

    /// Runs the lifecycle and returns the finished application.
    pub fn build(mut self) -> Result<App<E, H>, BuildError>
    where
        H: HookFamilies,
    {
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
            &self.readiness,
            |inst, lc| inst.configure(lc),
        )?;
        run_stage(
            Stage::Register,
            &mut self.instances,
            &names,
            &mut self.services,
            &self.readiness,
            |inst, lc| inst.register(lc),
        )?;
        run_stage(
            Stage::AfterConfig,
            &mut self.instances,
            &names,
            &mut self.services,
            &self.readiness,
            |inst, lc| inst.after_config(lc),
        )?;

        if let Some(setup) = self.setup.take() {
            let mut cx = Setup {
                services: &mut self.services,
                router: &mut router,
                readiness: &self.readiness,
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

        // Hooks, after every plugin is configured and the router plan is
        // settled, and before `before_start`, so a plugin's last word is
        // spoken with its hooks already in place.
        let (pipeline, model) = self.install_hooks()?;

        run_stage(
            Stage::BeforeStart,
            &mut self.instances,
            &names,
            &mut self.services,
            &self.readiness,
            |inst, lc| inst.before_start(lc),
        )?;

        Ok(App {
            instances: self.instances,
            services: self.services,
            readiness: self.readiness,
            router,
            pipeline,
            model: Arc::new(model),
            routes: RoutePlan::new(),
            streams: StreamGate::new(),
            _providers: PhantomData,
            _families: PhantomData,
        })
    }

    /// Collects every installed hook and sorts it into declared order.
    ///
    /// The sync and the async request hooks go into one `Vec` and through one
    /// [`sort`], so the order between the two kinds is the order somebody
    /// declared rather than a consequence of which list a hook landed in. See
    /// the header of [`crate::plugin_hooks`].
    fn install_hooks(&self) -> Result<(Pipeline, ModelHooks), BuildError>
    where
        H: HookFamilies,
    {
        let mut request: Vec<Ordered<RequestStep>> = Vec::new();
        let mut response: Vec<Ordered<Box<dyn ResponseHook>>> = Vec::new();
        let mut error: Vec<Ordered<Box<dyn ErrorHook>>> = Vec::new();
        let mut model = ModelHooks::new();

        for reg in &self.instances {
            let instance = reg.inner.name().to_string();
            let plugin = reg.inner.plugin_type_name().to_string();
            match (reg.installer, reg.inner.installs_hooks()) {
                // Registered with `plugin(..)`, but it hooks. The hooks would
                // go quiet, so the build stops and says what to write.
                (None, true) => return Err(BuildError::HooksNotInstalled { instance, plugin }),
                // Registered with `plugin_hooking(..)`, but the const that
                // makes the opposite mistake detectable is missing.
                (Some(_), false) => return Err(BuildError::HooksNotDeclared { instance, plugin }),
                (None, false) => {}
                (Some(install), true) => {
                    let mut registrar = HookRegistrar::new(
                        &instance,
                        &mut request,
                        &mut response,
                        &mut error,
                        &mut model,
                    );
                    install(reg.inner.plugin_any(), &mut registrar).map_err(|source| {
                        BuildError::Hook {
                            instance: instance.clone(),
                            stage: Stage::ConfigureRouter,
                            source,
                        }
                    })?;
                }
            }
        }

        // A hook registered into a family the record does not name is sorted,
        // stored, and then never run, because the entry point that walks it is
        // compiled out by `if !H::FAMILY { return }`. Nothing says so at run
        // time -- the hook is simply absent, which is the hardest kind of bug
        // to see. So every family is checked, and the build stops naming the
        // marker to write.
        //
        // All five, not only the async one: an unrun ETag hook is a
        // performance bug where an unrun session lookup is a security one, but
        // neither should be silent.
        let undeclared = [
            (
                !H::ASYNC_REQUEST,
                request
                    .iter()
                    .find(|o| matches!(o.value, RequestStep::Async(_)))
                    .map(|o| &o.id),
                "async request",
                "OnAsyncRequest",
            ),
            (
                !H::REQUEST,
                request
                    .iter()
                    .find(|o| matches!(o.value, RequestStep::Sync(_)))
                    .map(|o| &o.id),
                "request",
                "OnRequest",
            ),
            (
                !H::RESPONSE,
                response.first().map(|o| &o.id),
                "response",
                "OnResponse",
            ),
            (!H::ERROR, error.first().map(|o| &o.id), "error", "OnError"),
            (!H::MODEL, model.any_id(), "model", "OnModel"),
        ];
        for (missing, found, family, marker) in undeclared {
            if let (true, Some(id)) = (missing, found) {
                return Err(BuildError::FamilyNotDeclared {
                    instance: id.instance().to_string(),
                    hook: id.to_string(),
                    family,
                    marker,
                });
            }
        }

        let pipeline = Pipeline::from_parts(sort(request)?, sort(response)?, sort(error)?);
        Ok((pipeline, model.finish()?))
    }
}

fn run_stage<F>(
    stage: Stage,
    instances: &mut [Registered],
    names: &[String],
    services: &mut Services,
    readiness: &Readiness,
    mut hook: F,
) -> Result<(), BuildError>
where
    F: FnMut(&mut dyn AnyInstance, &mut Lifecycle<'_>) -> Result<(), PluginError>,
{
    for reg in instances.iter_mut() {
        let name = reg.inner.name().to_string();
        let mut lc = Lifecycle::new(stage, &name, reg.primary, names, services, readiness);
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
pub struct App<E = Base, H = NoHooks> {
    instances: Vec<Registered>,
    services: Services,
    readiness: Readiness,
    router: RouterPlan,
    pipeline: Pipeline,
    model: Arc<ModelHooks>,
    routes: RoutePlan<E>,
    streams: StreamGate,
    _providers: PhantomData<fn() -> E>,
    _families: PhantomData<fn() -> H>,
}

impl<E, H> std::fmt::Debug for App<E, H> {
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
            .field("hooks", &self.pipeline)
            .field("routes", &self.routes)
            .field("streams", &self.streams)
            .field("readiness", &self.readiness)
            .finish()
    }
}

impl<E, H> App<E, H> {
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

    /// The sorted hooks this application installed.
    pub fn pipeline(&self) -> &Pipeline {
        &self.pipeline
    }

    /// Every response body this application still has open, and the one bit
    /// that tells them all to end.
    ///
    /// Cloneable, so the transport can hold it: `brocanth-server` takes a
    /// clone and closes it the moment its listener stops, which is ADR 2's
    /// `Begin` arriving at the wire. [`App::shutdown`] closes the same gate,
    /// so whichever route a stop takes, an open stream is told once.
    ///
    /// The core never polls a body -- it has no runtime -- so it never waits
    /// here either. [`StreamGate::drained`] is the wait, and the transport
    /// awaits it under a budget it chooses.
    pub fn streams(&self) -> &StreamGate {
        &self.streams
    }

    /// The readiness checks the plugins registered, and the bit shutdown
    /// flips.
    ///
    /// Cloneable, so the probe can hold it. The core never runs a check: it
    /// has no runtime. Whatever serves `/readyz` awaits them. See
    /// [`crate::plugin_health`].
    pub fn readiness(&self) -> &Readiness {
        &self.readiness
    }

    /// Registers a streaming body with this application's gate.
    ///
    /// Every response the application produces goes through here, so a
    /// handler never has to know the gate exists.
    fn adopt(&self, response: &mut Response) {
        if let Some(stream) = response.body_mut().streaming_mut() {
            stream.watch(&self.streams);
        }
    }

    /// The model hooks, for a caller that wants to look rather than emit.
    pub fn model_hooks(&self) -> &ModelHooks {
        &self.model
    }

    /// Every route the application accepted from a plugin.
    pub fn route_plan(&self) -> &RoutePlan<E> {
        &self.routes
    }

    /// Plugins that contribute routes and whose routes were never mounted.
    ///
    /// Not an error — an application that does not mount the admin in
    /// production is the whole point of the seam — but it is the answer to
    /// "where did the admin go", and what `brocanth routes` prints under
    /// *offered, not mounted*.
    pub fn unmounted_route_sources(&self) -> Vec<&str> {
        self.instances
            .iter()
            .filter(|r| r.inner.contributes_routes())
            .filter(|r| {
                !self
                    .routes
                    .mounted_sources()
                    .iter()
                    .any(|m| m == r.inner.name())
            })
            .map(|r| r.inner.name())
            .collect()
    }

    /// Mounts every route the primary instance of `P` offers, under `prefix`.
    ///
    /// One line for a plugin with twelve screens, and the prefix is still the
    /// application's. Returns how many routes were taken.
    ///
    /// # Errors
    ///
    /// [`RouteError::NoSuchPlugin`] when `P` is not registered,
    /// [`RouteError::Conflict`] when a path and verb are already taken —
    /// naming both plugins — and [`RouteError::BadPath`] when a contributed
    /// path does not parse.
    pub fn mount_routes<P: RouteSource<E>>(&mut self, prefix: &str) -> Result<usize, RouteError> {
        self.mount_routes_where::<P>(prefix, |_| true)
    }

    /// [`App::mount_routes`] at the prefix the plugin suggests,
    /// [`RouteSource::PREFIX`].
    ///
    /// # Errors
    ///
    /// As [`App::mount_routes`].
    pub fn mount_routes_default<P: RouteSource<E>>(&mut self) -> Result<usize, RouteError> {
        self.mount_routes_where::<P>(P::PREFIX, |_| true)
    }

    /// [`App::mount_routes`], taking only the routes whose qualified name the
    /// filter accepts.
    ///
    /// "The admin, but not the destructive screens" in one line, with no
    /// cooperation needed from the plugin.
    ///
    /// # Errors
    ///
    /// As [`App::mount_routes`].
    pub fn mount_routes_where<P: RouteSource<E>>(
        &mut self,
        prefix: &str,
        filter: impl Fn(&str) -> bool,
    ) -> Result<usize, RouteError> {
        self.mount_instance_routes::<P>(None, prefix, &filter)
    }

    /// [`App::mount_routes`] for one named instance, so two instances of one
    /// plugin can live at two prefixes.
    ///
    /// # Errors
    ///
    /// As [`App::mount_routes`].
    pub fn mount_routes_of<P: RouteSource<E>>(
        &mut self,
        instance: &str,
        prefix: &str,
    ) -> Result<usize, RouteError> {
        self.mount_instance_routes::<P>(Some(instance), prefix, &|_| true)
    }

    fn mount_instance_routes<P: RouteSource<E>>(
        &mut self,
        instance: Option<&str>,
        prefix: &str,
        filter: &dyn Fn(&str) -> bool,
    ) -> Result<usize, RouteError> {
        let wanted = instance.unwrap_or(P::NAME);
        let Some(index) = self.index_of::<P>(instance) else {
            let registered: Vec<String> = self
                .instances
                .iter()
                .map(|r| r.inner.name().into())
                .collect();
            return Err(no_such_plugin(wanted, &registered));
        };

        let name = self.instances[index].inner.name().to_string();
        let plugin = self.instances[index]
            .inner
            .plugin_any()
            .downcast_ref::<P>()
            .expect("the index came from a type match");

        let mut taken = Vec::new();
        let mut bundle = RouteBundle::new(prefix, &name, filter, &mut taken);
        let described = plugin.routes(&mut bundle);
        let bad = bundle.into_errors();
        described.map_err(|error| RouteError::Source {
            plugin: name.clone(),
            error,
        })?;
        if let Some(first) = bad.into_iter().next() {
            return Err(first);
        }

        self.routes.accept(&name, taken)
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
// Running, starting and stopping
// ---------------------------------------------------------------------------

impl<E, H: HookFamilies> App<E, H> {
    /// One request, through the installed hooks and the handler.
    ///
    /// The server layer calls this. With no hook installed, `H` is
    /// [`NoHooks`] and the whole of it folds down to `dispatch(..)` plus the
    /// bare 500 for an error nobody claimed: no allocation, no dynamic
    /// dispatch, no branch on an empty list.
    pub fn run<F>(&self, request: &mut Request, state: &mut RequestState, dispatch: F) -> Response
    where
        F: FnOnce(&mut Request, &mut RequestState) -> Result<Response, HandlerError>,
    {
        let mut response = self.pipeline.run::<H, F>(request, state, dispatch);
        self.adopt(&mut response);
        response
    }

    /// The request hooks, for a caller that will `.await` the handler itself.
    ///
    /// See [`Pipeline::begin`]. An async server layer calls `begin`, awaits
    /// the handler, then calls [`App::finish`]; the core still holds no
    /// future and spawns nothing.
    #[inline]
    pub fn begin(&self, request: &mut Request, state: &mut RequestState) -> Begin {
        self.pipeline.begin::<H>(request, state)
    }

    /// The request hooks, including the ones that wait.
    ///
    /// See [`Pipeline::begin_async`]. An application that installed an
    /// [`AsyncRequestHook`](crate::plugin_hooks::AsyncRequestHook) must use
    /// this rather than [`App::run`] or [`App::begin`]: for it, the other two
    /// are a compile error rather than a chain walked halfway. An application
    /// without one may still use it. The chain is then the sync chain, and
    /// the future is ready on its first poll.
    ///
    /// [`App::finish`] is unchanged and is still synchronous, because nothing
    /// after dispatch waits.
    #[inline]
    pub async fn begin_async(&self, request: &mut Request, state: &mut RequestState) -> Begin {
        self.pipeline.begin_async::<H>(request, state).await
    }

    /// The error hooks, response hooks and `after_dispatch`, over whatever the
    /// handler produced. See [`App::begin`].
    ///
    /// A streaming body is registered with [`App::streams`] here, after every
    /// response hook has run: this is the last point the core touches a
    /// response, and a body registered earlier would be counted open while a
    /// hook could still replace it.
    #[inline]
    pub fn finish(
        &self,
        request: &mut Request,
        state: &mut RequestState,
        outcome: Result<Response, HandlerError>,
    ) -> Response {
        let mut response = self.pipeline.finish::<H>(request, state, outcome);
        self.adopt(&mut response);
        response
    }

    /// A handle an ORM keeps so its write path can emit model events.
    ///
    /// Cloning is an `Arc` bump, and
    /// [`emit`](crate::plugin_model::ModelDispatch::emit) is compiled away
    /// when no plugin installed a model hook.
    pub fn model_dispatch(&self) -> ModelDispatch<H> {
        ModelDispatch::new(Arc::clone(&self.model))
    }

    /// The application is serving: run [`Plugin::start`] for every instance,
    /// in registration order.
    ///
    /// # Errors
    ///
    /// [`BuildError::Hook`] naming the instance that failed. The instances
    /// before it have already started, so the caller's next move is
    /// [`App::shutdown`].
    pub fn start(&mut self) -> Result<(), BuildError> {
        let names: Vec<String> = self
            .instances
            .iter()
            .map(|r| r.inner.name().to_string())
            .collect();
        run_stage(
            Stage::Start,
            &mut self.instances,
            &names,
            &mut self.services,
            &self.readiness,
            |inst, lc| inst.start(lc),
        )
    }

    /// Stops the application: `begin_shutdown` for every instance, then
    /// `shutdown` for every instance, both in reverse registration order.
    ///
    /// `grace` is the budget for the whole shutdown, shared by every plugin
    /// and readable as [`ShutdownCx::remaining`]. [`None`] means no budget.
    ///
    /// Never returns an error, because a shutdown is a report: one plugin
    /// failing to drain must not stop the next one from closing. Read
    /// [`ShutdownReport::failures`].
    pub fn shutdown(&mut self, reason: ShutdownReason, grace: Option<Duration>) -> ShutdownReport {
        let started = Instant::now();
        let mut report = ShutdownReport::new(reason, grace);

        // Not ready, before anything else. A load balancer keeps routing to
        // a process until its readiness probe fails, so the probe must fail
        // before a plugin closes its gate (ADR 2). A transport that wants a
        // delay between the two calls `drain` earlier and waits; calling it
        // twice is harmless.
        self.readiness.drain();

        // Phase `Begin` for a response body, before any plugin is asked:
        // every open stream is told to end at its next chunk, and a parked
        // one is woken so that there is a next chunk. A plugin's own
        // `begin_shutdown` may then close the thing the stream reads from
        // without cutting a client off mid-frame.
        self.streams.close();

        for phase in [ShutdownPhase::Begin, ShutdownPhase::Drain] {
            for reg in self.instances.iter_mut().rev() {
                let name = reg.inner.name().to_string();
                let outcome = {
                    let mut sd = ShutdownCx::new(
                        reason,
                        phase,
                        grace,
                        started,
                        &name,
                        &mut self.services,
                        &self.streams,
                    );
                    match phase {
                        ShutdownPhase::Begin => reg.inner.begin_shutdown(&mut sd),
                        ShutdownPhase::Drain => reg.inner.shutdown(&mut sd),
                    }
                };
                report.record(&name);
                if let Err(error) = outcome {
                    report.fail(&name, phase, error);
                }
            }
        }

        // What is left is what the transport did not drain. The core cannot
        // wait for it -- only the thing polling a body can -- so it counts it
        // instead of claiming a clean stop it did not get.
        report.finish(started.elapsed(), self.streams.open_streams());
        report
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
