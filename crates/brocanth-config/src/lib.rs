//! Configuration for brocanth — and nothing in the core knows it exists.
//!
//! PLAN.md §8 is the governing constraint: *"The core is a router handing a
//! request and a context to a controller… Config is a plugin too. The core
//! takes already-parsed values. TOML, the environment-scoped file resolution,
//! and the merge order are a crate an application opts into — a service
//! configured entirely from its environment should not link a TOML parser."*
//!
//! So `serde` and every parser live here. The dependency arrow points from this
//! crate to [`brocanth`] and never back; `brocanth`'s own
//! `tests/no_hidden_dependencies.rs` fails the build if it ever does.
//!
//! # What this crate keeps
//!
//! * **One name convention.** [`AppName`] normalises `MyApp`, `my_app`,
//!   `my-app` and `My App` to one spelling, so there is one `MY_APP_ENV`, one
//!   `MY_APP_CONFIG` and one `MY_APP_SERVER__PORT`.
//! * **Sources, highest precedence first:** the environment, `$<APP>_CONFIG`,
//!   `/etc/<app>/<env>.*`, `./<app>_<env>.*`, `./config/<env>.*`, and a
//!   registered plugin's own defaults underneath all of them. A file named by
//!   the environment variable and missing is always an error.
//! * **Deep merge.** Tables recurse; scalars and arrays are replaced whole, so
//!   a deployment layers a small file over the committed one.
//! * **Sections are enriched, not pre-declared.** [`Config`] holds raw values
//!   and knows what none of them mean. A plugin declares the type its section
//!   deserialises into.
//! * **An unclaimed section is an error**, with the nearest claimed name
//!   suggested. A misspelt section that builds clean and silently drops a
//!   feature is the failure this prevents.
//!
//! # What this crate adds
//!
//! ## Provenance, as the data model
//!
//! Every value carries the [`Provenance`] of where it came from and of every
//! value it replaced. A plugin's default, a file and a line, or a named
//! environment variable: four origins, no `Option`, no side table.
//!
//! This is the part that makes the rest safe. §8 allows Rails' convention over
//! configuration and rejects *"the framework decided for you and you cannot see
//! where"*. A plugin that supplies its own defaults is squarely the second
//! thing — **unless the effective configuration is inspectable**, which is
//! what [`Effective::dump`] is for:
//!
//! ```text
//! my_app · production
//!
//!   sources, highest precedence first
//!    1  environment    MY_APP_*                              2 variables
//!    2  working dir    /srv/app/my_app_production.toml       toml
//!    3  config dir     /srv/app/config/production.yaml       yaml
//!    4  defaults       2 plugins                             server, logger
//!
//!   [logger]  claimed by `my_log::LoggerPlugin`
//!     logger.json                    = false                       default (logger)
//!   * logger.level                   = "error"                     [2] my_app_production.toml:2
//!                                      was "warn"                  [3] production.yaml:5
//!                                      was "info"                  default (logger)
//!
//!   [server]  claimed by `brocanth_server::ServerPlugin`
//!     server.host                    = "0.0.0.0"                   default (server)
//!   * server.pool.max_size           = 50                          $MY_APP_SERVER__POOL__MAX_SIZE
//!                                      was 5                       default (server)
//!     server.pool.timeout_s          = 30                          default (server)
//!   * server.port                    = 8080                        $MY_APP_SERVER__PORT
//!                                      was 3100                    [3] production.yaml:2
//!                                      was 3000                    default (server)
//!   * server.workers                 = 16                          [3] production.yaml:3
//!                                      was 4                       default (server)
//!
//!   7 keys · 3 at their default · 4 overridden (marked *)
//! ```
//!
//! A `*` in the first column means a human decided; a blank means the framework
//! did, and the right-hand column always says which human or which plugin. The
//! `was` lines read down the value column, so "the port is 8080, it was 3100 in
//! the YAML, and the plugin wanted 3000" is one glance rather than three.
//!
//! `cargo run -p brocanth-config --all-features --example dump` prints that.
//!
//! ## More than TOML
//!
//! TOML, JSON and YAML, each behind its own cargo feature, plus the
//! environment. §8.2 again: a format that is off leaves its parser out of the
//! build, not behind a branch. `default = ["toml"]`, so a service that reads
//! JSON writes:
//!
//! ```toml
//! brocanth-config = { version = "0.1", default-features = false, features = ["json"] }
//! ```
//!
//! and links no TOML parser.
//!
//! The crates were chosen on one requirement: **a parser must report the
//! position of every value, not only of an error.** A provenance that cannot
//! name a line is worth very little at three in the morning, which rules out
//! `serde_json` and all of the `serde_yaml` family whatever their maintenance
//! status. See [`crate::formats`].
//!
//! Environment variables are not a file format and get their own decisions —
//! the spelling of a nested key, the typing of a string, and an array. They are
//! written up in [`crate::env`], with the reasons.
//!
//! ## Plugin-contributed sections
//!
//! A plugin implements [`ConfiguredPlugin`], and the application registers it
//! with [`AppBuilderConfigExt::plugin_with_defaults`]. The plugin's defaults
//! become the lowest layer, attributed; the section's key set becomes known, so
//! a typo in a key is an error with a suggestion; and two plugins claiming one
//! section name is an error naming both. A plugin not named in `main` does none
//! of this and contributes nothing, which is §8.1 untouched.
//!
//! # Two things decided against, and why
//!
//! **No `brocanth-schema`.** That crate's `SchemaNode` is a real validation
//! vocabulary and it would have been plausible here. It does not pay. A
//! plugin's configuration type already has a `Deserialize` implementation,
//! which *is* a schema, is derived, is checked by the compiler, and cannot
//! drift from the type it builds. A `SchemaNode` beside it would be a second
//! description of the same thing, written by hand, free to disagree with the
//! first, and the disagreement would surface as a configuration that validates
//! and then fails to deserialise. The one thing a schema was wanted for — the
//! key set of a section, for the unclaimed-key check — came out of serde
//! itself, because serde distinguishes a struct from a map and so tells us
//! which tables have a complete key set. See [`crate::ser`]. Taking the
//! dependency would also point this crate at a sibling plugin crate, which §8
//! allows but does not encourage without cause. The door stays open:
//! [`ConfiguredPlugin`] can grow a provided `schema()` method later without
//! breaking any implementation.
//!
//! **No hot reload.** A configuration that changes under a running process
//! turns every `&Config` into a lock and every plugin's `new(config)` into a
//! lie — the plugin was *built* from its configuration, per §8's "a plugin
//! arrives already configured". Restarting is the supported way to change a
//! configuration. Database-stored settings and secret management are declined
//! too, the first by PLAN.md §12 and the second because a secret that passes
//! through this crate would end up in a dump.
//!
//! Declining to manage secrets does not stop a deployment writing one into a
//! file, so [`Redaction`] is the other half of that decision: a key named like
//! a secret, and the password inside a connection URL, do not reach a reader.
//! Every [`Dump`] applies it, and so does the **Settings** tab. See
//! [`crate::redact`] for the rule and the two alternatives it beat.
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
//! use brocanth_config::{AppBuilderConfigExt, ConfiguredPlugin, Loader};
//! # #[derive(serde::Serialize, serde::Deserialize)] struct LoggerConfig { level: String }
//! # struct LoggerPlugin;
//! # impl Plugin for LoggerPlugin {
//! #     type Config = LoggerConfig;
//! #     type Provides = NoContext;
//! #     const NAME: &'static str = "logger";
//! #     fn new(_c: LoggerConfig) -> Self { Self }
//! # }
//! # impl ConfiguredPlugin for LoggerPlugin {
//! #     fn defaults() -> LoggerConfig { LoggerConfig { level: "info".into() } }
//! # }
//! let app = AppBuilder::new()
//!     .with_config(Loader::new("my_app").load()?)
//!     .plugin_with_defaults::<LoggerPlugin>("logger")?
//!     .build()?;
//!
//! println!("{}", brocanth_config::effective_of(app.services()).unwrap().dump());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! The two mix freely in one application.

