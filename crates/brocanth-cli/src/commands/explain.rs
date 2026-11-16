//! `brocanth explain` -- why the project is made the way it is.
//!
//! The conditions report as a command (`docs/module-capabilities.md` §7):
//! why each module is in, the provider of each Kind, each contribution,
//! each hook with its phase, and each setting with its layers. It reads
//! `brocanth.toml` and the manifests this CLI was built with. It builds
//! nothing and connects to nothing; it runs the generator in memory for
//! the settings.

use std::path::Path;
use std::process::ExitCode;

use clap::Args;

use crate::commands::project::Project;
use crate::commands::{add, create, Outcome};
use crate::ui::{Diagnostic, Ui};

#[derive(Args)]
pub struct ExplainArgs {}

pub fn run(ui: &Ui, dir: Option<&Path>, _args: ExplainArgs) -> Outcome {
    let project = Project::find(dir)?;
    let marker = project.root.join(super::delegate::MARKER);
    let text = std::fs::read_to_string(&marker).map_err(|e| add::read_error(&marker, &e))?;
    let record = add::read_record(&text)?;
    let recorded = recorded_bindings(&text)?;
    let answers = create::Answers {
        name: record.name.clone(),
        title: record.title.clone(),
        picks: record.enabled.clone(),
        local: None,
        git: false,
    };
    let built = create::contributions(&answers).map_err(|mut wrong| {
        wrong.push(
            Diagnostic::error(
                "explain",
                "the modules in brocanth.toml do not generate with this brocanth",
            )
            .help("run `brocanth check` for the cause, or install the brocanth that made this project"),
        );
        wrong
    })?;
    let settings: Vec<(String, crate::explain::Declared)> = built
        .settings()
        .map(|(by, section, setting)| {
            (
                by.to_string(),
                crate::explain::Declared {
                    section: section.to_string(),
                    setting: setting.clone(),
                },
            )
        })
        .collect();
    ui.result(
        crate::explain::report(&record.name, &record.enabled, recorded.as_ref(), &settings)
            .trim_end(),
    );
    Ok(ExitCode::SUCCESS)
}

/// `[bindings]` in `brocanth.toml`, or `None` for a file from before it.
fn recorded_bindings(text: &str) -> Result<Option<crate::explain::Recorded>, Vec<Diagnostic>> {
    let doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| add::toml_error("brocanth.toml", &e))?;
    let Some(table) = doc.get("bindings").and_then(|b| b.as_table_like()) else {
        return Ok(None);
    };
    let mut out = Vec::new();
    for (kind, item) in table.iter() {
        let provider = item.get("provider").and_then(|p| p.as_str());
        let why = item.get("why").and_then(|w| w.as_str()).unwrap_or("chosen");
        let Some(provider) = provider else {
            return Err(vec![Diagnostic::error(
                "brocanth.toml",
                format!("`[bindings] {kind}` has no `provider`"),
            )
            .at("brocanth.toml")
            .help(
                "run `brocanth add` or `brocanth remove` once: each rewrites `[bindings]`",
            )]);
        };
        out.push((kind.to_string(), provider.to_string(), why.to_string()));
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bindings_are_read_from_the_record() {
        let text = "[bindings]\nPgPool = { provider = \"db\", why = \"jobs requires PgPool\" }\n";
        let got = recorded_bindings(text).unwrap().unwrap();
        assert_eq!(
            got,
            vec![(
                "PgPool".to_string(),
                "db".to_string(),
                "jobs requires PgPool".to_string()
            )]
        );
        assert!(recorded_bindings("[project]\nname = \"x\"\n")
            .unwrap()
            .is_none());
        let wrong = recorded_bindings("[bindings]\nPgPool = { why = \"x\" }\n").unwrap_err();
        assert!(wrong[0].render().contains("help:"));
    }

    #[test]
    fn the_report_names_why_each_module_is_in_and_each_binding() {
        let picks: Vec<&'static str> = vec!["config", "db", "jobs", "mail"];
        let picks: Vec<&'static str> = crate::commands::create::all()
            .into_iter()
            .filter(|k| picks.contains(k))
            .collect();
        let out = crate::explain::report("my-app", &picks, None, &[]);
        assert!(
            out.contains("provides PgPool: jobs requires PgPool"),
            "{out}"
        );
        assert!(out.contains("JobQueue"), "{out}");
        assert!(out.contains("read by mail"), "{out}");
        assert!(out.contains("no [bindings] table"), "{out}");
        assert!(out.contains("ConfigCheck"), "{out}");
    }
}
