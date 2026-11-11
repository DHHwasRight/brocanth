//! The generator behind `brocanth create`, as a library, for one caller: the
//! tests of a module crate.
//!
//! The binary is `src/main.rs`. This target compiles the same modules a
//! second time and exposes only [`testing`]. A module crate takes it as a
//! dev-dependency and generates a project with its own module in it,
//! without a fork of the CLI. `brocanth_scaffold::testing` holds the checks
//! to run on the result.
//!
//! The unit tests stay in the binary target (`cargo test -p brocanth-cli
//! --bin brocanth`), so this target does not run them a second time.

// The binary uses every command; this target uses the generator alone.
#![allow(dead_code, unused_imports)]

mod choices;
mod commands;
mod modules;
mod owned;
mod questions;
mod tui;
mod ui;
mod wizard;

/// Generate a project from module keys, with modules the CLI does not have.
pub mod testing {
    use brocanth_scaffold::Module;

    /// Every module this CLI was built with, in the order the chooser lists
    /// them. For a check that must cover each module, so a new one cannot
    /// be left out.
    pub fn modules() -> &'static [&'static dyn Module] {
        crate::modules::ALL
    }

    /// The files `brocanth create <name> --with <picks>` writes, with each
    /// module in `extra` beside the modules this CLI was built with.
    ///
    /// A module in `extra` replaces the CLI's module with the same key, so
    /// a module crate tests the module it compiles. `picks` names every
    /// module of the project, `extra` ones included. The brocanth crates
    /// come from the repository, as `create` writes them, so the result is
    /// the same on every machine and fits a golden file.
    ///
    /// # Errors
    ///
    /// Every diagnostic `create` prints for these picks, one per line, each
    /// with its `help:` line.
    pub fn generate(
        name: &str,
        picks: &[&'static str],
        extra: &[&'static dyn Module],
    ) -> Result<Vec<(String, String)>, String> {
        generated(name, picks, extra, None)
    }

    /// [`generate`], with the brocanth crates as path dependencies on the
    /// checkout this CLI was compiled from, as `--brocanth-path` writes
    /// them. For `brocanth_scaffold::testing::build`: the result builds
    /// against the code under test. It holds an absolute path, so it does
    /// not fit a golden file.
    ///
    /// # Errors
    ///
    /// As [`generate`], and when the checkout cannot be found.
    pub fn generate_for_build(
        name: &str,
        picks: &[&'static str],
        extra: &[&'static dyn Module],
    ) -> Result<Vec<(String, String)>, String> {
        let checkout = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            // `--brocanth-path` names the core crate; its siblings follow.
            .join("../brocanth")
            .canonicalize()
            .map_err(|e| {
                format!(
                    "error: cannot find the brocanth checkout: {e}\n  \
                     help: build against a checkout of brocanth, not a published crate"
                )
            })?;
        generated(name, picks, extra, Some(checkout.display().to_string()))
    }

    fn generated(
        name: &str,
        picks: &[&'static str],
        extra: &[&'static dyn Module],
        local: Option<String>,
    ) -> Result<Vec<(String, String)>, String> {
        // The generator takes a key it does not know for a bug in itself;
        // here it is a typo in a test.
        let unknown: Vec<&str> = crate::modules::with_extra(extra, || {
            let known = crate::modules::keys();
            picks
                .iter()
                .copied()
                .filter(|key| !known.contains(key))
                .collect()
        });
        if !unknown.is_empty() {
            return Err(format!(
                "error[module]: no module has the key {}\n  \
                 help: pass the module under test in `extra`, or check the spelling",
                unknown
                    .iter()
                    .map(|k| format!("`{k}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let answers = crate::commands::create::Answers {
            name: name.to_string(),
            title: crate::ui::title_case(name),
            picks: picks.to_vec(),
            local,
            git: false,
        };
        crate::modules::with_extra(extra, || crate::commands::create::scaffold(&answers)).map_err(
            |wrong| {
                wrong
                    .iter()
                    .map(|d| {
                        let mut out = format!(
                            "error[{}]: {}",
                            d.code.as_deref().unwrap_or("create"),
                            d.message
                        );
                        for (kind, text) in &d.notes {
                            out.push_str(&format!("\n  {kind}: {text}"));
                        }
                        out
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            },
        )
    }
}
