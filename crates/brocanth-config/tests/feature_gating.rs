//! The test that keeps the format split honest.
//!
//! PLAN.md §8.2: *"Feature flags gate compilation, not behaviour. A feature
//! that is off must mean the code is absent, not that a branch returns early.
//! If turning a feature off leaves its dependencies in `Cargo.lock`, the split
//! is wrong."*
//!
//! A README can claim that; only a test keeps it true. This asks `cargo tree`
//! what the crate actually resolves to under each feature set and fails if a
//! parser is reachable that nothing asked for. `brocanth`'s own
//! `tests/no_hidden_dependencies.rs` does the same job one level down, for the
//! core; this one is for the formats.
//!
//! It is the reason the value model in `src/value.rs` is the crate's own
//! rather than `toml::Value`: with the model built on a format's types, this
//! test could not pass.

use std::process::Command;

/// Every crate a format's parser brings, including the satellites, so that
/// pulling one in through a transitive edge is caught too.
const TOML_PARSER: &[&str] = &["toml", "toml_parser", "winnow", "serde_spanned"];
const JSON_PARSER: &[&str] = &["jsonc-parser"];
const YAML_PARSER: &[&str] = &["saphyr", "saphyr-parser"];

/// The packages reachable from `brocanth-config` with these features.
///
/// Normal dependencies only: a dev-dependency never reaches anyone's binary.
fn tree(features: &[&str], default: bool) -> Vec<String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let mut command = Command::new(cargo);
    command
        .args(["tree", "-p", "brocanth-config", "-e", "normal"])
        .args(["--prefix", "none", "--no-dedupe"]);
    if !default {
        command.arg("--no-default-features");
    }
    if !features.is_empty() {
        command.args(["--features", &features.join(",")]);
    }

    let out = command.output().expect("`cargo tree` did not run");
    assert!(
        out.status.success(),
        "`cargo tree` failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

fn assert_absent(features: &[&str], default: bool, banned: &[&str], why: &str) {
    let tree = tree(features, default);
    let found: Vec<&&str> = banned
        .iter()
        .filter(|b| tree.contains(&(**b).to_string()))
        .collect();
    assert!(
        found.is_empty(),
        "{why}\nthese are still in the tree: {found:?}\n\
         A feature that is off must leave its parser out of the build (PLAN.md §8.2). \
         Check that nothing outside `src/formats/{{format}}.rs` names that crate, and that \
         the value model in `src/value.rs` has not grown a dependency on one."
    );
}

fn assert_present(features: &[&str], default: bool, wanted: &str) {
    let tree = tree(features, default);
    assert!(
        tree.contains(&wanted.to_string()),
        "turning the feature on must put `{wanted}` in the tree, and it did not; \
         a feature that changes nothing is worse than no feature"
    );
}

#[test]
fn with_every_format_off_no_parser_is_reachable() {
    assert_absent(
        &[],
        false,
        &[TOML_PARSER, JSON_PARSER, YAML_PARSER].concat(),
        "a build with no format enabled links no parser at all",
    );
}

#[test]
fn a_service_that_reads_only_yaml_links_no_toml_parser() {
    // The §8.5 sentence, as a test: "a service configured entirely from its
    // environment should not link a TOML parser".
    assert_absent(
        &["yaml"],
        false,
        &[TOML_PARSER, JSON_PARSER].concat(),
        "the `yaml` feature must not drag in the other two parsers",
    );
    assert_present(&["yaml"], false, "saphyr");
}

#[test]
fn a_service_that_reads_only_json_links_no_toml_parser() {
    assert_absent(
        &["json"],
        false,
        &[TOML_PARSER, YAML_PARSER].concat(),
        "the `json` feature must not drag in the other two parsers",
    );
    assert_present(&["json"], false, "jsonc-parser");
}

#[test]
fn the_default_build_is_toml_and_only_toml() {
    assert_absent(
        &[],
        true,
        &[JSON_PARSER, YAML_PARSER].concat(),
        "the default feature set is TOML; the other two must be opt-in",
    );
    assert_present(&[], true, "toml");
}

#[test]
fn turning_every_format_on_brings_every_parser() {
    // The negative tests above all pass if the features are wired to nothing.
    // This is the half that stops that being a way to pass.
    for parser in [TOML_PARSER, JSON_PARSER, YAML_PARSER].concat() {
        if parser == "winnow" || parser == "toml_parser" || parser == "saphyr-parser" {
            continue; // transitive, and not guaranteed to be a direct edge
        }
        assert_present(&["toml", "json", "yaml"], false, parser);
    }
}
