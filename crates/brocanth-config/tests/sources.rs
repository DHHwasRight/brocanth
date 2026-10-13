//! The whole stack, end to end: six sources, one merge, and a provenance for
//! every one of them.
//!
//! The unit tests in `src/` each check one piece. These check the piece that
//! cannot be unit tested — that the pieces agree about precedence, and that a
//! value coming from each possible origin says so.

#![cfg(all(feature = "toml", feature = "json", feature = "yaml"))]

use std::path::Path;

use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_context::NoContext;
use brocanth_config::{
    effective_of, AppBuilderConfigExt, Config, ConfigError, ConfiguredPlugin, Format, Loader,
    Provenance,
};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// A plugin with real defaults
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct Pool {
    max_size: u32,
    timeout_s: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct ServerConfig {
    host: String,
    port: u16,
    workers: u32,
    hosts: Vec<String>,
    pool: Pool,
}

struct ServerPlugin {
    config: ServerConfig,
}

impl Plugin for ServerPlugin {
    type Config = ServerConfig;
    type Provides = NoContext;
    const NAME: &'static str = "server";

    fn new(config: ServerConfig) -> Self {
        Self { config }
    }
}

impl ConfiguredPlugin for ServerPlugin {
    fn defaults() -> ServerConfig {
        ServerConfig {
            host: "0.0.0.0".into(),
            port: 3000,
            workers: 4,
            hosts: vec!["localhost".into()],
            pool: Pool {
                max_size: 5,
                timeout_s: 30,
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct LoggerConfig {
    level: String,
    json: bool,
}

struct LoggerPlugin {
    #[allow(dead_code)]
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

impl ConfiguredPlugin for LoggerPlugin {
    fn defaults() -> LoggerConfig {
        LoggerConfig {
            level: "info".into(),
            json: false,
        }
    }
}

/// A second plugin that also wants to be called `logger`.
struct AuditPlugin;

impl Plugin for AuditPlugin {
    type Config = LoggerConfig;
    type Provides = NoContext;
    const NAME: &'static str = "audit";

    fn new(_config: LoggerConfig) -> Self {
        Self
    }
}

impl ConfiguredPlugin for AuditPlugin {
    fn defaults() -> LoggerConfig {
        LoggerConfig {
            level: "warn".into(),
            json: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn lookup(
    pairs: &[(&str, &str)],
) -> impl Fn(&str) -> Option<String> + Send + Sync + 'static + use<> {
    let map: Vec<(String, String)> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    move |k: &str| {
        map.iter()
            .find(|(name, _)| name == k)
            .map(|(_, value)| value.clone())
    }
}

fn vars(pairs: &[(&str, &str)]) -> impl Fn() -> Vec<(String, String)> + Send + Sync + 'static {
    let owned: Vec<(String, String)> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    move || owned.clone()
}

// ---------------------------------------------------------------------------
// Precedence across every source there is
// ---------------------------------------------------------------------------

#[test]
fn every_source_wins_over_the_one_below_it_and_loses_to_the_one_above() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // 5. The lowest file: the committed defaults.
    write(
        &root.join("config/production.toml"),
        "[server]\nhost = \"10.0.0.1\"\nport = 3100\nworkers = 8\n\
         [server.pool]\nmax_size = 11\n",
    );
    // 4. The checkout.
    write(
        &root.join("my_app_production.toml"),
        "[server]\nhost = \"10.0.0.2\"\nport = 3200\nworkers = 9\n",
    );
    // 3. The machine.
    write(
        &root.join("etc/my_app/production.toml"),
        "[server]\nhost = \"10.0.0.3\"\nport = 3300\n",
    );
    // 2. The deployment, pointed at by hand.
    let explicit = root.join("deploy.toml");
    write(&explicit, "[server]\nhost = \"10.0.0.4\"\n");

    let config = Loader::new("my_app")
        .env("production")
        .root(root)
        .cwd(root)
        .env_lookup(lookup(&[("MY_APP_CONFIG", explicit.to_str().unwrap())]))
        // 1. The environment, above every file.
        .env_vars(vars(&[("MY_APP_SERVER__POOL__TIMEOUT_S", "99")]))
        .load()
        .unwrap();

    let app = AppBuilder::new()
        .with_config(config)
        .plugin_with_defaults::<ServerPlugin>("server")
        .unwrap()
        .build()
        .unwrap();

    let got = &app.plugin::<ServerPlugin>().unwrap().config;
    assert_eq!(got.host, "10.0.0.4", "the explicit file beats /etc");
    assert_eq!(got.port, 3300, "/etc beats the checkout");
    assert_eq!(got.workers, 9, "the checkout beats ./config");
    assert_eq!(got.pool.max_size, 11, "./config beats the plugin's default");
    assert_eq!(got.pool.timeout_s, 99, "the environment beats every file");
    assert_eq!(
        got.hosts,
        vec!["localhost".to_string()],
        "a key nothing set keeps the plugin's default"
    );
}

// ---------------------------------------------------------------------------
// Provenance, one assertion per possible origin
// ---------------------------------------------------------------------------

#[test]
fn a_value_from_each_possible_origin_says_where_it_came_from() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    write(
        &root.join("config/production.toml"),
        "[server]\nhost = \"10.0.0.1\"\n",
    );
    write(
        &root.join("my_app_production.yaml"),
        "server:\n  workers: 9\n",
    );
    write(
        &root.join("etc/my_app/production.json"),
        "{\n  \"server\": {\n    \"port\": 3300\n  }\n}\n",
    );
    let explicit = root.join("deploy.toml");
    write(&explicit, "[server]\nhosts = [\"a\", \"b\"]\n");

    let config = Loader::new("my_app")
        .env("production")
        .root(root)
        .cwd(root)
        .env_lookup(lookup(&[("MY_APP_CONFIG", explicit.to_str().unwrap())]))
        .env_vars(vars(&[("MY_APP_SERVER__POOL__TIMEOUT_S", "99")]))
        .load()
        .unwrap();

    let app = AppBuilder::new()
        .with_config(config)
        .plugin_with_defaults::<ServerPlugin>("server")
        .unwrap()
        .build()
        .unwrap();
    let effective = effective_of(app.services()).unwrap();

    // 1. An environment variable, named in full.
    match effective.provenance("server.pool.timeout_s").unwrap() {
        Provenance::EnvVar { var } => assert_eq!(&**var, "MY_APP_SERVER__POOL__TIMEOUT_S"),
        other => panic!("expected an env var, got {other:?}"),
    }

    // 2. The file `$MY_APP_CONFIG` named, with a line.
    let from = effective.provenance("server.hosts").unwrap();
    let file = from.file().expect("a file");
    assert_eq!(file.path, explicit);
    assert_eq!(file.format, Format::Toml);
    assert!(from.to_string().ends_with("deploy.toml:2"), "{from}");

    // 3. The system directory, in JSON, with a line.
    let from = effective.provenance("server.port").unwrap();
    assert_eq!(from.file().unwrap().format, Format::Json);
    assert!(from.to_string().ends_with("production.json:3"), "{from}");

    // 4. The working directory, in YAML, with a line.
    let from = effective.provenance("server.workers").unwrap();
    assert_eq!(from.file().unwrap().format, Format::Yaml);
    assert!(from.to_string().ends_with("production.yaml:2"), "{from}");

    // 5. The config directory, in TOML, with a line.
    let from = effective.provenance("server.host").unwrap();
    assert_eq!(from.file().unwrap().format, Format::Toml);
    assert!(from.to_string().ends_with("production.toml:2"), "{from}");

    // 6. A plugin's own default, naming the plugin.
    match effective.provenance("server.pool.max_size").unwrap() {
        Provenance::Default { plugin } => assert_eq!(&**plugin, "server"),
        other => panic!("expected a default, got {other:?}"),
    }
}

#[test]
fn an_overridden_value_remembers_every_layer_it_beat() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(&root.join("config/test.toml"), "[server]\nport = 3100\n");
    write(&root.join("my_app_test.toml"), "[server]\nport = 3200\n");

    let config = Loader::new("my_app")
        .env("test")
        .root(root)
        .cwd(root)
        .env_lookup(lookup(&[]))
        .env_vars(vars(&[("MY_APP_SERVER__PORT", "8080")]))
        .load()
        .unwrap();

    let app = AppBuilder::new()
        .with_config(config)
        .plugin_with_defaults::<ServerPlugin>("server")
        .unwrap()
        .build()
        .unwrap();

    let port = effective_of(app.services())
        .unwrap()
        .config()
        .root()
        .at("server.port")
        .unwrap();
    assert_eq!(port.as_integer(), Some(8080));

    let history: Vec<(String, String)> = port
        .shadowed()
        .iter()
        .map(|v| (v.display(), v.provenance().to_string()))
        .collect();
    assert_eq!(history.len(), 3, "two files and one default: {history:?}");
    assert_eq!(history[0].0, "3200");
    assert!(history[0].1.ends_with("my_app_test.toml:2"), "{history:?}");
    assert_eq!(history[1].0, "3100");
    assert!(history[1].1.ends_with("test.toml:2"), "{history:?}");
    assert_eq!(
        history[2],
        ("3000".to_string(), "default (server)".to_string())
    );
}

// ---------------------------------------------------------------------------
// The dump
// ---------------------------------------------------------------------------

#[test]
fn the_dump_separates_what_a_human_decided_from_what_a_plugin_did() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        &root.join("config/production.yaml"),
        "server:\n  port: 3100\n",
    );

    let config = Loader::new("my_app")
        .env("production")
        .root(root)
        .cwd(root)
        .env_lookup(lookup(&[]))
        .env_vars(vars(&[("MY_APP_SERVER__HOST", "127.0.0.1")]))
        .load()
        .unwrap();

    let app = AppBuilder::new()
        .with_config(config)
        .plugin_with_defaults::<ServerPlugin>("server")
        .unwrap()
        .build()
        .unwrap();

    let text = effective_of(app.services()).unwrap().dump().to_string();

    assert!(text.contains("my_app · production"), "{text}");
    assert!(text.contains("environment"), "{text}");
    assert!(text.contains("config dir"), "{text}");
    assert!(text.contains("defaults"), "{text}");

    // A human's decision is marked and attributed.
    assert!(text.contains("* server.host"), "{text}");
    assert!(text.contains("$MY_APP_SERVER__HOST"), "{text}");
    assert!(text.contains("* server.port"), "{text}");
    assert!(text.contains("production.yaml:2"), "{text}");
    assert!(text.contains("was 3000"), "the shadowed default: {text}");

    // A plugin's decision is not marked, and still attributed.
    assert!(text.contains("default (server)"), "{text}");
    for line in text.lines() {
        if line.contains("server.workers") {
            assert!(!line.trim_start().starts_with('*'), "{line}");
        }
    }

    assert!(
        text.contains("6 keys · 4 at their default · 2 overridden"),
        "{text}"
    );
}

// ---------------------------------------------------------------------------
// Two plugins, one section name
// ---------------------------------------------------------------------------

#[test]
fn two_plugins_claiming_one_section_is_an_error_that_names_both() {
    let err = AppBuilder::new()
        .with_config(Config::empty("my_app", "test"))
        .plugin_with_defaults::<LoggerPlugin>("logger")
        .unwrap()
        .plugin_as_with_defaults::<AuditPlugin>("logger")
        .unwrap_err();

    match &err {
        ConfigError::DuplicateSection {
            name,
            first,
            plugin,
        } => {
            assert_eq!(name, "logger");
            assert!(first.contains("LoggerPlugin"), "{first}");
            assert!(plugin.contains("AuditPlugin"), "{plugin}");
        }
        other => panic!("expected DuplicateSection, got {other:?}"),
    }

    let message = err.to_string();
    assert!(message.contains("LoggerPlugin"), "{message}");
    assert!(message.contains("AuditPlugin"), "{message}");
    assert!(message.contains("`[logger]`"), "{message}");
}

#[test]
fn two_plugins_under_two_names_are_fine() {
    let app = AppBuilder::new()
        .with_config(Config::empty("my_app", "test"))
        .plugin_with_defaults::<LoggerPlugin>("logger")
        .unwrap()
        .plugin_as_with_defaults::<AuditPlugin>("audit")
        .unwrap()
        .build()
        .unwrap();

    let effective = effective_of(app.services()).unwrap();
    assert_eq!(effective.claims().len(), 2);
    assert_eq!(
        effective.provenance("audit.level").unwrap().to_string(),
        "default (audit)"
    );
}

// ---------------------------------------------------------------------------
// The unclaimed key
// ---------------------------------------------------------------------------

#[test]
fn a_key_no_plugin_claims_stops_the_build_and_suggests_the_real_one() {
    let err = AppBuilder::new()
        .with_config(
            Config::from_toml("my_app", "test", "[server]\nhost = \"x\"\nworkrs = 8\n").unwrap(),
        )
        .plugin_with_defaults::<ServerPlugin>("server")
        .unwrap()
        .build()
        .unwrap_err();

    let message = err.to_string();
    assert!(message.contains("server.workrs"), "{message}");
    assert!(message.contains("did you mean `workers`?"), "{message}");
    assert!(
        message.contains("<inline toml>:3"),
        "it says where: {message}"
    );
}

#[test]
fn an_unclaimed_key_is_found_however_deep_it_is() {
    let err = AppBuilder::new()
        .with_config(Config::from_toml("my_app", "test", "[server.pool]\nmax_sze = 8\n").unwrap())
        .plugin_with_defaults::<ServerPlugin>("server")
        .unwrap()
        .build()
        .unwrap_err();

    let message = err.to_string();
    assert!(message.contains("server.pool.max_sze"), "{message}");
    assert!(message.contains("did you mean `max_size`?"), "{message}");
}

#[test]
fn a_key_inside_a_map_is_never_unclaimed_because_a_map_takes_any_key() {
    #[derive(Deserialize, Serialize)]
    struct TagsConfig {
        tags: std::collections::BTreeMap<String, String>,
    }

    struct TagsPlugin;

    impl Plugin for TagsPlugin {
        type Config = TagsConfig;
        type Provides = NoContext;
        const NAME: &'static str = "tags";
        fn new(_c: TagsConfig) -> Self {
            Self
        }
    }

    impl ConfiguredPlugin for TagsPlugin {
        fn defaults() -> TagsConfig {
            TagsConfig {
                tags: std::collections::BTreeMap::new(),
            }
        }
    }

    AppBuilder::new()
        .with_config(
            Config::from_toml("my_app", "test", "[tags.tags]\nanything = \"goes\"\n").unwrap(),
        )
        .plugin_with_defaults::<TagsPlugin>("tags")
        .unwrap()
        .build()
        .unwrap();
}

#[test]
fn a_plugin_that_declares_no_defaults_gets_no_key_check() {
    // `plugin_from_config` is the older path and it stays as it was: unknown
    // keys inside the section are that plugin's own business, through
    // `deny_unknown_fields` or not at all.
    #[derive(Default, Deserialize)]
    struct Loose {
        #[allow(dead_code)]
        level: String,
    }

    struct LoosePlugin;

    impl Plugin for LoosePlugin {
        type Config = Loose;
        type Provides = NoContext;
        const NAME: &'static str = "loose";
        fn new(_c: Loose) -> Self {
            Self
        }
    }

    AppBuilder::new()
        .with_config(
            Config::from_toml("my_app", "test", "[loose]\nlevel = \"x\"\nwhatever = 1\n").unwrap(),
        )
        .plugin_from_config::<LoosePlugin>("loose")
        .unwrap()
        .build()
        .unwrap();
}

// ---------------------------------------------------------------------------
// One configuration, three formats
// ---------------------------------------------------------------------------

/// The same configuration, written three ways. Loading each must give one
/// result, so a deployment can change format without changing behaviour.
#[test]
fn the_same_document_in_each_format_loads_to_the_same_values() {
    let toml = "\
[server]
host = \"127.0.0.1\"
port = 8080
workers = 2
hosts = [\"a\", \"b\"]

[server.pool]
max_size = 20
timeout_s = 5
";
    let json = "\
{
  \"server\": {
    \"host\": \"127.0.0.1\",
    \"port\": 8080,
    \"workers\": 2,
    \"hosts\": [\"a\", \"b\"],
    \"pool\": { \"max_size\": 20, \"timeout_s\": 5 }
  }
}
";
    let yaml = "\
server:
  host: 127.0.0.1
  port: 8080
  workers: 2
  hosts:
    - a
    - b
  pool:
    max_size: 20
    timeout_s: 5
";

    let expected = ServerConfig {
        host: "127.0.0.1".into(),
        port: 8080,
        workers: 2,
        hosts: vec!["a".into(), "b".into()],
        pool: Pool {
            max_size: 20,
            timeout_s: 5,
        },
    };

    for (format, text) in [
        (Format::Toml, toml),
        (Format::Json, json),
        (Format::Yaml, yaml),
    ] {
        let config = Config::from_text("my_app", "test", format, text).unwrap();
        let got: ServerConfig =
            brocanth_config::from_value(config.root().at("server").unwrap().clone()).unwrap();
        assert_eq!(got, expected, "reading {format}");

        // And the raw trees agree too, which is a stronger claim than the
        // typed one: it covers keys no plugin has declared.
        let reference = Config::from_text("my_app", "test", Format::Toml, toml).unwrap();
        assert_eq!(
            config.root(),
            reference.root(),
            "the raw tree from {format} differs from the one from toml"
        );
    }
}

#[test]
fn a_file_is_found_by_its_extension_whatever_format_it_is_in() {
    for (name, text) in [
        ("production.toml", "[server]\nport = 1\n"),
        ("production.json", "{ \"server\": { \"port\": 1 } }"),
        ("production.yaml", "server:\n  port: 1\n"),
        ("production.yml", "server:\n  port: 1\n"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("config").join(name), text);

        let config = Loader::new("my_app")
            .env("production")
            .root(dir.path())
            .cwd(dir.path())
            .env_lookup(lookup(&[]))
            .load()
            .unwrap();

        assert_eq!(config.sources().len(), 1, "{name}");
        assert_eq!(
            config.root().at("server.port").unwrap().as_integer(),
            Some(1),
            "{name}"
        );
    }
}

#[test]
fn one_slot_written_twice_is_an_error_naming_both_files() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("config/production.toml"),
        "[server]\nport = 1\n",
    );
    write(
        &dir.path().join("config/production.yaml"),
        "server:\n  port: 2\n",
    );

    let err = Loader::new("my_app")
        .env("production")
        .root(dir.path())
        .cwd(dir.path())
        .env_lookup(lookup(&[]))
        .load()
        .unwrap_err();

    assert!(
        matches!(err, ConfigError::AmbiguousSource { .. }),
        "{err:?}"
    );
    let message = err.to_string();
    assert!(message.contains("production.toml"), "{message}");
    assert!(message.contains("production.yaml"), "{message}");
}

