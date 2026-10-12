//! Configuration for brocanth — and nothing in the core knows it exists.
//!
//! PLAN.md §8 is the governing constraint: *"The core is a router handing a
//! request and a context to a controller… Config is a plugin too. The core
//! takes already-parsed values. TOML, the environment-scoped file resolution,
//! and the merge order are a crate an application opts into — a service
//! configured entirely from its environment should not link a TOML parser."*
//!
//! So `serde` and `toml` live here. The dependency arrow points from this
//! crate to [`brocanth`] and never back; `brocanth`'s own
//! `tests/no_hidden_dependencies.rs` fails the build if it ever does.
//!
//! # What this crate keeps
//!
//! Every behaviour the configuration layer had before the split, moved rather
//! than rebuilt:
//!
//! * **One name convention.** [`AppName`] normalises `MyApp`, `my_app`,
//!   `my-app` and `My App` to one spelling, so there is one `MY_APP_ENV` and
//!   one `MY_APP_CONFIG` rather than the original's two prefixes and a third
//!   in its README.
//! * **Four sources, highest precedence first:** `$<APP>_CONFIG`,
//!   `/etc/<app>/<env>.toml`, `./<app>_<env>.toml`, `./config/<env>.toml`. A
//!   file named by the environment variable and missing is always an error.
//! * **Deep merge.** Tables recurse; scalars and arrays are replaced whole, so
//!   a deployment layers a small file over the committed one.
//! * **Sections are enriched, not pre-declared.** [`Config`] holds raw tables
//!   and knows what none of them mean. A plugin declares the type its section
//!   deserialises into.
//! * **An unclaimed section is an error**, with the nearest claimed name
//!   suggested. A misspelt section that builds clean and silently drops a
//!   feature is the failure this prevents.
//!
//! # The two ways an application configures a plugin
//!
//! Without this crate, in the core alone — no parser is linked, and the value
//! is checked by the compiler rather than by a deserialiser:
//!
//! ```
//! # use brocanth::plugin_api::{Plugin, PluginError};
//! # use brocanth::plugin_app::AppBuilder;
//! # use brocanth::plugin_context::NoContext;
//! # enum Level { Warn }
//! # struct LoggerConfig { level: Level }
//! # struct LoggerPlugin;
//! # impl Plugin for LoggerPlugin {
//! #     type Config = LoggerConfig;
//! #     type Provides = NoContext;
//! #     const NAME: &'static str = "logger";
//! #     fn new(_c: LoggerConfig) -> Self { Self }
//! # }
//! let app = AppBuilder::new()
//!     .plugin(LoggerPlugin::new(LoggerConfig { level: Level::Warn }))
//!     .build()?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! With this crate, one `use` brings the ergonomic path back:
//!
//! ```no_run
//! # use brocanth::plugin_api::{Plugin, PluginError};
//! # use brocanth::plugin_app::AppBuilder;
//! # use brocanth::plugin_context::NoContext;
//! use brocanth_config::{AppBuilderConfigExt, Loader};
//! # #[derive(Default, serde::Deserialize)] struct LoggerConfig { level: String }
//! # struct LoggerPlugin;
//! # impl Plugin for LoggerPlugin {
//! #     type Config = LoggerConfig;
//! #     type Provides = NoContext;
//! #     const NAME: &'static str = "logger";
//! #     fn new(_c: LoggerConfig) -> Self { Self }
//! # }
//! let app = AppBuilder::new()
//!     .with_config(Loader::new("my_app").load()?)
//!     .plugin_from_config::<LoggerPlugin>("logger")?
//!     .build()?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! The two mix freely in one application.

mod config;
mod plugin;

pub use config::{
    AppName, Config, ConfigError, Loader, Origin, Source, CORE_SECTIONS, DEFAULT_ENV,
};
pub use plugin::{config_of, AppBuilderConfigExt, ConfigPlugin, SharedConfig};
