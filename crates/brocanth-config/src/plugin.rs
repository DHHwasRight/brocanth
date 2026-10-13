//! Config as a plugin, and the seam a plugin contributes a section through.
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
//! # The seam for a plugin's own defaults
//!
//! [`ConfiguredPlugin`] is the new half. A plugin implements it next to its
//! `impl Plugin`, states the value every key takes when nothing sets it, and
//! the application registers it with
//! [`plugin_with_defaults`](AppBuilderConfigExt::plugin_with_defaults). Three
//! things follow, and each is a §8 obligation rather than a convenience:
//!
//! 1. **The defaults become the lowest layer of the document**, with a
//!    [`Provenance::Default`](crate::Provenance::Default) on every node, so
//!    [`Effective::dump`] prints them attributed. A default nobody can see is
//!    the half of Rails PLAN.md rejects; a default the dump names is the half
//!    it keeps.
//! 2. **The key set of the section is known**, because serde distinguishes a
//!    struct from a map, so a key the plugin has not got is an error with the
//!    nearest real key suggested. See [`crate::ser`].
//! 3. **Two plugins claiming one section is an error naming both**, raised at
//!    the second `plugin_with_defaults` call rather than resolved by
//!    last-one-wins.
//!
//! A plugin that is not named in `main` does none of this, contributes no
//! section, no default and no byte. §8.1 is untouched: the seam is a trait the
//! plugin implements and the *application* calls.

use std::sync::{Arc, Mutex};

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError, Services};
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_context::{NoContext, Provided};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::config::{Config, ConfigError};
use crate::ser::to_value;
use crate::value::{self, Provenance, Value};

/// The configuration, shared by the plugin that polices it and by every plugin
/// that wants to read beyond its own section.
pub type SharedConfig = Arc<Config>;

/// The loaded configuration, if an application installed one.
///
/// A plugin reads the whole document with
/// `brocanth_config::config_of(lc.services())`. The core has no `config()`
/// accessor to offer, because the core has no configuration.
///
/// This is the configuration as *found*: files and the environment, with no
/// plugin defaults under it. [`effective_of`] is the one with the defaults, and
/// it exists from `configure` onwards.
pub fn config_of(services: &Services) -> Option<&Config> {
    services.get::<SharedConfig>().map(|c| c.as_ref())
}

/// The configuration an application actually runs with.
///
/// Available from the end of [`ConfigPlugin::configure`], which is the first
/// hook of the first stage, so every plugin after that point can read it.
pub fn effective_of(services: &Services) -> Option<&Effective> {
    services.get::<Arc<Effective>>().map(|e| e.as_ref())
}

// ---------------------------------------------------------------------------
// Section claims
// ---------------------------------------------------------------------------

/// One plugin's claim on one configuration section.
#[derive(Clone, Debug)]
pub struct SectionClaim {
    /// The section name, which is also the plugin's instance name.
    pub section: String,
    /// The plugin's Rust type, so an error can name it unambiguously.
    pub plugin: String,
    /// The plugin's defaults, as a value tree. Closed tables in it are the key
    /// set of the section.
    pub defaults: Value,
}

/// Every claim made so far. Shared, because claims arrive one builder call at a
/// time and the check that reads them runs later.
pub(crate) type SharedClaims = Arc<Mutex<Vec<SectionClaim>>>;

/// The configuration after every registered plugin's defaults are under it.
pub struct Effective {
    config: Config,
    claims: Vec<SectionClaim>,
}

impl Effective {
    /// The merged document: defaults, then files, then the environment.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Every section a registered plugin claimed, in registration order.
    #[must_use]
    pub fn claims(&self) -> &[SectionClaim] {
        &self.claims
    }

    /// Where one key came from. `eff.provenance("server.pool.max_size")`.
    #[must_use]
    pub fn provenance(&self, path: &str) -> Option<&Provenance> {
        self.config.provenance(path)
    }

    /// The whole effective configuration, printed for a human.
    ///
    /// ```text
    /// println!("{}", brocanth_config::effective_of(app.services()).unwrap().dump());
    /// ```
    #[must_use]
    pub fn dump(&self) -> crate::dump::Dump<'_> {
        self.config.dump_with(&self.claims)
    }
}

// ---------------------------------------------------------------------------
// The plugin seam
// ---------------------------------------------------------------------------

