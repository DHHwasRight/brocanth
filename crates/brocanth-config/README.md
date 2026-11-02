# brocanth-config

`brocanth-config` loads the configuration of a brocanth application. It reads
TOML, JSON and YAML files and the environment, merges them in layers, and
records where each value came from. A plugin gets its own section,
deserialised into its own type. Use it when an application reads settings
from files per environment. Do not use it for a service that is configured
in code or from a few variables: the core takes plain values, and without
this crate no parser is linked. This crate does not reload a changed file
and does not manage secrets.

## Getting started

Add the crate. TOML is the default format. JSON (`json`) and YAML (`yaml`)
are separate features. The `admin` feature adds a **Settings** tab to
`brocanth-admin`.

```toml
[dependencies]
brocanth-config = { git = "https://github.com/DHHwasRight/brocanth", branch = "main" }
# JSON only, and no TOML parser:
# brocanth-config = { git = "...", branch = "main", default-features = false, features = ["json"] }
```

In a generated project, the `config` module adds this crate and writes
`config/<env>.toml`:

```text
brocanth create my-app --with config,sessions
brocanth add config
```

The `config` module has no settings of its own. It changes how every other
module reads its settings: each declared setting gets a key under
`[<module>]` in `config/<env>.toml`, and each plugin is built with
`plugin_from_config`. A plugin that installs hooks is built with
`plugin_hooking_from_config`, and its section is the plugin's instance
name, such as `[security_headers]`. `brocanth add config` rewrites each plugin's settings struct to derive
`Deserialize`. It refuses, and prints the change, only for a file you
have edited. See
[docs/module-flags.md](../../docs/module-flags.md).

### Layers and precedence

Lowest to highest:

| Layer | Source | Provenance |
|---|---|---|
| default | a plugin's own defaults (`ConfiguredPlugin::defaults`), or `Settings::default()` | `Provenance::Default` |
| files | `./config/<env>.*`, then `./<app>_<env>.*`, then `/etc/<app>/<env>.*`, then the file `$<APP>_CONFIG` names | `Provenance::File` (path, line, column) |
| environment | `$<APP>_<SECTION>__<KEY>`, with `__` between path segments | `Provenance::EnvVar` |
| flags | `Loader::flag(path, flag, raw, text)`: `--<section>.<key>=VALUE` in a generated binary | `Provenance::Flag` |

The narrower decision wins. A file applies to every deployment, a variable
to every process one shell starts, and a flag to one run. The environment
name comes from `$<APP>_ENV` and defaults to `development`. `AppName`
normalises `MyApp`, `my_app`, `my-app` and `My App` to one prefix,
`MY_APP`. A file that `$<APP>_CONFIG` names and that is missing is an
error.

Tables merge key by key. A scalar or an array replaces the lower value
whole. Each value keeps the values it replaced, so a report can say "8080,
was 3100 in the file, was 3000 by default".

### Empty values and `None`

| Given | Value | Reads as |
|---|---|---|
| nothing | the layer below decides | |
| `--x=` or `MY_APP_S__X=` (empty) | `ValueKind::Empty` | `None` into an `Option`, `""` into a `String`, a type error into a number or a bool |
| JSON or YAML `null` | `ValueKind::Unset` | removes the key, so the plugin default applies |
| the alias set with `Loader::none_as(path, alias)` | `ValueKind::Empty` | `None` |

`Empty` survives a merge, because somebody decided it. `Unset` does not.
An alias is set per key and never globally: `-1` is invalid for an
unsigned field and a real value for some signed fields.

### The settings report

`Config::layers()` prints each layer alone, lowest first, with only the
keys it sets. The merge follows and names the layer of each value. A
generated project prints it with `my-app settings` (with `commands`) or
`my-app --settings`. `Effective::dump()` prints the merged view with the
history of each value. Both mask secrets with the standard `Redaction`.

## Examples

