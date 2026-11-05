//! The CLI links no module's runtime.
//!
//! Each capability crate keeps its scaffold module beside the code it
//! scaffolds, and puts that code behind a `runtime` feature the CLI turns
//! off. A dependency line in `crates/brocanth-cli/Cargo.toml` that forgets
//! `default-features = false`, or a crate that adds a runtime dependency
//! outside its `runtime` feature, brings a driver, a TLS stack or an async
//! runtime back into a tool that writes text. Nothing else notices: the CLI
//! still builds, only slower and three times larger. This test does.
//!
//! It asks cargo for the CLI's own graph (`cargo tree -p brocanth-cli -e
//! normal`), which resolves features for the CLI alone, not unified with the
//! rest of the workspace.

use std::process::Command;

/// Crates that belong to a running application and never to the CLI.
///
/// A name matches exactly, or as a prefix when it ends in `*`.
const RUNTIME: &[&str] = &[
    "sqlx*",
    "sea-query*",
    "sea-orm*",
    "reqwest",
    "hyper*",
    "h2",
    "rustls*",
    "aws-lc*",
    "ring",
    "tokio*",
    "rsx*",
    "argon2",
    "jsonwebtoken",
    "chrono",
    "tracing*",
    "prometheus-client",
    "opentelemetry*",
    "zod",
    "brocanth-server",
    "brocanth-queue",
];

fn is_runtime(name: &str) -> bool {
    RUNTIME
        .iter()
        .any(|pattern| match pattern.strip_suffix('*') {
            Some(prefix) => name.starts_with(prefix),
            None => name == *pattern,
        })
}

#[test]
fn the_cli_links_no_runtime_crate() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let out = Command::new(cargo)
        .args([
            "tree",
            "--locked",
            "-p",
            "brocanth-cli",
            "-e",
            "normal",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run cargo tree");
    assert!(
        out.status.success(),
        "cargo tree failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let tree = String::from_utf8_lossy(&out.stdout);
    let mut found: Vec<&str> = tree
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| is_runtime(name))
        .collect();
    found.sort_unstable();
    found.dedup();
    assert!(
        found.is_empty(),
        "the CLI links runtime crates: {found:?}\n\
         help: run `cargo tree -p brocanth-cli -e normal -i <crate>` to see which \
         dependency brings it, then give that crate's dependency line in \
         crates/brocanth-cli/Cargo.toml `default-features = false`, or move the \
         dependency behind the crate's `runtime` feature"
    );
}

#[test]
fn the_list_matches_what_it_should() {
    assert!(is_runtime("sqlx-postgres"));
    assert!(is_runtime("tokio"));
    assert!(is_runtime("rsx-macros"));
    assert!(!is_runtime("ratatui"));
    assert!(!is_runtime("ring-buffer"));
    assert!(!is_runtime("brocanth-scaffold"));
}
