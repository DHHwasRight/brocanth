//! `brocanth explain` -- why the project is made the way it is.
//!
//! The conditions report as a command (`docs/module-capabilities.md` §7):
//! why each module is in, the provider of each Kind, each contribution,
//! each hook with its phase, and each setting with its layers. It reads
//! `brocanth.toml` and the manifests this CLI was built with. It builds
//! nothing and connects to nothing; it runs the generator in memory for
//! the settings. `--json` prints the same facts as one JSON document.

use std::path::Path;
use std::process::ExitCode;

use clap::Args;

use crate::commands::project::Project;
use crate::commands::{add, create, Outcome};
use crate::ui::{Diagnostic, Ui};

#[derive(Args)]
pub struct ExplainArgs {
    /// Print the same facts as one JSON document, for tools and agents
    ///
    /// Stable field names, documented in the CLI README, and a `"version"`
    /// field: 1. The text report is rendered from the same document.
    #[arg(long)]
    pub json: bool,
}

pub fn run(ui: &Ui, dir: Option<&Path>, args: ExplainArgs) -> Outcome {
    let project = Project::find(dir)?;
    let marker = project.root.join(super::delegate::MARKER);
    let text = std::fs::read_to_string(&marker).map_err(|e| add::read_error(&marker, &e))?;
    let facts = explained(&text)?;
    if args.json {
        ui.result(crate::explain::json(&facts));
    } else {
        ui.result(crate::explain::text(&facts).trim_end());
    }
    Ok(ExitCode::SUCCESS)
}

/// The facts for the project `brocanth.toml` describes: the one source of
/// the text report and of `--json`.
fn explained(text: &str) -> Result<crate::explain::Explained, Vec<Diagnostic>> {
    let record = add::read_record(text)?;
    let recorded = recorded_bindings(text)?;
    let answers = create::Answers {
        name: record.name.clone(),
        title: record.title.clone(),
        picks: record.enabled.clone(),
        local: None,
        git: false,
        named: record.named.clone(),
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
    Ok(crate::explain::facts_named(
        &record.name,
        &record.enabled,
        &record.named,
        recorded.as_ref(),
        &settings,
    ))
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
        let out = crate::explain::text(&crate::explain::facts("my-app", &picks, None, &[]));
        assert!(
            out.contains("provides PgPool: jobs requires PgPool"),
            "{out}"
        );
        assert!(out.contains("JobQueue"), "{out}");
        assert!(out.contains("read by mail"), "{out}");
        assert!(out.contains("no [bindings] table"), "{out}");
        assert!(out.contains("ConfigCheck"), "{out}");
    }

    /// The modules of the fixed project the JSON golden is taken from: a
    /// Kind added for another module, a part, a need, contributions with
    /// and without a toggle, a mount, required hooks and a secret.
    const FIXED: &[&str] = &[
        "admin",
        "cache",
        "commands",
        "config",
        "db",
        "jobs",
        "mail",
        "middleware",
        "middleware-csrf",
        "observability",
        "policy",
        "rls",
    ];

    /// `brocanth.toml` as `brocanth create` writes it for `picks`.
    fn record_of(picks: &[&str]) -> String {
        let picks: Vec<&'static str> = crate::commands::create::all()
            .into_iter()
            .filter(|k| picks.contains(k))
            .collect();
        let answers = create::Answers {
            name: "my-app".into(),
            title: "My App".into(),
            picks,
            local: None,
            git: false,
            named: Vec::new(),
        };
        create::scaffold(&answers)
            .expect("the modules agree")
            .into_iter()
            .find(|(path, _)| path == super::super::delegate::MARKER)
            .map(|(_, text)| text)
            .expect("create writes brocanth.toml")
    }

    /// A named store is a line of its own, and the binding in `[bindings]`
    /// says it is the default. The JSON has a `named` list only then.
    #[test]
    fn a_named_store_is_explained_beside_the_default() {
        let words: Vec<String> = ["config", "storage", "storage-fs", "storage-s3@public"]
            .iter()
            .map(|w| w.to_string())
            .collect();
        let (picks, named) = crate::resolve::split_words(&words).expect("words");
        let answers = create::Answers {
            name: "my-app".into(),
            title: "My App".into(),
            picks,
            local: None,
            git: false,
            named,
        };
        let record = create::scaffold(&answers)
            .expect("the modules agree")
            .into_iter()
            .find(|(path, _)| path == super::super::delegate::MARKER)
            .map(|(_, text)| text)
            .expect("create writes brocanth.toml");
        let facts = explained(&record).expect("it explains");
        let out = crate::explain::text(&facts);
        assert!(
            out.contains("ObjectStore    <- storage-fs (default; storage requires ObjectStore)"),
            "{out}"
        );
        assert!(
            out.contains("ObjectStore[public] <- storage-s3 (named)"),
            "{out}"
        );
        assert!(out.contains("provides ObjectStore[public]: named"), "{out}");
        let json = crate::explain::json(&facts);
        assert!(json.contains("\"named\""), "{json}");
        assert!(
            !crate::explain::json(&explained(&record_of(FIXED)).unwrap()).contains("\"named\"")
        );
    }

    /// `brocanth explain --json` for the fixed project prints what it
    /// printed yesterday. The name holds `generator_writes`, so the CLI's
    /// bless (`BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth --
    /// generator_writes`) rewrites it with the generator's goldens.
    #[test]
    fn explain_json_generator_writes_what_it_wrote_yesterday() {
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/explain.json");
        let have = crate::explain::json(&explained(&record_of(FIXED)).expect("it explains")) + "\n";
        if std::env::var_os("BROCANTH_BLESS").is_some() {
            std::fs::write(&file, &have).expect("write the golden");
            return;
        }
        let want = std::fs::read_to_string(&file).unwrap_or_default();
        assert!(
            want == have,
            "`brocanth explain --json` changed for {}.\nRun with BROCANTH_BLESS=1 and read the diff.",
            file.display()
        );
    }

    /// Every fact the text prints is in the JSON: the text is rendered from
    /// the document read back from its JSON, and is the same text. `text`
    /// takes the document and nothing else, so it has no other source.
    #[test]
    fn every_fact_in_the_text_is_in_the_json() {
        let full = record_of(FIXED);
        // No `[bindings]` table, and a table that misses a binding: the two
        // `!` lines.
        let bare = full
            .split("\n[bindings]")
            .next()
            .expect("a first part")
            .to_string()
            + &full[full.find("\n[modules.").expect("module tables")..];
        let partial = full.replace("JobQueue = { provider = \"jobs\", why = \"chosen\" }\n", "");
        assert_ne!(partial, full, "the fixture binds JobQueue");
        for (record, warns) in [(full, false), (bare, true), (partial, true)] {
            let facts = explained(&record).expect("it explains");
            assert_eq!(crate::explain::text(&facts).contains("\n  ! "), warns);
            let json = crate::explain::json(&facts);
            let back: crate::explain::Explained =
                serde_json::from_str(&json).expect("the JSON reads back");
            assert_eq!(back, facts);
            assert_eq!(crate::explain::text(&back), crate::explain::text(&facts));
            let value: serde_json::Value = serde_json::from_str(&json).expect("JSON");
            assert_eq!(value["version"], 1);
        }
    }
}