Read one section from TOML text into a type:

```rust
use brocanth_config::Config;

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct Server {
    host: String,
    port: u16,
}

let config = Config::from_toml("my_app", "development", "[server]\nport = 8080\n")?;
let server: Server = config.section("server")?;
assert_eq!(server.port, 8080);
assert_eq!(server.host, ""); // not in the text: `Default` applies
assert_eq!(config.provenance("server.port").unwrap().to_string(), "<inline toml>:2");
# Ok::<(), brocanth_config::ConfigError>(())
```

Load the layers. A variable beats the file, and a flag beats the variable.
The loader reads the real environment and filesystem unless you inject
them, as this example does:

```rust
use brocanth_config::Loader;

let dir = tempfile::tempdir()?;
std::fs::create_dir_all(dir.path().join("config"))?;
std::fs::write(
    dir.path().join("config/production.toml"),
    "[server]\nport = 3100\nworkers = 4\n",
)?;

let config = Loader::new("my_app")
    .env("production")
    .root(dir.path())
    .cwd(dir.path())
    .env_lookup(|_| None)
    .env_vars(|| vec![("MY_APP_SERVER__PORT".into(), "8080".into())])
    .flag("server.workers", "server.workers", "16", false)
    .load()?;

let server = config.raw_section("server").unwrap();
assert_eq!(server.get("port").unwrap().as_integer(), Some(8080));
assert_eq!(server.get("workers").unwrap().as_integer(), Some(16));
assert_eq!(config.provenance("server.port").unwrap().to_string(), "$MY_APP_SERVER__PORT");
assert_eq!(config.provenance("server.workers").unwrap().to_string(), "--server.workers");

// Each layer alone, then the merge. Pass the defaults in: only the
// application can build its settings types.
let report = config
    .layers()
    .defaults(vec![("server.port", "3000".to_string())])
    .to_string();
assert!(report.starts_with("layers, lowest precedence first"));
assert!(report.contains("$MY_APP_SERVER__PORT"));
# Ok::<(), Box<dyn std::error::Error>>(())
```

An empty flag and a declared alias both mean `None`:

```rust
use brocanth_config::Loader;

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct Limits {
    per_minute: Option<i64>,
    burst: Option<i64>,
    label: String,
}

let dir = tempfile::tempdir()?;
std::fs::create_dir_all(dir.path().join("config"))?;
std::fs::write(
    dir.path().join("config/development.toml"),
    "[limits]\nper_minute = 60\nburst = -1\nlabel = \"api\"\n",
)?;

let config = Loader::new("my_app")
    .root(dir.path())
    .cwd(dir.path())
    .env_lookup(|_| None)
    .flag("limits.per_minute", "limits.per_minute", "", false)
    .flag("limits.label", "limits.label", "", true)
    .none_as("limits.burst", "-1")
    .load()?;

let limits: Limits = config.section("limits")?;
assert_eq!(limits.per_minute, None); // `--limits.per_minute=`
assert_eq!(limits.burst, None); // `-1` in the file
assert_eq!(limits.label, ""); // empty into a string is ""
# Ok::<(), Box<dyn std::error::Error>>(())
```

Register a plugin whose defaults are the lowest layer. An unknown section
or key then stops `build()` with the nearest real name:

```rust,no_run
use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_context::NoContext;
use brocanth_config::{effective_of, AppBuilderConfigExt, ConfiguredPlugin, Loader};

#[derive(serde::Serialize, serde::Deserialize)]
struct LoggerConfig {
    level: String,
}

struct LoggerPlugin;

impl Plugin for LoggerPlugin {
    type Config = LoggerConfig;
    type Provides = NoContext;
    const NAME: &'static str = "logger";
    fn new(_config: LoggerConfig) -> Self {
        Self
    }
}

impl ConfiguredPlugin for LoggerPlugin {
    fn defaults() -> LoggerConfig {
        LoggerConfig { level: "info".into() }
    }
}

let app = AppBuilder::new()
    .with_config(Loader::new("my_app").load()?)
    .plugin_with_defaults::<LoggerPlugin>("logger")?
    .build()?;

println!("{}", effective_of(app.services()).unwrap().dump());
# Ok::<(), Box<dyn std::error::Error>>(())
```

