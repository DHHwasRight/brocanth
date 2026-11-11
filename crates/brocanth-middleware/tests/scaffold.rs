//! The `middleware` modules, written into a project by the CLI's generator.
//!
//! Each shape is generated, checked against rustfmt, and compared with
//! `tests/golden/<name>.txt`. Bless with `BROCANTH_BLESS=1`; build and test
//! each project too with `BROCANTH_BUILD=1`.

use brocanth_middleware::scaffold::{
    Compression, ConditionalGet, Csrf, Flash, Middleware, RateLimit, SecurityHeaders,
};
use brocanth_scaffold::{testing, Module};

const MODULES: &[&dyn Module] = &[
    &Middleware,
    &Csrf,
    &SecurityHeaders,
    &RateLimit,
    &ConditionalGet,
    &Compression,
    &Flash,
];

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

/// Every middleware module, with a config file: one section per plugin,
/// and the order the hooks run in.
#[test]
fn every_middleware_module_with_a_config_file() {
    shape(
        "middleware-all",
        &[
            "config",
            "middleware",
            "middleware-csrf",
            "middleware-security-headers",
            "middleware-rate-limit",
            "middleware-conditional-get",
            "middleware-compression",
            "flash",
        ],
    );
}

/// The hooking plugins with no config file: the literal branch of each
/// registration, and the settings from flags and the environment.
#[test]
fn the_hooking_plugins_without_a_config_file() {
    shape(
        "middleware-flags",
        &[
            "middleware",
            "middleware-csrf",
            "middleware-rate-limit",
            "middleware-conditional-get",
            "middleware-compression",
        ],
    );
}
