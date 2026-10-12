//! `brocanth generate` -- the scaffold generator's command surface, ahead of
//! the generator.
//!
//! §4 step 8 calls the scaffold generator "the capstone and also the
//! specification: if scaffolding produces code you would not write by hand, the
//! framework below it is wrong." It is therefore the last thing built, not the
//! first, and it belongs to the agent who owns the model layer -- the input is
//! SeaORM's entity registry (§7.2).
//!
//! What ships now is the surface and the plan: the kinds, the names they
//! resolve to, and the exact files each one would write. **Nothing is written
//! and nothing is faked.** A command that printed a convincing success and
//! created no files would be worse than this one.

use std::path::Path;
use std::process::ExitCode;

use clap::{Args, ValueEnum};

use crate::commands::Outcome;
use crate::project::Project;
use crate::ui::{Cell, Diagnostic, Table, Ui, ACCENT, DIM, STRONG};

#[derive(Args)]
pub struct GenerateArgs {
    /// What to generate
    #[arg(value_name = "KIND", value_enum)]
    pub kind: Kind,
    /// What to call it
    #[arg(value_name = "NAME")]
    pub name: String,
    /// Where the pages live, relative to the package root
    #[arg(long, value_name = "DIR")]
    pub pages: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Kind {
    /// One route: a file under the pages directory
    Page,
    /// A plugin crate skeleton with the five lifecycle hooks
    Plugin,
    /// A schema type, its validations and a migration to read before applying
    Model,
    /// A model, its seven routes, its forms and its tests
    Scaffold,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Page => "page",
            Kind::Plugin => "plugin",
            Kind::Model => "model",
            Kind::Scaffold => "scaffold",
        }
    }

    /// Where in §4 the generator behind this kind is scheduled.
    fn step(self) -> &'static str {
        match self {
            Kind::Page => "\u{a7}4 step 1, the typed route table",
            Kind::Plugin => "\u{a7}8, the plugin contract",
            Kind::Model => "\u{a7}4 step 3, schema types and migration generation",
            Kind::Scaffold => "\u{a7}4 step 8, the capstone",
        }
    }
}

pub fn run(ui: &Ui, dir: Option<&Path>, args: GenerateArgs) -> Outcome {
    let project = Project::discover(dir, args.pages.as_deref())?;
    let name = slug(&args.name);
    if name.is_empty() {
        return Err(vec![Diagnostic::error(
            "name",
            format!("{:?} leaves nothing to name a file after", args.name),
        )
        .help("lowercase letters, digits, - and _")]);
    }

    ui.heading(
        "generate",
        format!("{} \u{b7} {}", args.kind.as_str(), name),
    );
    ui.note(format!(
        "  {}",
        crate::ui::paint(STRONG, "Not built yet. When it is, this command will write:")
    ));
    ui.gap();

    let mut table = Table::new().indent(4);
    for (path, what) in plan(&project, args.kind, &name) {
        table.row(vec![Cell::styled(path, ACCENT), Cell::styled(what, DIM)]);
    }
    ui.table_note(&table);

    ui.gap();
    ui.note(format!(
        "  {}",
        crate::ui::paint(
            DIM,
            format!(
                "The generator is {}. Until then nothing here is written.",
                args.kind.step()
            )
        )
    ));
    ui.gap();
    ui.warn("`brocanth generate` is a surface, not a generator; no file was created");
    // Not a failure: the command did exactly what it says it does. A non-zero
    // exit would break a script that merely probes the surface.
    Ok(ExitCode::SUCCESS)
}

/// The files each kind would write. Written down here because the list is the
/// specification the generator will be held to, and because a reader deciding
/// whether to wait for it needs to see the shape.
fn plan(project: &Project, kind: Kind, name: &str) -> Vec<(String, &'static str)> {
    let pages = &project.pages_rel;
    match kind {
        Kind::Page => vec![
            (
                format!("{pages}/{name}.rs"),
                "the route: one `handle` per verb it answers",
            ),
            (
                format!("{pages}/mod.rs"),
                "edited: the `mod` line that makes the route reachable",
            ),
        ],
        Kind::Plugin => vec![
            (
                format!("src/plugins/{name}.rs"),
                "the five lifecycle hooks, a typed config section, nothing autowired",
            ),
            (
                "src/plugins/mod.rs".to_string(),
                "edited: the `mod` line",
            ),
            (
                "src/main.rs".to_string(),
                "edited: the `.plugin(..)` call, because a plugin is named in main or it is absent",
            ),
        ],
        Kind::Model => vec![
            (
                format!("src/models/{name}.rs"),
                "the schema type, its relations and its declarative validations",
            ),
            (
                format!("migrations/<timestamp>_create_{name}.rs"),
                "proposed, never applied: read it, edit it, commit it",
            ),
            (
                format!("tests/{name}_test.rs"),
                "a transaction per test, parallel by default",
            ),
        ],
        Kind::Scaffold => vec![
            (
                format!("src/models/{name}.rs"),
                "the schema type and its validations",
            ),
            (
                format!("{pages}/{name}/index.rs"),
                "index and create",
            ),
            (
                format!("{pages}/{name}/[id].rs"),
                "show, update and destroy",
            ),
            (
                format!("{pages}/{name}/new.rs"),
                "the form, built from the schema type",
            ),
            (
                format!("{pages}/{name}/[id]/edit.rs"),
                "the form again, with values and errors preserved on 422",
            ),
            (
                format!("migrations/<timestamp>_create_{name}.rs"),
                "proposed, never applied",
            ),
            (
                format!("tests/{name}_test.rs"),
                "one test per route, in-process",
            ),
        ],
    }
}

/// `BlogPost` and `blog posts` both name `blog_post`. A file name, so the
/// rules are a file name's.
fn slug(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 4);
    let chars: Vec<char> = raw.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c.is_ascii_uppercase() {
            let previous_is_lower = i > 0 && chars[i - 1].is_ascii_lowercase();
            if previous_is_lower && !out.ends_with('_') {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_matches('_').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_becomes_a_file_name_whatever_it_arrived_as() {
        assert_eq!(slug("BlogPost"), "blog_post");
        assert_eq!(slug("blog posts"), "blog_posts");
        assert_eq!(slug("blog-post"), "blog-post");
        assert_eq!(slug("  "), "");
    }
}
