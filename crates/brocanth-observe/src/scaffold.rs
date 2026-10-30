//! `observability` -- what `brocanth create` writes when somebody asks for
//! logs, probes, metrics or traces.
//!
//! One parent and four children, each a choice of its own:
//!
//! - `observability` publishes the `Observer` that `serve` wraps its handler
//!   with: a request id, and the SIGTERM path that fails `/readyz` first.
//! - `observability-logs`: a JSON or text line per request on stdout.
//! - `observability-health`: `/up` and `/readyz`.
//! - `observability-metrics`: Prometheus text on `/metrics`.
//! - `observability-tracing`: OTLP spans, off until asked for.
//!
//! A child is a cargo feature of this crate (`logs`, `metrics`, `otel`) or a
//! pair of paths, so a child that was not chosen is not compiled. The
//! parent writes the one plugin file, and the file says only what the
//! chosen children configure.
//!
//! What this module cannot write is the half in `src/app.rs`: `serve` has to
//! pass its handler through `Observer::wrap_routed` and spawn
//! `Observer::stop_on_signal`. No slot reaches inside `serve`, so that half
//! belongs to the generator. `docs/observability.md` lists the lines.
//!
//! Compiled only under the `scaffold` feature, which is off.

use brocanth_scaffold::snippet::{plugin, settings_derive, PLUGIN_TRAIT_USE};
use brocanth_scaffold::{Dep, Manifest, Module, Project, Scaffold, Slot};

const LOGS: &str = "observability-logs";
const HEALTH: &str = "observability-health";
const METRICS: &str = "observability-metrics";
const TRACING: &str = "observability-tracing";

/// The crate with no default features: each child turns on its own.
fn krate() -> Dep {
    Dep::brocanth("brocanth-observe").no_default_features()
}

/// The parent: an `Observer` around every request.
pub struct Observe;

impl Module for Observe {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "observability",
            "a request id on every request and a SIGTERM that fails /readyz first; logs, probes, metrics and traces below",
        )
        .asked_in("operations", "Observability: logs, health probes, metrics (traces off)")
        .krate(krate())
        .after("config")
        .absent(
            "brocanth-observe",
            "no request log, no /up or /readyz, no /metrics; a deploy cannot tell ready from alive",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/observability/mod.rs", plugin_rs(project));
        s.at(Slot::PluginsMod).line("pub mod observability;");
        plugin(s, project, "observability", "Observe");
        if project.has("config") {
            s.at(Slot::ConfigSections).block(config_section(project));
        } else {
            s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
        }
    }
}

/// A line per request on stdout.
pub struct Logs;

impl Module for Logs {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            LOGS,
            "a JSON line per request on stdout in production, text elsewhere; `tracing` and nothing else",
        )
        .asked_in("operations", "Logs on stdout")
        .part_of("observability")
        .because("the log layer is a feature of `brocanth-observe`, which the parent brings")
        .krate(krate().feature("logs"))
    }

    fn contribute(&self, _project: &Project, _s: &mut Scaffold) {
        // The parent writes the plugin; this choice is the feature and the
        // `format` and `level` settings in it.
    }
}

/// Liveness and readiness.
pub struct Health;

impl Module for Health {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            HEALTH,
            "/up for liveness, /readyz runs every plugin's check and fails first on SIGTERM",
        )
        .asked_in("operations", "Health probes")
        .part_of("observability")
        .because("the probes are routes the `Observer` answers, which the parent brings")
        .krate(krate())
    }

    fn contribute(&self, _project: &Project, _s: &mut Scaffold) {}
}

/// Prometheus text.
pub struct Metrics;

impl Module for Metrics {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            METRICS,
            "Prometheus text on /metrics: requests by route and status, in-flight, job queues",
        )
        .asked_in("operations", "Prometheus metrics")
        .part_of("observability")
        .because("the registry is a feature of `brocanth-observe`, which the parent brings")
        .krate(krate().feature("metrics"))
    }

    fn contribute(&self, _project: &Project, _s: &mut Scaffold) {}
}

/// OTLP traces.
pub struct Tracing;

impl Module for Tracing {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            TRACING,
            "an OTLP span per request and per job, W3C traceparent in and out; needs a collector",
        )
        .asked_in("operations", "Traces over OTLP (needs a collector)")
        .part_of_but_off("observability")
        .because("the exporter is a feature of `brocanth-observe`, which the parent brings")
        .krate(krate().feature("otel"))
    }

    fn contribute(&self, _project: &Project, _s: &mut Scaffold) {}
}

