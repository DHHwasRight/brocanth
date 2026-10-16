//! `model` -- SeaORM, validations and migrations.

use brocanth_scaffold::{Dep, Manifest, Module, Project, Scaffold};

/// The model layer.
///
/// Recommended and absent, which is section 8 point 4 word for word: an
/// application that stores nothing has no SQL driver in its lock file.
pub struct Model;

impl Module for Model {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "model",
            "SeaORM, validations, migrations; without it no SQL driver at all",
        )
        .krate(Dep::brocanth("brocanth-model"))
        .absent(
            "brocanth-model",
            "no ORM, no SQL driver, no sea-orm anywhere in the lock file",
        )
    }

    fn contribute(&self, _project: &Project, _scaffold: &mut Scaffold) {
        // Still written by `create.rs`. Moving here next.
    }
}
