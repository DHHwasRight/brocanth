//! The `observability` modules, written into a project by the CLI's
//! generator.
//!
//! Each shape is generated, checked against rustfmt, and compared with
//! `tests/golden/<name>.txt`. Bless with `BROCANTH_BLESS=1`; build and test
//! each project too with `BROCANTH_BUILD=1`.

use brocanth_observe::scaffold::{Health, Logs, Metrics, Observe, Tracing};
use brocanth_scaffold::{testing, Module};

const MODULES: &[&dyn Module] = &[&Observe, &Logs, &Health, &Metrics, &Tracing];

fn shape(name: &str, picks: &[&'static str]) {
    let files = brocanth_cli::testing::generate("my-blog", picks, MODULES)
        .unwrap_or_else(|e| panic!("{e}"));
    testing::assert_formatted(&files);
    testing::golden(
        format!("{}/tests/golden/{name}.txt", env!("CARGO_MANIFEST_DIR")),
        &testing::snapshot(&files),
    );
    if testing::building() {
        let files = brocanth_cli::testing::generate_for_build("my-blog", picks, MODULES)
            .unwrap_or_else(|e| panic!("{e}"));
        testing::build(&files).unwrap_or_else(|e| panic!("{e}"));
    }
}

/// Every child, the traces included, with a config file: one
/// `[observability]` section, a side listener for `/metrics`, and the
/// exporter feature.
#[test]
fn every_child_with_a_config_file() {
    shape(
        "observability-all",
        &[
            "config",
            "observability",
            "observability-logs",
            "observability-health",
            "observability-metrics",
            "observability-tracing",
        ],
    );
}

/// The probes and the metrics with no config file: the settings come from
/// flags and the environment.
#[test]
fn probes_and_metrics_without_a_config_file() {
    shape(
        "observability-flags",
        &[
            "observability",
            "observability-health",
            "observability-metrics",
        ],
    );
}

/// The parent alone: a request id and a drain on SIGTERM, and a settings
/// struct with no field.
#[test]
fn the_parent_alone() {
    shape("observability-bare", &["observability"]);
}
