//! Prints the effective configuration of a small application, the way an
//! application should on `--print-config` or on a startup flag.
//!
//! `cargo run -p brocanth-config --all-features --example dump`

use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_context::NoContext;
use brocanth_config::{effective_of, AppBuilderConfigExt, ConfiguredPlugin, Loader};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct Pool {
    max_size: u32,
    timeout_s: u32,
}

#[derive(Deserialize, Serialize)]
struct ServerConfig {
    host: String,
    port: u16,
    workers: u32,
    pool: Pool,
}

struct ServerPlugin;

impl Plugin for ServerPlugin {
    type Config = ServerConfig;
    type Provides = NoContext;
    const NAME: &'static str = "server";
    fn new(_c: ServerConfig) -> Self {
        Self
    }
}

impl ConfiguredPlugin for ServerPlugin {
    fn defaults() -> ServerConfig {
        ServerConfig {
            host: "0.0.0.0".into(),
            port: 3000,
            workers: 4,
            pool: Pool {
                max_size: 5,
                timeout_s: 30,
            },
        }
    }
}

#[derive(Deserialize, Serialize)]
struct LoggerConfig {
    level: String,
    json: bool,
}

struct LoggerPlugin;

impl Plugin for LoggerPlugin {
    type Config = LoggerConfig;
    type Provides = NoContext;
    const NAME: &'static str = "logger";
    fn new(_c: LoggerConfig) -> Self {
        Self
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join("brocanth-config-example");
    std::fs::create_dir_all(dir.join("config"))?;
    std::fs::write(
        dir.join("config/production.yaml"),
        "server:\n  port: 3100\n  workers: 16\nlogger:\n  level: warn\n",
    )?;
    std::fs::write(
        dir.join("my_app_production.toml"),
        "[logger]\nlevel = \"error\"\n",
    )?;

    let config = Loader::new("my_app")
        .env("production")
        .root(&dir)
        .cwd(&dir)
        .env_lookup(|_| None)
        .env_vars(|| {
            vec![
                ("MY_APP_SERVER__PORT".into(), "8080".into()),
                ("MY_APP_SERVER__POOL__MAX_SIZE".into(), "50".into()),
            ]
        })
        .load()?;

    let app = AppBuilder::new()
        .with_config(config)
        .plugin_with_defaults::<ServerPlugin>("server")?
        .plugin_as_with_defaults::<LoggerPlugin>("logger")?
        .build()?;

    print!("{}", effective_of(app.services()).unwrap().dump());
    Ok(())
}