/// A plugin that owns a configuration section and knows what it defaults to.
///
/// Implemented beside `impl Plugin`, in the plugin's own crate:
///
/// ```
/// # use brocanth::plugin_api::Plugin;
/// # use brocanth::plugin_context::NoContext;
/// use brocanth_config::ConfiguredPlugin;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize)]
/// struct LoggerConfig {
///     level: String,
///     json: bool,
/// }
///
/// struct LoggerPlugin { config: LoggerConfig }
///
/// impl Plugin for LoggerPlugin {
///     type Config = LoggerConfig;
///     type Provides = NoContext;
///     const NAME: &'static str = "logger";
///     fn new(config: LoggerConfig) -> Self { Self { config } }
/// }
///
/// impl ConfiguredPlugin for LoggerPlugin {
///     fn defaults() -> LoggerConfig {
///         LoggerConfig { level: "info".into(), json: false }
///     }
/// }
/// ```
///
/// `defaults()` and not `Default` on purpose: a plugin's *configuration*
/// default and the Rust `Default` of its configuration type are allowed to
/// differ, and tying them would make a plugin author choose between a sensible
/// `Default` and a sensible configuration. It also makes the declaration
/// visible: a reader of the plugin crate sees the word `defaults` and knows
/// exactly what an unconfigured application will run with.
///
/// There is no `schema()`. The reason is written up in `lib.rs`: `Deserialize`
/// is already the schema, it is checked by the compiler, and a second
/// description beside it could drift from it.
pub trait ConfiguredPlugin: Plugin {
    /// Every key this plugin has, at the value it takes when nothing sets it.
    fn defaults() -> Self::Config;
}

// ---------------------------------------------------------------------------
// The plugin
// ---------------------------------------------------------------------------

/// Configuration, as a plugin.
///
/// Registered by [`AppBuilderConfigExt::with_config`]. Its job is the pair of
/// checks the Crystal original did not make and Hugo still does not make:
///
/// * a top-level section that no registered instance claims stops the build;
/// * a key inside a claimed section that its plugin has not got stops the
///   build.
///
/// Both suggest the nearest real name. A misspelt section or key that builds
/// clean, logs nothing and silently drops a feature is the failure both
/// prevent, and it is the same failure class PLAN.md §9 names for a misspelt
/// rule field.
///
/// It then publishes the [`Effective`] configuration, so every plugin after it
/// — and the application — can print the whole thing with the origin of every
/// key.
pub struct ConfigPlugin {
    config: SharedConfig,
    claims: SharedClaims,
}

impl ConfigPlugin {
    /// The configuration this plugin polices, as it was found.
    pub fn config(&self) -> &Config {
        &self.config
    }
}

impl Plugin for ConfigPlugin {
    type Config = (SharedConfig, SharedClaims);
    type Provides = NoContext;
    const NAME: &'static str = "config";

    fn new((config, claims): (SharedConfig, SharedClaims)) -> Self {
        Self { config, claims }
    }

    fn configure(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        // `configure` is the first hook of the first stage, so an unclaimed
        // section or key is reported before any other plugin opens a socket.
        let claims = self
            .claims
            .lock()
            .map_err(|_| PluginError::new("the configuration claim list is poisoned"))?
            .clone();

        self.config
            .validate_sections(lc.instance_names())
            .map_err(|e| PluginError::new(e.to_string()))?;
        self.config
            .validate_keys(&claims)
            .map_err(|e| PluginError::new(e.to_string()))?;

        let mut config = (*self.config).clone();
        config.merge_under(defaults_layer(&claims));
        lc.services_mut()
            .insert(Arc::new(Effective { config, claims }));
        Ok(())
    }
}

/// Every claim's defaults, as one table ready to go underneath the document.
fn defaults_layer(claims: &[SectionClaim]) -> Value {
    let mut root = Value::new(
        crate::value::ValueKind::Table(crate::value::Table::open()),
        Provenance::inline("defaults"),
    );
    let table = root.as_table_mut().expect("just built as a table");
    for claim in claims {
        table.insert(claim.section.clone(), claim.defaults.clone());
    }
    root
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
    /// them, and leaving it off accepts them. Use
    /// [`AppBuilderConfigExt::plugin_with_defaults`] to get that check, a
    /// better error for it, and the plugin's defaults in the dump.
    ///
    /// # Errors
    ///
    /// [`ConfigError`] when no configuration is installed or the section does
    /// not fit `P::Config`.
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
    ///
    /// # Errors
    ///
    /// [`ConfigError`] when no configuration is installed or the section does
    /// not fit `P::Config`.
    fn plugin_as_from_config<P>(self, name: &str) -> Result<AppBuilder<E>, ConfigError>
    where
        P: Plugin,
        P::Config: DeserializeOwned + Default;

    /// Registers `P` at the section named `section`, with `P`'s own defaults
    /// underneath it.
    ///
    /// This is the Rails-shaped path, and the one that pays for itself: an
    /// application that configures nothing still boots, and
    /// [`Effective::dump`] still says, key by key, which plugin decided what.
    ///
    /// It also claims the section. A second plugin claiming the same name is
    /// [`ConfigError::DuplicateSection`], naming both, here rather than at
    /// `build()`.
    ///
    /// # Errors
    ///
    /// [`ConfigError`] when no configuration is installed, the section is
    /// already claimed, the plugin's defaults cannot be represented, or the
    /// merged section does not fit `P::Config`.
    fn plugin_with_defaults<P>(
        self,
        section: &str,
    ) -> Result<AppBuilder<<P::Provides as Provided>::Install<E>>, ConfigError>
    where
        P: ConfiguredPlugin,
        P::Config: Serialize + DeserializeOwned;

    /// Registers another instance of `P` at `name`, with `P`'s defaults
    /// underneath it.
    ///
    /// The provider list is not widened, for the reason
    /// [`AppBuilderConfigExt::plugin_as_from_config`] gives.
    ///
    /// # Errors
    ///
    /// The same as [`AppBuilderConfigExt::plugin_with_defaults`].
    fn plugin_as_with_defaults<P>(self, name: &str) -> Result<AppBuilder<E>, ConfigError>
    where
        P: ConfiguredPlugin,
        P::Config: Serialize + DeserializeOwned;
}

