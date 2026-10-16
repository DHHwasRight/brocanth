//! `sessions` -- a cookie, and a `Session` on the context.

use brocanth_scaffold::{Manifest, Module, Project, Scaffold};

/// Cookie sessions.
///
/// Written into the project like the logger, and for the same reason: it is
/// short enough to read, and a session policy is a thing applications
/// disagree about.
pub struct Sessions;

impl Module for Sessions {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "sessions",
            "a cookie, and a `Session` the context hands a handler",
        )
    }

    fn contribute(&self, _project: &Project, _scaffold: &mut Scaffold) {
        // Still written by `create.rs`. Moving here next.
    }
}
