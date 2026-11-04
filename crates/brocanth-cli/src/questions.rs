//! The chooser's questions without the chooser, and what a selection left
//! out.
//!
//! An agent that runs `brocanth create NAME --with ...` never sees the
//! chooser, so the decisions the chooser puts to a person were made in
//! silence. Two things here close that gap:
//!
//! * [`print`] writes every question, its modules, their defaults and their
//!   relations, as text or JSON (`create --questions`, `add --questions`).
//!   It reads the manifests and [`crate::wizard::QUESTIONS`], the same
//!   sources the chooser reads, so the two cannot disagree.
//! * [`wanted`] lists the modules a deployed project usually wants that the
//!   selection left out. It follows the `recommends` edges of the chosen
//!   manifests, so a module from another repository can add one without an
//!   edit here.

use crate::commands::create::{about, all, available};
use crate::ui::{Diagnostic, Ui};
use crate::wizard::{self, PRESETS, QUESTIONS};

/// What an agent must do with the questions. Printed at the top of both
/// formats, because the reader of `--json` is a program that reads text to
/// a model.
pub const RULE: &str = "These are decisions for the person. Present each question with its \
default and your recommendation, and wait for the answer, before you run \
`brocanth create --with` or `brocanth add`. Do not choose for them.";

/// One module a deployed project usually wants and the selection lacks.
#[derive(Debug, PartialEq, Eq)]
pub struct Wanted {
    /// The module that is absent.
    pub key: &'static str,
    /// Each chosen module that recommends it, with its reason.
    pub by: Vec<(&'static str, &'static str)>,
    /// Chosen modules it declares a conflict with, and would replace:
    /// `logging` for `observability-logs`.
    pub replaces: Vec<&'static str>,
}

impl Wanted {
    /// The keys `brocanth add` needs to bring this module in: its parent
    /// when that is absent too, then the module.
    pub fn to_add(&self, picks: &[&str]) -> Vec<&'static str> {
        let m = about(self.key);
        let mut out: Vec<&'static str> = m
            .parent
            .filter(|p| !picks.contains(p))
            .into_iter()
            .collect();
        out.push(self.key);
        out
    }

    /// The line the generated `Cargo.toml` says about this module's absence:
    /// its own, or its parent's when the parent is absent too.
    pub fn consequence(&self, picks: &[&str]) -> Option<&'static str> {
        let m = about(self.key);
        if let Some((_, why)) = m.absent {
            return Some(why);
        }
        let parent = m.parent.filter(|p| !picks.contains(p))?;
        about(parent).absent.map(|(_, why)| why)
    }

    /// A warning that names the module, who recommends it, what its absence
    /// costs, and the command that adds it.
    pub fn diagnostic(&self, code: &str, picks: &[&str]) -> Diagnostic {
        let names: Vec<String> = self.by.iter().map(|(by, _)| format!("`{by}`")).collect();
        let mut d = Diagnostic::warning(
            code.to_string(),
            format!(
                "`{}` is not chosen, and {} recommend{} it",
                self.key,
                names.join(", "),
                if self.by.len() == 1 { "s" } else { "" }
            ),
        );
        for (by, why) in &self.by {
            d = d.note(format!("{by}: {why}"));
        }
        if let Some(cost) = self.consequence(picks) {
            d = d.note(format!("without it: {cost}"));
        }
        if self.replaces.is_empty() {
            return d.help(format!(
                "ask the person; if they want it, run `brocanth add {}`",
                self.to_add(picks).join(" ")
            ));
        }
        // `add` cannot take a module out, so the way in is a new project.
        let replaced: Vec<String> = self.replaces.iter().map(|k| format!("`{k}`")).collect();
        let with: Vec<&str> = all()
            .into_iter()
            .filter(|k| {
                (picks.contains(k) && !self.replaces.contains(k)) || self.to_add(picks).contains(k)
            })
            .collect();
        d.note(format!(
            "it replaces {}, which is chosen",
            replaced.join(", ")
        ))
        .help(format!(
            "ask the person; if they want it, create the project with `--with {}`",
            with.join(",")
        ))
    }
}

/// The modules the chosen ones recommend and the selection lacks, in
/// listing order.
pub fn wanted(picks: &[&str]) -> Vec<Wanted> {
    let mut out: Vec<Wanted> = Vec::new();
    for key in available() {
        if picks.contains(&key) {
            continue;
        }
        let by: Vec<(&'static str, &'static str)> = picks
            .iter()
            .filter_map(|pick| all().into_iter().find(|k| k == pick))
            .flat_map(|pick| {
                about(pick)
                    .recommends
                    .into_iter()
                    .filter(move |(target, _)| *target == key)
                    .map(move |(_, why)| (pick, why))
            })
            .collect();
        if by.is_empty() {
            continue;
        }
        // A module the selection cannot take is not advice. A static site
        // has no server to log; that rule is not a choice. A declared
        // conflict is one: the target replaces the pick, and says so.
        let target = about(key);
        let mut replaces = Vec::new();
        let mut impossible = false;
        for pick in picks
            .iter()
            .filter_map(|p| all().into_iter().find(|k| k == p))
        {
            let other = about(pick);
            if brocanth_scaffold::clash(&target, &other).is_some() {
                if target.static_output || other.static_output {
                    impossible = true;
                } else {
                    replaces.push(pick);
                }
            }
        }
        if !impossible {
            out.push(Wanted { key, by, replaces });
        }
    }
    out
}