impl<E> AppBuilderConfigExt<E> for AppBuilder<E> {
    fn with_config(mut self, config: Config) -> AppBuilder<E> {
        let shared: SharedConfig = Arc::new(config);
        let claims: SharedClaims = Arc::new(Mutex::new(Vec::new()));
        self.services_mut().insert(shared.clone());
        self.services_mut().insert(claims.clone());
        self.plugin(ConfigPlugin::new((shared, claims)))
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

    fn plugin_with_defaults<P>(
        self,
        section: &str,
    ) -> Result<AppBuilder<<P::Provides as Provided>::Install<E>>, ConfigError>
    where
        P: ConfiguredPlugin,
        P::Config: Serialize + DeserializeOwned,
    {
        let config = claim_and_resolve::<P, E>(&self, section)?;
        Ok(self.plugin_named(section, P::new(config)))
    }

    fn plugin_as_with_defaults<P>(self, name: &str) -> Result<AppBuilder<E>, ConfigError>
    where
        P: ConfiguredPlugin,
        P::Config: Serialize + DeserializeOwned,
    {
        let config = claim_and_resolve::<P, E>(&self, name)?;
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

/// Records `P`'s claim on `name` and resolves its configuration: the plugin's
/// defaults, with whatever the document says over the top.
fn claim_and_resolve<P, E>(builder: &AppBuilder<E>, name: &str) -> Result<P::Config, ConfigError>
where
    P: ConfiguredPlugin,
    P::Config: Serialize + DeserializeOwned,
{
    let config =
        builder
            .services()
            .get::<SharedConfig>()
            .ok_or_else(|| ConfigError::NoConfigInstalled {
                name: name.to_string(),
            })?;
    let claims =
        builder
            .services()
            .get::<SharedClaims>()
            .ok_or_else(|| ConfigError::NoConfigInstalled {
                name: name.to_string(),
            })?;

    let plugin = std::any::type_name::<P>().to_string();
    let mut defaults = to_value(&P::defaults(), Provenance::default_of(name)).map_err(|e| {
        ConfigError::Defaults {
            name: name.to_string(),
            message: e.to_string(),
        }
    })?;

    {
        let mut claimed = claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(first) = claimed.iter().find(|c| c.section == name) {
            return Err(ConfigError::DuplicateSection {
                name: name.to_string(),
                first: first.plugin.clone(),
                plugin,
            });
        }
        claimed.push(SectionClaim {
            section: name.to_string(),
            plugin: plugin.clone(),
            defaults: defaults.clone(),
        });
    }

    // The plugin's defaults, with the document over the top. The merge rule is
    // the crate's one rule and it is what makes a file that sets one key keep
    // every other default in the section.
    if let Some(found) = config.raw_section(name) {
        let found = Value::new(
            crate::value::ValueKind::Table(found.clone()),
            Provenance::inline("found"),
        );
        value::merge(&mut defaults, found);
    }
    config.deserialize_section::<P::Config>(name, defaults)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(all(test, feature = "toml"))]
mod tests {
    use super::*;
    use brocanth::plugin_app::BuildError;
    use brocanth::plugin_context::{Base, ContextBuilder, ContextItem, ProvidesContext, With};
    use serde::Deserialize;
    use std::sync::atomic::{AtomicU64, Ordering};

    // -- a plugin whose section is a table -----------------------------------

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
