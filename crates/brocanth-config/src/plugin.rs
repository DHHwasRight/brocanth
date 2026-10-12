//! Config as a plugin, and the one `use` that gives an application the
//! ergonomic path back.
//!
//! The core takes already-built values: `app.plugin(LoggerPlugin::new(
//! LoggerConfig { level: Level::Warn }))`. That is honest and it is exactly
//! Rails' railtie model, where an engine is initialised explicitly rather than
//! discovered. It is also tedious when there are nine plugins and a file that
//! already says what each one wants.
//!
//! So this module adds [`AppBuilderConfigExt`]: one `use`, and the builder
//! grows `.with_config(..)` and `.plugin_from_config::<LoggerPlugin>("logger")`.
//! The convenience survives; it is no longer in the core, and an application
//! that never writes that `use` links no part of it.
//!
//! [`ConfigPlugin`] is what makes "config is a plugin too" literal rather than
//! a slogan. It is registered like any other plugin, it runs the same
//! `configure` hook as any other plugin, and the only thing it does there is
//! read the roster of registered instances and refuse a configuration section
//! that none of them claims.

use std::sync::Arc;

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError, Services};
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_context::{NoContext, Provided};
use serde::de::DeserializeOwned;

use crate::config::{Config, ConfigError};

/// The configuration, shared by the plugin that polices it and by every plugin
/// that wants to read beyond its own section.
pub type SharedConfig = Arc<Config>;

/// The loaded configuration, if an application installed one.
///
/// A plugin reads the whole document with
/// `brocanth_config::config_of(lc.services())`. The core has no `config()`
/// accessor to offer, because the core has no configuration.
pub fn config_of(services: &Services) -> Option<&Config> {
    services.get::<SharedConfig>().map(|c| c.as_ref())
}

// ---------------------------------------------------------------------------
// The plugin
// ---------------------------------------------------------------------------

/// Configuration, as a plugin.
///
/// Registered by [`AppBuilderConfigExt::with_config`]. Its one job is the
/// check the Crystal original did not make and Hugo still does not make: a
/// top-level section that no registered instance claims stops the build and
/// the error suggests the nearest claimed name. A misspelt section that builds
/// clean, logs nothing and silently drops a feature is the failure this
/// prevents.
pub struct ConfigPlugin {
    config: SharedConfig,
}

impl ConfigPlugin {
    /// The configuration this plugin polices.
    pub fn config(&self) -> &Config {
        &self.config
    }
}

impl Plugin for ConfigPlugin {
    type Config = SharedConfig;
    type Provides = NoContext;
    const NAME: &'static str = "config";

    fn new(config: SharedConfig) -> Self {
        Self { config }
    }