/// Print every warning [`wanted`] finds, and the pointer to the questions.
pub fn warn_wanted(ui: &Ui, picks: &[&str]) {
    let wanted = wanted(picks);
    if wanted.is_empty() {
        return;
    }
    ui.gap();
    for w in &wanted {
        ui.diagnostic(&w.diagnostic("recommended", picks));
    }
}

/// `--questions`: the chooser's questions, as text or as JSON.
///
/// `defaults` is what a run with no answer would take: `create`'s default
/// list, or the modules a project already has for `add`. `installed` is
/// `Some` inside a project.
pub fn print(ui: &Ui, json: bool, defaults: &[&str], installed: Option<&[&str]>) {
    let out = if json {
        to_json(defaults, installed)
    } else {
        to_text(defaults, installed)
    };
    ui.result(out.trim_end());
}

/// The questions in the order the guided route asks them, without the
/// shape question, which has presets rather than modules.
fn asked() -> Vec<&'static wizard::Question> {
    wizard::plan(None)
        .into_iter()
        .map(|at| &QUESTIONS[at])
        .filter(|q| q.key != "shape")
        .collect()
}

fn members(question: &str) -> Vec<&'static str> {
    all()
        .into_iter()
        .filter(|key| wizard::asks_about(question, about(key).section))
        .collect()
}

fn to_text(defaults: &[&str], installed: Option<&[&str]>) -> String {
    let mut out = format!("{RULE}\n\n");
    out.push_str("Shape: a starting set of ticks.\n");
    for preset in PRESETS {
        out.push_str(&format!(
            "  - {}: {} [{}]\n",
            preset.name,
            preset.blurb,
            preset.picks.join(", ")
        ));
    }
    let width = all().iter().map(|k| k.len()).max().unwrap_or(0) + 4;
    for q in asked() {
        out.push_str(&format!("\n{}: {}\n", q.title, q.blurb));
        for key in members(q.key) {
            let m = about(key);
            let tick = if installed.is_some_and(|i| i.contains(&key)) {
                "[installed]"
            } else if defaults.contains(&key) {
                "[x]"
            } else {
                "[ ]"
            };
            // A child is indented inside the key column, so the ticks
            // stay in one column.
            let shown = if m.parent.is_some() {
                format!("  {key}")
            } else {
                key.to_string()
            };
            out.push_str(&format!("  {tick:<11} {shown:<width$}{}\n", m.phrasing()));
            let pad = format!("  {:<11} {:<width$}", "", "");
            if m.phrasing() != m.gist {
                out.push_str(&format!("{pad}{}\n", m.gist));
            }
            if let Some(why) = m.blocked {
                out.push_str(&format!("{pad}not available: {why}\n"));
            }
            if let Some(parent) = m.parent {
                out.push_str(&format!("{pad}part of {parent}\n"));
            }
            for (k, why) in &m.needs {
                out.push_str(&format!("{pad}needs {k}: {why}\n"));
            }
            for (k, why) in &m.conflicts {
                out.push_str(&format!("{pad}conflicts with {k}: {why}\n"));
            }
            for (k, why) in &m.recommends {
                out.push_str(&format!("{pad}recommends {k}: {why}\n"));
            }
        }
    }
    out
}

fn to_json(defaults: &[&str], installed: Option<&[&str]>) -> String {
    let pairs = |list: &[(&str, &str)]| {
        let items: Vec<String> = list
            .iter()
            .map(|(k, why)| format!("{{\"key\":{},\"why\":{}}}", s(k), s(why)))
            .collect();
        format!("[{}]", items.join(","))
    };
    let keys = |list: &[&str]| {
        let items: Vec<String> = list.iter().map(|k| s(k)).collect();
        format!("[{}]", items.join(","))
    };
    let presets: Vec<String> = PRESETS
        .iter()
        .map(|p| {
            format!(
                "{{\"name\":{},\"blurb\":{},\"picks\":{}}}",
                s(p.name),
                s(p.blurb),
                keys(p.picks)
            )
        })
        .collect();
    let questions: Vec<String> = asked()
        .into_iter()
        .map(|q| {
            let modules: Vec<String> = members(q.key)
                .into_iter()
                .map(|key| {
                    let m = about(key);
                    let opt = |v: Option<&str>| v.map_or_else(|| "null".to_string(), s);
                    let mut fields = vec![
                        format!("\"key\":{}", s(key)),
                        format!("\"label\":{}", s(m.phrasing())),
                        format!("\"gist\":{}", s(m.gist)),
                        format!("\"default\":{}", defaults.contains(&key)),
                    ];
                    if let Some(installed) = installed {
                        fields.push(format!("\"installed\":{}", installed.contains(&key)));
                    }
                    fields.extend([
                        format!("\"parent\":{}", opt(m.parent)),
                        format!("\"on_with_parent\":{}", m.on_with_parent),
                        format!("\"blocked\":{}", opt(m.blocked)),
                        format!("\"absent\":{}", opt(m.absent.map(|(_, why)| why))),
                        format!("\"needs\":{}", pairs(&m.needs)),
                        format!("\"conflicts\":{}", pairs(&m.conflicts)),
                        format!("\"recommends\":{}", pairs(&m.recommends)),
                    ]);
                    format!("{{{}}}", fields.join(","))
                })
                .collect();
            format!(
                "{{\"key\":{},\"title\":{},\"blurb\":{},\"modules\":[\n    {}\n  ]}}",
                s(q.key),
                s(q.title),
                s(q.blurb),
                modules.join(",\n    ")
            )
        })
        .collect();
    format!(
        "{{\n\"rule\":{},\n\"defaults\":{},\n\"presets\":[\n  {}\n],\n\"questions\":[\n  {}\n]\n}}\n",
        s(RULE),
        keys(defaults),
        presets.join(",\n  "),
        questions.join(",\n  ")
    )
}

