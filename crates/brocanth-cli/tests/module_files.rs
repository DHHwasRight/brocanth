//! `brocanth create` refuses a module that writes where it may not: the
//! scaffold's own brief, a path outside the project, or a slot its
//! categories do not open. Each refusal says what to do next.

use brocanth_scaffold::{Category, Manifest, Module, Project, Scaffold, Slot};

struct Writes {
    key: &'static str,
    category: Category,
    write: fn(&mut Scaffold),
}

impl Module for Writes {
    fn manifest(&self) -> Manifest {
        Manifest::new(self.key, "a module that writes one thing").category(self.category)
    }

    fn contribute(&self, _: &Project, scaffold: &mut Scaffold) {
        (self.write)(scaffold);
    }
}

static AGENTS: Writes = Writes {
    key: "writes-agents",
    category: Category::Enhancer,
    write: |s| s.file("AGENTS.md", "mine\n"),
};

static OUTSIDE: Writes = Writes {
    key: "writes-outside",
    category: Category::Enhancer,
    write: |s| s.file("../outside.rs", "\n"),
};

static CLOSED: Writes = Writes {
    key: "writes-closed",
    category: Category::External,
    write: |s| {
        s.at(Slot::PagesRenderers).line("x");
    },
};

fn refused(module: &'static Writes) -> String {
    brocanth_cli::testing::generate("my-blog", &[module.key], &[module])
        .err()
        .unwrap_or_else(|| panic!("`{}` was not refused", module.key))
}

#[test]
fn a_module_that_writes_agents_md_is_refused() {
    let error = refused(&AGENTS);
    assert!(error.contains("AGENTS.md"), "{error}");
    assert!(error.contains("help:"), "{error}");
}

#[test]
fn a_module_that_writes_outside_the_project_is_refused() {
    let error = refused(&OUTSIDE);
    assert!(error.contains("../outside.rs"), "{error}");
    assert!(error.contains("help:"), "{error}");
}

#[test]
fn a_module_that_writes_a_closed_slot_is_refused() {
    let error = refused(&CLOSED);
    assert!(error.contains("writes-closed"), "{error}");
    assert!(error.contains("help:"), "{error}");
}
