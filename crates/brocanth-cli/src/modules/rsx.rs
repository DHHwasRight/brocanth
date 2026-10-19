//! `rsx`, and the two choices that are part of it.
//!
//! Three modules in one file, because they are one subject: templates, the
//! CSS build that reads the class names out of them, and the navigation that
//! skips the layouts they define. Each is a separate entry on the menu, and
//! each can be left out.

use brocanth_scaffold::{Dep, Manifest, Module, Project, Scaffold};

/// rsx lives in its own repository, so `--brocanth-path` does not reach it.
/// That asymmetry is on purpose: brocanth is the thing being developed here
/// and wants a path, rsx is an ordinary dependency and is fetched.
pub const REPOSITORY: &str = "https://github.com/DHHwasRight/rsx.git";

/// HTML from `.rsx` templates, compiled into the binary.
pub struct Rsx;

impl Module for Rsx {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "rsx",
            "`.rsx` templates compiled into the binary; without it a page builds a String",
        )
        .krate(Dep::git("rsx", REPOSITORY, "main").note("templates, compiled"))
        .absent(
            "rsx",
            "no template engine; a page builds the String it returns",
        )
    }

    fn contribute(&self, _project: &Project, _scaffold: &mut Scaffold) {
        // Still written by `create.rs`. Moving here next.
    }
}

/// Tailwind, built from the class names in the templates.
pub struct Tailwind;

impl Module for Tailwind {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "rsx-tailwind",
            "a CSS build over the class names in your templates; needs node",
        )
        .part_of("rsx")
        .because("the CSS build reads class names out of `.rsx` files")
        .needs(
            "commands",
            "the CSS build is `brocanth css`, a subcommand, and a project \
             with no command line has nowhere to put it",
        )
    }

    fn contribute(&self, _project: &Project, _scaffold: &mut Scaffold) {
        // Still written by `create.rs`. Moving here next.
    }
}

/// Partial navigation: dp-swap, a `<Link>`, and layouts the server can skip.
pub struct Htmx;

impl Module for Htmx {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "rsx-htmx",
            "a click swaps what changed; the server renders from the shared layout down",
        )
        .part_of("rsx")
        .because(
            "`<Link>` is an `.rsx` component, and the partial it asks for is a layout \
             the templates define",
        )
    }

    fn contribute(&self, _project: &Project, _scaffold: &mut Scaffold) {
        // Still written by `create.rs`. Moving here next.
    }
}