/// A JSON string. Written out rather than a dependency: the CLI has no
/// serde, and this is the only JSON it writes.
fn s(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_recommendation_names_a_module_that_exists_and_says_why() {
        for key in all() {
            for (target, why) in about(key).recommends {
                assert!(
                    crate::modules::find(target).is_some(),
                    "`{key}` recommends `{target}`, which no module declares"
                );
                assert_ne!(target, key, "`{key}` recommends itself");
                assert!(
                    !why.is_empty(),
                    "`{key}` recommends `{target}` without a reason"
                );
                assert!(
                    !about(key).needs.iter().any(|(n, _)| *n == target),
                    "`{key}` needs `{target}` already; a recommendation adds nothing"
                );
            }
        }
    }

    #[test]
    fn a_selection_without_observability_hears_what_its_modules_recommend() {
        // The run that prompted this: a real service, no observability.
        let picks = ["config", "db", "middleware", "commands"];
        let wanted = wanted(&picks);
        let keys: Vec<&str> = wanted.iter().map(|w| w.key).collect();
        assert_eq!(keys, ["observability-logs", "observability-health"]);
        let health = &wanted[1];
        assert_eq!(health.by[0].0, "db");
        assert_eq!(
            health.to_add(&picks),
            ["observability", "observability-health"]
        );
        // The parent is absent too, so the receipt's line about it is the
        // consequence.
        assert!(health.consequence(&picks).unwrap().contains("/readyz"));
        let said = health.diagnostic("recommended", &picks).render();
        assert!(said.contains("ask the person"), "{said}");
        assert!(
            said.contains("brocanth add observability observability-health"),
            "{said}"
        );
    }

    #[test]
    fn a_static_site_is_not_told_to_log_a_server_it_does_not_have() {
        assert!(wanted(&["static", "config", "commands"]).is_empty());
    }

    #[test]
    fn the_production_log_says_it_replaces_the_development_line() {
        let picks = ["logging", "config", "commands"];
        let wanted = wanted(&picks);
        assert_eq!(wanted[0].key, "observability-logs");
        assert_eq!(wanted[0].replaces, ["logging"]);
        let said = wanted[0].diagnostic("recommended", &picks).render();
        assert!(said.contains("it replaces `logging`"), "{said}");
        assert!(
            said.contains("--with observability,observability-logs,config,commands"),
            "{said}"
        );
    }

    #[test]
    fn the_default_selection_wants_nothing_more() {
        let defaults = crate::commands::create::DEFAULT;
        assert!(wanted(&defaults).is_empty(), "{:?}", wanted(&defaults));
    }

    #[test]
    fn docker_recommends_the_production_log_and_the_probe() {
        let keys: Vec<&str> = wanted(&["docker"]).iter().map(|w| w.key).collect();
        assert_eq!(keys, ["observability-logs", "observability-health"]);
    }

    #[test]
    fn the_questions_list_every_module_once_with_its_edges() {
        let text = to_text(&crate::commands::create::DEFAULT, None);
        assert!(text.starts_with(RULE), "{text}");
        for key in all() {
            let row = format!(" {key} ");
            assert!(text.contains(&row), "`{key}` is not asked about:\n{text}");
        }
        assert!(text.contains("recommends observability-health"), "{text}");
        assert!(text.contains("[x]         observability "), "{text}");
    }

    #[test]
    fn the_json_is_one_object_per_module_with_the_defaults_marked() {
        let json = to_json(&crate::commands::create::DEFAULT, Some(&["config"]));
        assert!(json.starts_with("{\n\"rule\":"), "{json}");
        assert!(json.contains("{\"key\":\"docker\","), "{json}");
        assert!(json.contains("\"key\":\"config\",\"label\":"), "{json}");
        assert!(json.contains("\"installed\":true"), "{json}");
        assert!(
            json.contains("\"recommends\":[{\"key\":\"observability-logs\""),
            "{json}"
        );
    }

    #[test]
    fn a_json_string_escapes_what_json_requires() {
        assert_eq!(s("a \"b\" \\ c\n"), "\"a \\\"b\\\" \\\\ c\\n\"");
    }
}
