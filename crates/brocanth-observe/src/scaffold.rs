//! `observe` -- what `brocanth create` writes when somebody asks for logs,
//! probes and metrics.
//!
//! Here rather than in the CLI because what it writes is a statement about
//! this crate: that the plugin is built from an `Observer`, that the format
//! defaults by environment, that `/metrics` can move to its own listener.
//!
//! What this module cannot write is the half in `src/app.rs`: `serve` has to
//! pass its handler through `Observer::wrap_routed` and spawn
//! `Observer::stop_on_signal`. No slot reaches inside `serve`, so that half
//! belongs to the generator. `docs/observability.md` lists the lines.
//!
//! Compiled only under the `scaffold` feature, which is off.

use brocanth_scaffold::snippet::{plugin, settings_derive, PLUGIN_TRAIT_USE};
use brocanth_scaffold::{Dep, Manifest, Module, Project, Scaffold, Slot};

/// Logs, probes and metrics, as a choice on the `brocanth create` menu.
pub struct Observe;

impl Module for Observe {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "observe",
            "JSON logs on stdout, /up and /readyz, Prometheus /metrics; the core assumes none",
        )
        .asked_in("foundations", "Logs, health probes and metrics")
        .krate(Dep::brocanth("brocanth-observe"))
        .after("config")
        .absent(
            "brocanth-observe",
            "no request log, no /up or /readyz, no /metrics; a deploy cannot tell ready from alive",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/observe/mod.rs", plugin_rs(project));
        s.at(Slot::PluginsMod).line("pub mod observe;");
        plugin(s, project, "observe", "Observe");
        if project.has("config") {
            s.at(Slot::ConfigSections).block(CONFIG_SECTION.to_string());
        } else {
            s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
        }
    }
}

const CONFIG_SECTION: &str = r#"
[observe]
# "json" or "text". Unset: json when the environment is production.
# format = "json"
level = "info"
live_path = "/up"
ready_path = "/readyz"
metrics_path = "/metrics"
# metrics_token = "a long random string"
# metrics_bind = "127.0.0.1:9100"
# Seconds to answer "not ready" after SIGTERM, before the listener stops.
drain_delay_secs = 0
"#;

fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "observe");
    format!(
        r##"//! Logs, probes and metrics, as this application registers them.
//!
//! The core assumes no log layer. This file is where the application chose
//! one: a JSON or text line per request on stdout, `/up` and `/readyz`, and
//! Prometheus text on `/metrics`. Delete the registration and this file, and
//! none of it is compiled.
//!
//! `serve` in `src/app.rs` reads the `Observer` this plugin publishes, wraps
//! its handler with it, and stops on SIGTERM through it.

use std::time::Duration;

use brocanth::plugin_api::{{Lifecycle, Plugin, PluginError}};
use brocanth::plugin_context::NoContext;
use brocanth::plugin_shutdown::ShutdownCx;
use brocanth_observe::{{LogFormat, ObserveConfig, ObservePlugin, Observer}};

/// What this plugin is built from.
///
/// {note}
{derive}
pub struct Settings {{
    /// `json` or `text`. Empty: `json` in production, `text` elsewhere.
    pub format: String,
    /// A level (`info`) or per-target levels (`info,sqlx=warn`). `RUST_LOG`
    /// wins when it is set.
    pub level: String,
    /// Liveness. Empty turns it off.
    pub live_path: String,
    /// Readiness: every plugin's check. Empty turns it off.
    pub ready_path: String,
    /// Prometheus text. Empty turns it off.
    pub metrics_path: String,
    /// When set, `/metrics` needs `Authorization: Bearer <token>`.
    pub metrics_token: String,
    /// When set, `/metrics` is served on this address and not on the main
    /// listener.
    pub metrics_bind: String,
    /// Seconds to answer "not ready" after SIGTERM before the listener
    /// stops. A few seconds on Kubernetes; zero on a laptop.
    pub drain_delay_secs: u64,
}}

impl Default for Settings {{
    fn default() -> Self {{
        Self {{
            format: String::new(),
            level: "info".to_string(),
            live_path: "/up".to_string(),
            ready_path: "/readyz".to_string(),
            metrics_path: "/metrics".to_string(),
            metrics_token: String::new(),
            metrics_bind: String::new(),
            drain_delay_secs: 0,
        }}
    }}
}}

/// `MY_APP_ENV`, the variable the config plugin reads too.
fn environment() -> String {{
    let var = format!("{{}}_ENV", crate::app::APP.to_uppercase().replace('-', "_"));
    std::env::var(var).unwrap_or_else(|_| "development".to_string())
}}

fn path(value: String) -> Option<String> {{
    (!value.trim().is_empty()).then_some(value)
}}

fn observer(settings: Settings) -> Result<Observer, String> {{
    let mut config = ObserveConfig::new(crate::app::APP);
    config.log_format = LogFormat::resolve(Some(&settings.format), &environment())?;
    config.log_level = settings.level;
    config.live_path = path(settings.live_path);
    config.ready_path = path(settings.ready_path);
    config.metrics_path = path(settings.metrics_path);
    config.metrics_token = path(settings.metrics_token);
    config.metrics_bind = path(settings.metrics_bind);
    config.drain_delay = Duration::from_secs(settings.drain_delay_secs);
    Observer::new(config)
}}

pub struct Observe {{
    inner: Result<ObservePlugin, String>,
}}

impl Observe {{
    fn inner(&mut self) -> Result<&mut ObservePlugin, PluginError> {{
        self.inner
            .as_mut()
            .map_err(|e| PluginError::new(format!("[observe] {{e}}")))
    }}
}}

impl Plugin for Observe {{
    type Config = Settings;
    type Provides = NoContext;
    const NAME: &'static str = "observe";

    fn new(settings: Settings) -> Self {{
        // `Plugin::new` cannot fail. A bad `format` fails `register` instead,
        // with the line to write.
        Self {{
            inner: observer(settings).map(ObservePlugin::new),
        }}
    }}

    fn register(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        self.inner()?.register(lc)
    }}

    fn after_config(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        self.inner()?.after_config(lc)
    }}

    fn before_start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        self.inner()?.before_start(lc)
    }}

    fn shutdown(&mut self, sd: &mut ShutdownCx<'_>) -> Result<(), PluginError> {{
        self.inner()?.shutdown(sd)
    }}
}}
"##
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_writes_the_plugin_and_registers_it() {
        let project = Project::new("my-blog", "My Blog", &["observe"], &["observe"]);
        let mut s = Scaffold::new();
        Observe.contribute(&project, &mut s);
        let files: Vec<&str> = s.files().map(|(path, _)| path).collect();
        assert_eq!(files, ["src/plugins/observe/mod.rs"]);
        assert!(s
            .slot(Slot::PluginsMod)
            .any(|l| l.contains("pub mod observe;")));
        assert!(s
            .slot(Slot::AppPlugins)
            .any(|l| l.contains("observe::Observe")));
    }
}
