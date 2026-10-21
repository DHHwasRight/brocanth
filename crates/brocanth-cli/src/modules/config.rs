//! `config` -- TOML configuration, four sources, merged.

use brocanth_scaffold::{Dep, Manifest, Module, Project, Scaffold};

/// The config plugin.
///
/// The one choice that changes the *source* of every other choice rather
/// than adding to it: with it a plugin's settings derive `Deserialize` and
/// `main` reads a file; without it the struct derives nothing, `main` writes
/// a literal, and no parser is linked.
pub struct Config;

impl Module for Config {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "config",
            "config/<env>.toml, merged; without it nothing links a TOML parser",
        )
        .asked_in(
            "foundations",
            "Settings in config/<env>.toml, layered per environment",
        )
        .krate(Dep::brocanth("brocanth-config"))
        .absent(
            "brocanth-config",
            "no TOML parser; main builds each plugin from a literal",
        )
    }

    fn contribute(&self, _project: &Project, _scaffold: &mut Scaffold) {
        // Still written by `create.rs`. Moving here next.
    }
}
