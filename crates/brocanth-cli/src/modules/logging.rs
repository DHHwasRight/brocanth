//! `logging` -- a line per request.

use brocanth_scaffold::{Manifest, Module, Project, Scaffold};

/// A request logger, written out into the project rather than linked.
///
/// It has no crate of its own on purpose: the whole of it is a plugin with
/// two hooks, and a reader who can see it can change it. The core assumes no
/// log layer at all, which is the thing this choice makes visible.
pub struct Logging;

impl Module for Logging {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "logging",
            "a line per request; the core assumes no log layer",
        )
    }

    fn contribute(&self, _project: &Project, _scaffold: &mut Scaffold) {
        // Still written by `create.rs`. Moving here next.
    }
}