fn config_section(project: &Project) -> String {
    let mut out = String::from("\n[observability]\n");
    if project.has(LOGS) {
        out.push_str(
            "# \"json\" or \"text\". Unset: json when the environment is production.\n\
             # format = \"json\"\n\
             level = \"info\"\n",
        );
    }
    if project.has(HEALTH) {
        out.push_str(
            "live_path = \"/up\"\n\
             ready_path = \"/readyz\"\n\
             # Seconds to answer \"not ready\" after SIGTERM, before the listener stops.\n\
             drain_delay_secs = 0\n",
        );
    }
    if project.has(METRICS) {
        out.push_str(
            "metrics_path = \"/metrics\"\n\
             # metrics_token = \"a long random string\"\n\
             # metrics_bind = \"127.0.0.1:9100\"\n",
        );
    }
    if project.has(TRACING) {
        out.push_str(
            "# The collector. Unset: OTEL_EXPORTER_OTLP_ENDPOINT, then localhost:4318.\n\
             # otlp_endpoint = \"http://localhost:4318\"\n\
             sample_ratio = 1.0\n",
        );
    }
    out
}
fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "observability");
    let (logs, health, metrics, tracing) = (
        project.has(LOGS),
        project.has(HEALTH),
        project.has(METRICS),
        project.has(TRACING),
    );

    let mut chose = Vec::new();
    if logs {
        chose.push("a JSON or text line per request on stdout");
    }
    if health {
        chose.push("`/up` and `/readyz`");
    }
    if metrics {
        chose.push("Prometheus text on `/metrics`");
    }
    if tracing {
        chose.push("an OTLP span per request");
    }
    let chose = if chose.is_empty() {
        "a request id on every request, and nothing else yet".to_string()
    } else {
        chose.join(", ")
    };

    let mut fields = String::new();
    let mut defaults = String::new();
    let mut build = String::new();
    let mut imports = vec!["ObserveConfig", "ObservePlugin", "Observer"];
    if logs {
        imports.insert(0, "LogFormat");
        fields.push_str(
            "    /// `json` or `text`. Empty: `json` in production, `text` elsewhere.\n    pub format: String,\n    /// A level (`info`) or per-target levels (`info,sqlx=warn`). `RUST_LOG`\n    /// wins when it is set.\n    pub level: String,\n",
        );
        defaults.push_str(
            "            format: String::new(),\n            level: \"info\".to_string(),\n",
        );
        build.push_str(
            "    config.log_format = LogFormat::resolve(Some(&settings.format), &environment())?;\n    config.log_level = settings.level;\n",
        );
    }
    if health {
        fields.push_str(
            "    /// Liveness. Empty turns it off.\n    pub live_path: String,\n    /// Readiness: every plugin's check. Empty turns it off.\n    pub ready_path: String,\n    /// Seconds to answer \"not ready\" after SIGTERM before the listener\n    /// stops. A few seconds on Kubernetes; zero on a laptop.\n    pub drain_delay_secs: u64,\n",
        );
        defaults.push_str(
            "            live_path: \"/up\".to_string(),\n            ready_path: \"/readyz\".to_string(),\n            drain_delay_secs: 0,\n",
        );
        build.push_str(
            "    config.live_path = path(settings.live_path);\n    config.ready_path = path(settings.ready_path);\n    config.drain_delay = Duration::from_secs(settings.drain_delay_secs);\n",
        );
    } else {
        build.push_str("    // No `observability-health`: no probe routes.\n    config.live_path = None;\n    config.ready_path = None;\n");
    }
    if metrics {
        fields.push_str(
            "    /// Prometheus text. Empty turns it off.\n    pub metrics_path: String,\n    /// When set, `/metrics` needs `Authorization: Bearer <token>`.\n    pub metrics_token: String,\n    /// When set, `/metrics` is served on this address and not on the main\n    /// listener.\n    pub metrics_bind: String,\n",
        );
        defaults.push_str(
            "            metrics_path: \"/metrics\".to_string(),\n            metrics_token: String::new(),\n            metrics_bind: String::new(),\n",
        );
        build.push_str(
            "    config.metrics_path = path(settings.metrics_path);\n    config.metrics_token = path(settings.metrics_token);\n    config.metrics_bind = path(settings.metrics_bind);\n",
        );
    }
    if tracing {
        fields.push_str(
            "    /// The OTLP/HTTP collector. Empty: `OTEL_EXPORTER_OTLP_ENDPOINT`.\n    pub otlp_endpoint: String,\n    /// The share of new traces kept, from 0.0 to 1.0.\n    pub sample_ratio: f64,\n",
        );
        defaults.push_str(
            "            otlp_endpoint: String::new(),\n            sample_ratio: 1.0,\n",
        );
        build.push_str(
            "    config.tracing.enabled = true;\n    config.tracing.endpoint = path(settings.otlp_endpoint);\n    config.tracing.sample_ratio = settings.sample_ratio;\n",
        );
    }
    let uses_duration = if health {
        "use std::time::Duration;\n\n"
    } else {
        ""
    };
    let needs_path = health || metrics || tracing;
    let path_fn = if needs_path {
        "fn path(value: String) -> Option<String> {\n    (!value.trim().is_empty()).then_some(value)\n}\n\n"
    } else {
        ""
    };
    let env_fn = if logs {
        "/// `MY_APP_ENV`, the variable the config plugin reads too.\nfn environment() -> String {\n    let var = format!(\"{}_ENV\", crate::app::APP.to_uppercase().replace('-', \"_\"));\n    std::env::var(var).unwrap_or_else(|_| \"development\".to_string())\n}\n\n"
    } else {
        ""
    };
    let (settings_struct, observer_sig) = if fields.is_empty() {
        (
            format!("/// What this plugin is built from: nothing yet. Each child of\n/// `observability` adds its settings here.\n///\n/// {note}\n{derive}\npub struct Settings {{}}\n\nimpl Default for Settings {{\n    fn default() -> Self {{\n        Self {{}}\n    }}\n}}\n"),
            "fn observer(_settings: Settings) -> Result<Observer, String> {\n    #[allow(unused_mut)]\n    let mut config = ObserveConfig::new(crate::app::APP);\n",
        )
    } else {
        (
            format!("/// What this plugin is built from.\n///\n/// {note}\n{derive}\npub struct Settings {{\n{fields}}}\n\nimpl Default for Settings {{\n    fn default() -> Self {{\n        Self {{\n{defaults}        }}\n    }}\n}}\n"),
            "fn observer(settings: Settings) -> Result<Observer, String> {\n    let mut config = ObserveConfig::new(crate::app::APP);\n",
        )
    };
    let imports = imports.join(", ");
    format!(
        r##"//! Observability, as this application registers it: {chose}.
//!
//! The core assumes no log layer and no probes. This file is where the
//! application chose them. Delete the registration and this file, and none
//! of it is compiled. Each piece is its own module: `brocanth add
//! observability-metrics` and the like.
//!
//! `serve` in `src/app.rs` reads the `Observer` this plugin publishes, wraps
//! its handler with it, and stops on SIGTERM through it.

{uses_duration}use brocanth::plugin_api::{{Lifecycle, Plugin, PluginError}};
use brocanth::plugin_context::NoContext;
use brocanth::plugin_shutdown::ShutdownCx;
use brocanth_observe::{{{imports}}};

{settings_struct}
{env_fn}{path_fn}{observer_sig}{build}    Observer::new(config)
}}

pub struct Observe {{
    inner: Result<ObservePlugin, String>,
}}

impl Observe {{
    fn inner(&mut self) -> Result<&mut ObservePlugin, PluginError> {{
        self.inner
            .as_mut()
            .map_err(|e| PluginError::new(format!("[observability] {{e}}")))
    }}
}}

impl Plugin for Observe {{
    type Config = Settings;
    type Provides = NoContext;
    const NAME: &'static str = "observability";

    fn new(settings: Settings) -> Self {{
        // `Plugin::new` cannot fail. A bad setting fails `register` instead,
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
        let project = Project::new("my-blog", "My Blog", &["observability"], &["observability"]);
        let mut s = Scaffold::new();
        Observe.contribute(&project, &mut s);
        let files: Vec<&str> = s.files().map(|(path, _)| path).collect();
        assert_eq!(files, ["src/plugins/observability/mod.rs"]);
        assert!(s
            .slot(Slot::PluginsMod)
            .any(|l| l.contains("pub mod observability;")));
        assert!(s
            .slot(Slot::AppPlugins)
            .any(|l| l.contains("observability::Observe")));
    }
}