`no_run`: it reads the real environment and the working directory.

### A plugin that installs hooks

`plugin_from_config` and `plugin_with_defaults` call `AppBuilder::plugin`,
and `build()` refuses a plugin that installs hooks when it comes that way.
For a `HookSource`, use `plugin_hooking_from_config::<P>(section)`. The
instance takes the section's name, so the section is claimed, and the
hook record grows by the plugin's families, as `plugin_hooking` does.

`plugin_hooking_from_config_with::<P>(section, adjust)` also applies
`adjust` to the section before it builds the plugin. Use it for a value
only code can give, such as the hash of a script the application inlines.
The file still decides every key that `adjust` leaves alone.

```rust
use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{
    BodyShape, HookRegistrar, HookSource, OnResponse, Request, RequestState, ResponseHead,
    ResponseHook,
};
use brocanth::plugin_order::Order;
use brocanth_config::{AppBuilderConfigExt, Config};

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct StampConfig {
    header: String,
    value: String,
}

struct Stamp(StampConfig);

impl Plugin for Stamp {
    type Config = StampConfig;
    type Provides = NoContext;
    const NAME: &'static str = "stamp";
    const INSTALLS_HOOKS: bool = true;
    fn new(config: StampConfig) -> Self {
        Self(config)
    }
}

impl HookSource for Stamp {
    type Families = OnResponse;
    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        registrar.response("stamp", Order::new(), Noop);
        Ok(())
    }
}

struct Noop;

impl ResponseHook for Noop {
    fn on_headers(
        &self,
        _request: &Request,
        _head: &mut ResponseHead,
        _body: BodyShape<'_>,
        _state: &mut RequestState,
    ) -> Result<(), PluginError> {
        Ok(())
    }
}

let config = Config::from_toml("my_app", "production", "[stamp]\nheader = \"x-app\"\n")?;
let app = AppBuilder::new()
    .with_config(config)
    // The file sets `header`; code adds `value`.
    .plugin_hooking_from_config_with::<Stamp>("stamp", |mut c| {
        c.value = env!("CARGO_PKG_VERSION").to_string();
        c
    })?
    .build()?;
assert_eq!(app.plugin_named::<Stamp>("stamp").unwrap().0.header, "x-app");
# Ok::<(), Box<dyn std::error::Error>>(())
```

`brocanth-middleware`'s CSRF, security headers and rate limit register
this way in a generated project. See
[docs/middleware.md](../../docs/middleware.md).

`cargo run -p brocanth-config --all-features --example dump` prints a full
dump.

## Testing

```text
cargo test -p brocanth-config
cargo test -p brocanth-config --all-features
```

No database and no environment variable is needed. No test changes the
process environment: each one injects `env_lookup`, `env_vars`, `root` and
`cwd`. `tests/feature_gating.rs` runs `cargo tree` and checks that a format
that is off links no parser. `tests/panel.rs` renders the **Settings** tab
through `brocanth-test` and checks that a password never reaches the page.

In an application, build a `Config` with `Config::from_toml` or an injected
`Loader`, as in the examples above. Then build the application from it
and test the screens with `brocanth_test::TestClient`.

## Links

- [docs/module-flags.md](../../docs/module-flags.md): settings, flags,
  precedence, `None` and `none_as`, and the settings report.
- [docs/modules.md](../../docs/modules.md): where the `config` module
  attaches.
- The crate docs (`cargo doc -p brocanth-config --open`): the source order,
  the environment rules (`env`), the formats (`formats`) and redaction
  (`redact`).
