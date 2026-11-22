//! Bundles the Postgres archive into the binary, on request.
//!
//! With the feature `embedded` and `BROCANTH_POSTGRES_BUNDLE` set to a
//! `postgresql-<version>-<target>.tar.gz` file, this checks the file
//! against the SHA-256 pinned in `src/embedded_pins.rs` and lets
//! `src/embedded.rs` embed it (`include_bytes!`). A wrong file fails the
//! build. Without the variable it does nothing, and a build needs no
//! archive and no network.

use std::path::PathBuf;

use sha2::{Digest, Sha256};

include!("src/embedded_pins.rs");

/// The variable that names the archive to bundle, read at build time.
const BUNDLE_ENV: &str = "BROCANTH_POSTGRES_BUNDLE";

fn main() {
    println!("cargo::rustc-check-cfg=cfg(brocanth_bundled)");
    println!("cargo::rerun-if-env-changed={BUNDLE_ENV}");
    if std::env::var_os("CARGO_FEATURE_EMBEDDED").is_none() {
        return;
    }
    let Some(file) = std::env::var_os(BUNDLE_ENV).filter(|v| !v.is_empty()) else {
        return;
    };
    let file = std::path::absolute(PathBuf::from(file)).expect("an absolute path");
    println!("cargo::rerun-if-changed={}", file.display());
    let target = std::env::var("TARGET").expect("cargo sets TARGET");
    let name = format!("postgresql-{POSTGRES_VERSION}-{target}.tar.gz");
    let bytes = std::fs::read(&file).unwrap_or_else(|e| {
        panic!(
            "{BUNDLE_ENV}={}: {e}\nhelp: set {BUNDLE_ENV} to the path of {name}, or unset it",
            file.display()
        )
    });
    let Some(pinned) = pinned_sha256(&target) else {
        panic!(
            "{BUNDLE_ENV}: Postgres {POSTGRES_VERSION} has no archive for {target}\n\
             help: unset {BUNDLE_ENV}; this target cannot bundle the server"
        );
    };
    let found: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if found != pinned {
        panic!(
            "{BUNDLE_ENV}={}: its SHA-256 is {found}, and brocanth-db pins {pinned} for {name}\n\
             help: the file is not the release archive; download {name} again from \
             https://github.com/theseus-rs/postgresql-binaries/releases/tag/{POSTGRES_VERSION}",
            file.display()
        );
    }
    println!("cargo::rustc-cfg=brocanth_bundled");
    println!(
        "cargo::rustc-env=BROCANTH_POSTGRES_BUNDLED_FILE={}",
        file.display()
    );
}