// ---------------------------------------------------------------------------
// The merge rule, under the two new sources
// ---------------------------------------------------------------------------

#[test]
fn the_environment_replaces_an_array_whole_rather_than_patching_it() {
    // The rule is "tables recurse, scalars and arrays are replaced whole", and
    // an array spelled with indices does not get to be an exception: the whole
    // environment becomes one layer before anything merges.
    let config = Config::from_toml(
        "my_app",
        "test",
        "[server]\nhosts = [\"a\", \"b\", \"c\"]\n",
    )
    .unwrap();

    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("config/test.toml"),
        "[server]\nhosts = [\"a\", \"b\", \"c\"]\nport = 3100\n",
    );
    let loaded = Loader::new("my_app")
        .env("test")
        .root(dir.path())
        .cwd(dir.path())
        .env_lookup(lookup(&[]))
        .env_vars(vars(&[("MY_APP_SERVER__HOSTS__0", "z")]))
        .load()
        .unwrap();

    assert_eq!(
        config
            .root()
            .at("server.hosts")
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        3,
        "the file had three"
    );
    assert_eq!(
        loaded
            .root()
            .at("server.hosts")
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1,
        "one index set in the environment replaces the array whole"
    );
    assert_eq!(
        loaded.root().at("server.port").unwrap().as_integer(),
        Some(3100),
        "and the table around it still recurses"
    );
}

