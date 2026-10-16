//! `commands` -- this project's own command line.

use brocanth_scaffold::{Dep, Manifest, Module, Project, Scaffold};

/// A `manage` binary whose subcommands are the project's own structs.
///
/// Also what makes a directory a project: it writes `brocanth.toml`, the
/// marker the global binary walks up to find.
pub struct Commands;

impl Module for Commands {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "commands",
            "this project's own command line, `serve` plus whatever you add",
        )
        .krate(Dep::brocanth("brocanth-command"))
    }

    fn contribute(&self, _project: &Project, _scaffold: &mut Scaffold) {
        // Still written by `create.rs`. Moving here next.
    }
}
