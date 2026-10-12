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
//! features. There are five of them, and they are the whole contract.
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
//! **Lifecycle hooks**, in this order: [`Plugin::configure`],
//! [`Plugin::register`], [`Plugin::after_config`], then the application's own
//! setup block, then [`Plugin::configure_router`], then
//! [`Plugin::before_start`]. Each stage completes for every instance before
//! the next one starts, so `after_config` is the first point at which
//! cross-plugin lookups through [`Services`] are valid.
//!
//! **Request hooks** — before dispatch and after the response — which is what
//! a logger, a metrics plugin and an ORM's connection-per-request all need.
//! **Shutdown hooks**, so a connection pool can drain. Both are declared here
//! as part of the contract and land with the HTTP server layer; until then a
//! plugin that needs them is waiting on the core, not working around it.
//!
//! Anything a plugin cannot do through those seams is a gap in the core, to be
//! fixed by widening the seam rather than by special-casing the plugin. A
//! plugin reaching around them — a global registry, a hidden `static`, a
//! feature flag in the core that names one plugin — is the failure this design
//! exists to prevent.
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
//! [`ProvidesContext<T>`]: plugin_context::ProvidesContext
//! [`Services`]: plugin_api::Services

pub mod plugin_api;
pub mod plugin_app;
pub mod plugin_context;
pub mod routing;
pub mod routing_codegen;
pub mod routing_middleware;
pub mod routing_params;
pub mod routing_path;
pub mod routing_pattern;
pub mod routing_verb;
pub mod suggest;