#[test]
fn a_null_in_a_higher_layer_drops_back_to_the_plugins_default() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // The committed file forces a port.
    write(&root.join("config/test.toml"), "[server]\nport = 3100\n");
    // This deployment wants the plugin's default back, and does not want to
    // have to know what it is.
    let explicit = root.join("deploy.yaml");
    write(&explicit, "server:\n  port: null\n");

    let config = Loader::new("my_app")
        .env("test")
        .root(root)
        .cwd(root)
        .env_lookup(lookup(&[("MY_APP_CONFIG", explicit.to_str().unwrap())]))
        .env_vars(vars(&[]))
        .load()
        .unwrap();

    assert!(
        config.root().at("server.port").is_none(),
        "the key is gone from the found document"
    );

    let app = AppBuilder::new()
        .with_config(config)
        .plugin_with_defaults::<ServerPlugin>("server")
        .unwrap()
        .build()
        .unwrap();

    assert_eq!(app.plugin::<ServerPlugin>().unwrap().config.port, 3000);
    assert_eq!(
        effective_of(app.services())
            .unwrap()
            .provenance("server.port")
            .unwrap()
            .to_string(),
        "default (server)"
    );
}

// ---------------------------------------------------------------------------
// An application that configures nothing still boots
// ---------------------------------------------------------------------------

#[test]
fn an_application_that_configures_nothing_boots_and_can_still_say_why() {
    let dir = tempfile::tempdir().unwrap();
    let config = Loader::new("my_app")
        .env("production")
        .root(dir.path())
        .cwd(dir.path())
        .env_lookup(lookup(&[]))
        .load()
        .unwrap();

    assert!(config.sources().is_empty());

    let app = AppBuilder::new()
        .with_config(config)
        .plugin_with_defaults::<ServerPlugin>("server")
        .unwrap()
        .plugin_as_with_defaults::<LoggerPlugin>("logger")
        .unwrap()
        .build()
        .unwrap();

    assert_eq!(
        app.plugin::<ServerPlugin>().unwrap().config,
        ServerPlugin::defaults()
    );

    let text = effective_of(app.services()).unwrap().dump().to_string();
    assert!(text.contains("0 overridden"), "{text}");
    assert!(text.contains("default (server)"), "{text}");
    assert!(text.contains("default (logger)"), "{text}");
}
