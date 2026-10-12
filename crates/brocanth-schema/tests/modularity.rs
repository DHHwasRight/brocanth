//! §8, asserted rather than claimed.
//!
//! "A build of the core alone, with every feature off, that asserts the
//! dependency tree contains no TOML parser, no SQL driver, no logging
//! framework and no async runtime. Modularity claimed in a README rots;
//! modularity asserted in CI does not."
//!
//! The plugin's version of that test is this one: with every feature off,
//! `brocanth-schema` has no dependencies at all, so an OpenAPI emitter or a
//! form builder that only wants to read a `SchemaNode` links neither the
//! framework nor a regex engine. The manifest is read as text, because the
//! alternative is a dependency on a manifest parser, which would be a funny
//! way to fail this test.

use std::path::Path;

#[test]
fn every_dependency_is_optional() {
    let manifest = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .expect("this crate has a manifest");

    let dependencies = section(&manifest, "[dependencies]");
    assert!(
        !dependencies.is_empty(),
        "the manifest moved; this test is now checking nothing"
    );

    for line in dependencies {
        assert!(
            line.contains("optional = true"),
            "`{line}` is not optional, so an application that never validates \
             anything would still link it"
        );
    }
}

#[test]
fn there_are_no_dev_dependencies_either() {
    let manifest = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .expect("this crate has a manifest");
    assert!(
        !manifest.contains("[dev-dependencies]"),
        "a dev-dependency does not reach a consumer's binary, but it does appear in \
         `cargo tree`, and a claim that needs a footnote is not the claim we want"
    );
}

/// The non-empty, non-comment lines of one table, up to the next table.
fn section<'a>(manifest: &'a str, header: &str) -> Vec<&'a str> {
    manifest
        .lines()
        .skip_while(|line| line.trim() != header)
        .skip(1)
        .take_while(|line| !line.trim_start().starts_with('['))
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}