    fn configure(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        // `configure` is the first hook of the first stage, so an unclaimed
        // section is reported before any other plugin opens a socket.
        self.config
            .validate_sections(lc.instance_names())
            .map_err(|e| PluginError::new(e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// The extension trait
// ---------------------------------------------------------------------------

/// File configuration for [`AppBuilder`], in one `use`.
///
/// ```no_run
/// use brocanth::plugin_app::AppBuilder;
/// use brocanth_config::{AppBuilderConfigExt, Loader};
/// # use brocanth::plugin_api::{Plugin, PluginError};
/// # use brocanth::plugin_context::NoContext;
/// # #[derive(Default, serde::Deserialize)] struct LoggerConfig { level: String }
/// # struct LoggerPlugin;
/// # impl Plugin for LoggerPlugin {
/// #     type Config = LoggerConfig;
/// #     type Provides = NoContext;
/// #     const NAME: &'static str = "logger";
/// #     fn new(_c: LoggerConfig) -> Self { Self }
/// # }
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let app = AppBuilder::new()
///     .with_config(Loader::new("my_app").load()?)
///     .plugin_from_config::<LoggerPlugin>("logger")?
///     .build()?;
/// # Ok(())
/// # }
/// ```
///
/// Without this crate the same application writes
/// `.plugin(LoggerPlugin::new(LoggerConfig { level: "warn".into() }))` and
/// links no parser at all. Both paths are supported, and they mix: a plugin
/// registered the plain way claims its section name just the same, so the
/// unknown-section check still covers it.
pub trait AppBuilderConfigExt<E>: Sized {
    /// Installs a loaded configuration and registers [`ConfigPlugin`].
    ///
    /// Call it before the plugins that read from it.
    fn with_config(self, config: Config) -> AppBuilder<E>;

    /// Registers `P`, built from the section named `section`.
    ///
    /// The instance takes the section's name, so a second instance of the same
    /// plugin reads a second section. This is the registration that widens the
    /// context provider list, exactly as `AppBuilder::plugin` does; use
    /// [`AppBuilderConfigExt::plugin_as_from_config`] for extra instances.
    ///
    /// A missing section is not an error: the plugin's `Config::default()` is
    /// used, so `[logger]` stays optional. Unknown keys *inside* the section
    /// are the plugin's business — `#[serde(deny_unknown_fields)]` rejects
    /// them, and leaving it off accepts them.
    fn plugin_from_config<P>(
        self,
        section: &str,
    ) -> Result<AppBuilder<<P::Provides as Provided>::Install<E>>, ConfigError>
    where
        P: Plugin,
        P::Config: DeserializeOwned + Default;

    /// Registers another instance of `P`, built from the section named `name`.
    ///
    /// The provider list is not widened: a context item is one value, and two
    /// instances would only fight over it.
    fn plugin_as_from_config<P>(self, name: &str) -> Result<AppBuilder<E>, ConfigError>
    where
        P: Plugin,
        P::Config: DeserializeOwned + Default;
}

impl<E> AppBuilderConfigExt<E> for AppBuilder<E> {
    fn with_config(mut self, config: Config) -> AppBuilder<E> {
        let shared: SharedConfig = Arc::new(config);
        self.services_mut().insert(shared.clone());
        self.plugin(ConfigPlugin::new(shared))
    }

    fn plugin_from_config<P>(
        self,
        section: &str,
    ) -> Result<AppBuilder<<P::Provides as Provided>::Install<E>>, ConfigError>
    where
        P: Plugin,
        P::Config: DeserializeOwned + Default,
    {
        let config = section_of::<P>(&self, section)?;
        Ok(self.plugin_named(section, P::new(config)))
    }

    fn plugin_as_from_config<P>(self, name: &str) -> Result<AppBuilder<E>, ConfigError>
    where
        P: Plugin,
        P::Config: DeserializeOwned + Default,
    {
        let config = section_of::<P>(&self, name)?;
        Ok(self.plugin_as(name, P::new(config)))
    }
}

fn section_of<P>(builder: &AppBuilder<impl Sized>, name: &str) -> Result<P::Config, ConfigError>
where
    P: Plugin,
    P::Config: DeserializeOwned + Default,
{
    let config =
        builder
            .services()
            .get::<SharedConfig>()
            .ok_or_else(|| ConfigError::NoConfigInstalled {
                name: name.to_string(),
            })?;
    config.section::<P::Config>(name)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use brocanth::plugin_app::BuildError;
    use brocanth::plugin_context::{Base, ContextBuilder, ContextItem, ProvidesContext, With};
    use serde::Deserialize;
    use std::sync::atomic::{AtomicU64, Ordering};

    // -- a plugin whose section is a TOML table -----------------------------

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
        config: LoggerConfig,
    }

    impl Plugin for LoggerPlugin {
        type Config = LoggerConfig;
        type Provides = NoContext;
        const NAME: &'static str = "logger";

        fn new(config: LoggerConfig) -> Self {
            Self { config }
        }
    }

    #[derive(Debug, Deserialize)]
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

    struct RequestIdPlugin {
        prefix: String,
        counter: AtomicU64,
    }

    impl Plugin for RequestIdPlugin {
        type Config = RequestIdConfig;
        type Provides = ProvidesContext<RequestId>;
        const NAME: &'static str = "request_id";

        fn new(config: RequestIdConfig) -> Self {
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

    fn config(text: &str) -> Config {
        Config::from_toml("my_app", "test", text).unwrap()
    }

    // -- typed sections through the plugin ----------------------------------

    #[test]
    fn each_instance_reads_the_section_named_after_it() {
        let app = AppBuilder::new()
            .with_config(config(
                r#"
                [logger]
                level = "warn"
                [db_log]
                level = "debug"
                json = true
                "#,
            ))
            .plugin_from_config::<LoggerPlugin>("logger")
            .unwrap()
            .plugin_as_from_config::<LoggerPlugin>("db_log")
            .unwrap()
            .build()
            .unwrap();

        assert_eq!(
            app.plugin::<LoggerPlugin>().unwrap().config,
            LoggerConfig {
                level: "warn".into(),
                json: false
            }
        );
        assert_eq!(
            app.plugin_named::<LoggerPlugin>("db_log").unwrap().config,
            LoggerConfig {
                level: "debug".into(),
                json: true
            }
        );
    }

    #[test]
    fn a_missing_section_leaves_the_plugin_with_its_default() {
        let app = AppBuilder::new()
            .with_config(config(""))
            .plugin_from_config::<LoggerPlugin>("logger")
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            app.plugin::<LoggerPlugin>().unwrap().config,
            LoggerConfig::default()
        );
    }

    #[test]
    fn an_unknown_key_inside_a_plugin_section_is_that_plugins_business() {
        // LoggerConfig opted into deny_unknown_fields, so it rejects. Nothing
        // above it looked.
        let err = AppBuilder::new()
            .with_config(config("[logger]\nlevel = \"warn\"\nverbose = true\n"))
            .plugin_from_config::<LoggerPlugin>("logger")
            .unwrap_err();
        assert!(matches!(err, ConfigError::Section { .. }), "{err:?}");
    }

    #[test]
    fn asking_for_a_section_before_installing_a_configuration_says_so() {
        let err = AppBuilder::new()
            .plugin_from_config::<LoggerPlugin>("logger")
            .unwrap_err();
        assert!(
            matches!(err, ConfigError::NoConfigInstalled { .. }),
            "{err:?}"
        );
        assert!(err.to_string().contains("with_config"), "{err}");
    }

    // -- the unclaimed section ----------------------------------------------

    #[test]
    fn an_unclaimed_top_level_section_stops_the_build() {
        let err = AppBuilder::new()
            .with_config(config("[loggr]\nlevel = \"warn\"\n"))
            .plugin_from_config::<LoggerPlugin>("logger")
            .unwrap()
            .build()
            .unwrap_err();
        let msg = err.to_string();
        assert!(matches!(err, BuildError::Hook { .. }), "{err:?}");
        assert!(msg.contains("`[loggr]`"), "{msg}");
        assert!(msg.contains("did you mean `[logger]`?"), "{msg}");
    }

    #[test]
    fn a_plugin_registered_the_plain_way_still_claims_its_section() {
        // Mixing the two paths must not turn a plugin's own section into an
        // unknown one: the claim is the instance name, not the route it was
        // registered by.
        AppBuilder::new()
            .with_config(config("[logger]\nlevel = \"warn\"\n"))
            .plugin(LoggerPlugin::new(LoggerConfig::default()))
            .build()
            .unwrap();
    }

    // -- the context provider survives the extension trait ------------------

    #[test]
    fn a_provider_registered_from_config_still_widens_the_provider_list() {
        let app = AppBuilder::new()
            .with_config(config("[request_id]\nprefix = \"web\"\n"))
            .plugin_from_config::<RequestIdPlugin>("request_id")
            .unwrap()
            .build()
            .unwrap();

        fn handler(cx: &brocanth::plugin_context::Context<With<RequestId, Base>>) -> String {
            let id: &RequestId = cx.extract();
            id.0.clone()
        }

        assert_eq!(handler(&app.context()), "web-1");
        assert_eq!(handler(&app.context()), "web-2");
    }

    // -- the configuration is readable by any plugin ------------------------

    #[test]
    fn a_plugin_reads_the_whole_document_through_the_services() {
        let app = AppBuilder::new()
            .with_config(config("[logger]\nlevel = \"warn\"\n"))
            .plugin_from_config::<LoggerPlugin>("logger")
            .unwrap()
            .build()
            .unwrap();

        let cfg = config_of(app.services()).unwrap();
        assert_eq!(cfg.env(), "test");
        assert_eq!(cfg.app().as_str(), "my_app");
        assert!(cfg.raw_section("logger").is_some());
    }

    // -- two applications, two configurations -------------------------------

    #[test]
    fn two_configured_applications_in_one_process_do_not_interfere() {
        // The half of the core's isolation test that needed a configuration to
        // state: two applications, two documents, no leak either way.
        let app_a = AppBuilder::new()
            .with_config(
                Config::from_toml("my_app", "production", "[logger]\nlevel = \"warn\"\n").unwrap(),
            )
            .plugin_from_config::<LoggerPlugin>("logger")
            .unwrap()
            .build()
            .unwrap();

        let app_b = AppBuilder::new()
            .with_config(
                Config::from_toml(
                    "other_app",
                    "development",
                    "[logger]\nlevel = \"debug\"\n[db_log]\nlevel = \"trace\"\n",
                )
                .unwrap(),
            )
            .plugin_from_config::<LoggerPlugin>("logger")
            .unwrap()
            .plugin_as_from_config::<LoggerPlugin>("db_log")
            .unwrap()
            .build()
            .unwrap();

        assert_eq!(config_of(app_a.services()).unwrap().env(), "production");
        assert_eq!(config_of(app_b.services()).unwrap().env(), "development");
        assert_eq!(
            config_of(app_a.services()).unwrap().app().as_str(),
            "my_app"
        );
        assert_eq!(
            config_of(app_b.services()).unwrap().app().as_str(),
            "other_app"
        );

        assert_eq!(app_a.plugin::<LoggerPlugin>().unwrap().config.level, "warn");
        assert_eq!(
            app_b.plugin::<LoggerPlugin>().unwrap().config.level,
            "debug"
        );

        // A section claimed by B is unknown to A, which is why A would have
        // rejected it.
        assert_eq!(app_a.instances_of::<LoggerPlugin>(), vec!["logger"]);
        assert_eq!(
            app_b.instances_of::<LoggerPlugin>(),
            vec!["logger", "db_log"]
        );
    }
}
