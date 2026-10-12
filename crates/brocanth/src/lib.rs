//! Brocanth: a server framework for HTML-first applications.
//!
//! The Crystal original is a 1,272-line prototype, so this is a build rather
//! than a port. What carries over is its design, which was argued and
//! recorded: file-based routes with compile-time helpers, layouts nesting by
//! directory, pattern-scoped middleware, plugins that enrich context
//! explicitly rather than by autowiring, and a handler that returns a value
//! the framework turns into a response.
//!
//! What does not carry over is everything the original's shape forced. It
//! served GET only, had no persistence, and kept its registries in global
//! mutable state so two applications in one process collided. Each of those is
//! a Crystal-shaped workaround, and in Rust the type system deletes rather
//! than documents them.
//!
//! # Nothing ships unless it is asked for
//!
//! **This crate is a router handing a request and a context to a controller.**
//! That is the whole of it. It has three dependencies — `wayfind`, `http` and
//! `thiserror` — and `tests/no_hidden_dependencies.rs` fails the build if a
//! fourth kind arrives: no TOML parser, no serde, no SQL driver, no logging
//! framework, no async runtime. Modularity claimed in a README rots;
//! modularity asserted in CI does not.
//!
//! Everything else is a plugin that must be named in `main`. A plugin that is
//! not imported is not compiled, not linked, and contributes nothing: no
//! dependency in the tree, no field on the context, no bytes in the binary.
//! The dependency arrow points from the plugin to the core, always.
//!
//! Configuration is a plugin like any other. The core takes already-built
//! values — `app.plugin(LoggerPlugin::new(LoggerConfig { level: Level::Warn }))`
//! — so a service configured entirely from its environment links no parser.
//! `brocanth-config` is the crate that reads TOML files, layers four sources,
//! merges them and rejects a section nobody claims, and it adds
//! `.plugin_from_config::<LoggerPlugin>("logger")` to [`AppBuilder`] through an
//! extension trait.
//!
//! # The seams this crate publishes
//!
//! The core cannot know what plugins exist, so it publishes seams rather than
//! features. There are six levels, and together they are the whole contract.
//! Anything a plugin cannot do through them is a gap in the core, to be fixed
//! by widening the seam rather than by special-casing the plugin.
//!
//! ## 1. Application — build, start, stop
//!
//! Eight hooks, in this order: [`Plugin::configure`], [`Plugin::register`],
//! [`Plugin::after_config`], the application's own setup block, hook
//! installation, [`Plugin::configure_router`], [`Plugin::before_start`] — then
//! the server binds and [`App::start`] runs [`Plugin::start`], and
//! [`App::shutdown`] runs [`Plugin::begin_shutdown`] and then
//! [`Plugin::shutdown`]. Each stage completes for every instance before the
//! next one starts, so `after_config` is the first point at which cross-plugin
//! lookups through [`Services`] are valid.
//!
//! Shutdown is two phases, in reverse registration order, because one phase
//! cannot serve both a connection pool and a job runner: `begin_shutdown`
//! closes the gate and returns, `shutdown` drains within
//! [`ShutdownCx::remaining`], and one plugin failing does not stop the next
//! one from closing. See [`plugin_shutdown`].
//!
//! ## 2. Router — a plugin contributing routes
//!
//! A plugin implements [`RouteSource`] and describes its routes relative to a
//! prefix it does not choose. The application mounts the whole bundle in one
//! line — `app.mount_routes::<AdminPlugin>("/backoffice")?` — or a subset with
//! [`App::mount_routes_where`]. Nothing mounts itself, the prefix is always
//! the application's, and [`App::unmounted_route_sources`] lists what was
//! offered and not taken. Two plugins on one path is an error naming both.
//!
//! ## 3. Request — before and after dispatch
//!
//! [`RequestHook::before_dispatch`] runs in declared order and may
//! short-circuit; [`RequestHook::after_dispatch`] runs in reverse declared
//! order and sees the finished response whatever happened to it. Which is what
//! a logger, a metrics plugin and an ORM's connection-per-request all need.
//!
//! ## 4. Response — rewriting headers and body
//!
//! [`ResponseHook::on_response`], in **reverse** declared order, so the hook
//! declared last is the outermost: compression wraps the ETag that an inner
//! hook computed. Partial-navigation headers live here too.
//!
//! ## 5. Model — the seam an ORM registers into
//!
//! The core cannot name a connection, a transaction or a row, so it does not
//! try. The ORM declares an event *kind* — a `'static` marker — and what the
//! event carries at a lifetime, through [`ModelEvent::Payload`]. Any plugin
//! registers a [`ModelHook`] for that kind, the ORM emits from inside its own
//! write path through [`ModelDispatch`], and the core forwards a value it
//! never looks inside. The dependency arrow still points from the ORM to the
//! core. See [`plugin_model`].
//!
//! ## 6. Error — turning a plugin's error into a response
//!
//! A handler fails with a [`HandlerError`], which carries any error without
//! understanding it. [`ErrorHook::on_error`] recognises its own type through
//! [`HandlerError::downcast_ref`] and answers, so a validation failure renders
//! a form and an auth failure redirects, with the core knowing neither
//! concept. First hook to answer wins; an error nobody claims is a bare 500.
//!
//! ## And the two that were already here
//!
//! **Context providers.** A plugin contributes typed values to the per-request
//! context: it names a [`ProvidesContext<T>`] as its [`Plugin::Provides`] and
//! fills it in [`Plugin::provide`]. The installed provider set lives in the
//! *type* of the application, so a handler that asks for a value whose plugin
//! was never registered does not compile. See [`plugin_context`].
//!
//! **Middleware offers.** A plugin *offers* a layer through
//! [`MiddlewareCatalog::offer`]; only the application mounts it, at a pattern
//! the application chooses, in its [`AppBuilder::setup`] block. This was the
//! Crystal author's explicit instruction — *"I do not want plugins to register
//! middleware themselves. I want to manually inject middlewares since I may
//! want to configure the pattern."* — and the type system is what keeps it: a
//! [`MiddlewareCatalog`] has no `mount`. Mounting a name nobody offered fails
//! the build and suggests the nearest offered name.
//!
//! # Order, and what it costs to be free
//!
//! Two plugins hooking one point run in the order one of them *declared*. A
//! hook carries an [`Order`] — `after("session")`, `before("compression")`,
//! naming either an instance or one `instance.hook` — and registration order
//! is only the tie-break between hooks that said nothing about each other. A
//! required dependency nobody answers is a build error with a suggestion; a
//! cycle is a build error naming both ends and printing the loop. See
//! [`plugin_order`].
//!
//! A hook nobody registered costs nothing: no allocation, no dynamic dispatch
//! and no branch on an empty list. An empty `Vec` still has to be looked at to
//! find out it is empty, so emptiness is not a run-time fact here — it is a
//! fact about the application's *type*. [`AppBuilder`] carries a
//! [`HookFamilies`] record beside the context provider list, and the hot path
//! is generic over it. `tests/hooks_cost_nothing.rs` reads the generated LLVM
//! IR and fails if the empty case is anything other than a single `ret`.
//!
//! The price is one extra builder call: a plugin that hooks is registered with
//! [`AppBuilder::plugin_hooking`] rather than `plugin`, and declares
//! `const INSTALLS_HOOKS: bool = true` so that using the wrong one fails the
//! build with the line to write instead. PLAN.md ADR 1 is the argument.
//!
//! A plugin reaching around these seams — a global registry, a hidden
//! `static`, a distributed slice, a feature flag in the core that names one
//! plugin — is the failure this design exists to prevent.
//!
//! [`AppBuilder`]: plugin_app::AppBuilder
//! [`AppBuilder::setup`]: plugin_app::AppBuilder::setup
//! [`MiddlewareCatalog`]: plugin_api::MiddlewareCatalog
//! [`MiddlewareCatalog::offer`]: plugin_api::MiddlewareCatalog::offer
//! [`Plugin`]: plugin_api::Plugin
//! [`Plugin::Provides`]: plugin_api::Plugin::Provides
//! [`Plugin::configure`]: plugin_api::Plugin::configure
//! [`Plugin::register`]: plugin_api::Plugin::register
//! [`Plugin::after_config`]: plugin_api::Plugin::after_config
//! [`Plugin::configure_router`]: plugin_api::Plugin::configure_router
//! [`Plugin::before_start`]: plugin_api::Plugin::before_start
//! [`Plugin::provide`]: plugin_api::Plugin::provide
//! [`Plugin::start`]: plugin_api::Plugin::start
//! [`Plugin::begin_shutdown`]: plugin_api::Plugin::begin_shutdown
//! [`Plugin::shutdown`]: plugin_api::Plugin::shutdown
//! [`AppBuilder::plugin_hooking`]: plugin_app::AppBuilder::plugin_hooking
//! [`App::start`]: plugin_app::App::start
//! [`App::shutdown`]: plugin_app::App::shutdown
//! [`App::mount_routes_where`]: plugin_app::App::mount_routes_where
//! [`App::unmounted_route_sources`]: plugin_app::App::unmounted_route_sources
//! [`ErrorHook::on_error`]: plugin_hooks::ErrorHook::on_error
//! [`HandlerError`]: plugin_hooks::HandlerError
//! [`HandlerError::downcast_ref`]: plugin_hooks::HandlerError::downcast_ref
//! [`HookFamilies`]: plugin_hooks::HookFamilies
//! [`ModelDispatch`]: plugin_model::ModelDispatch
//! [`ModelEvent::Payload`]: plugin_model::ModelEvent::Payload
//! [`ModelHook`]: plugin_model::ModelHook
//! [`Order`]: plugin_order::Order
//! [`ProvidesContext<T>`]: plugin_context::ProvidesContext
//! [`RequestHook::before_dispatch`]: plugin_hooks::RequestHook::before_dispatch
//! [`RequestHook::after_dispatch`]: plugin_hooks::RequestHook::after_dispatch
//! [`ResponseHook::on_response`]: plugin_hooks::ResponseHook::on_response
//! [`RouteSource`]: plugin_routes::RouteSource
//! [`Services`]: plugin_api::Services
//! [`ShutdownCx::remaining`]: plugin_shutdown::ShutdownCx::remaining

pub mod plugin_api;
pub mod plugin_app;
pub mod plugin_context;
pub mod plugin_hooks;
pub mod plugin_model;
pub mod plugin_order;
pub mod plugin_routes;
pub mod plugin_shutdown;
pub mod routing;
pub mod routing_codegen;
pub mod routing_middleware;
pub mod routing_params;
pub mod routing_path;
pub mod routing_pattern;
pub mod routing_verb;
pub mod suggest;