#![warn(missing_docs)]

mod config;
mod de;
mod dump;
pub mod env;
pub mod formats;
mod plugin;
pub mod redact;
pub mod ser;
mod value;

/// The **Settings** tab in `brocanth-admin`.
///
/// Behind the `admin` feature, which is off. A service that reads a TOML file
/// and serves no backoffice links neither the admin nor rsx.
#[cfg(feature = "admin")]
pub mod admin;

/// The tab's screen, compiled in by rsx at macro expansion: nothing is read
/// from disk at run time.
///
/// At the crate root and not inside [`admin`], because a template's own `use`
/// line names this path.
#[cfg(feature = "admin")]
mod admin_templates {
    rsx::load_dir!("src/admin_templates");
}

pub use config::{
    AppName, Config, ConfigError, Loader, Origin, Source, UnknownKey, CORE_SECTIONS, DEFAULT_ENV,
};
pub use de::{from_value, DeError, ValueDeserializer};
pub use dump::Dump;
pub use plugin::{
    config_of, effective_of, AppBuilderConfigExt, ConfigPlugin, ConfiguredPlugin, Effective,
    SectionClaim, SharedConfig,
};
pub use redact::{Reason, Redaction, Shown};
pub use value::{merge, merge_tables, Format, Provenance, Table, Value, ValueKind};
