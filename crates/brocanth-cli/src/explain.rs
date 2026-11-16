//! What a manifest says, as text: the table on each `docs/modules/*.md`
//! page, and the report `brocanth explain` prints for a project.
//!
//! One source for both. The page shows a module alone; the report shows a
//! project's modules together, with the bindings `brocanth.toml` records.
//! Neither builds nor connects: both read the manifests and run each
//! module's `contribute` in memory for its settings
//! (`docs/module-capabilities.md` §12, stage 6).

use brocanth_scaffold::kind::Cardinality;
use brocanth_scaffold::{Kind, Setting};

use crate::commands::create::about;

/// A declared setting, as the page and the report show it.
#[derive(Debug, Clone)]
pub struct Declared {
    /// The section in the file and the first part of the flag: `jobs`.
    pub section: String,
    /// The setting.
    pub setting: Setting,
}

/// How a hook may be turned off, in one word.
fn hook_state(hook: &brocanth_scaffold::Hook) -> &'static str {
    if hook.required {
        "required"
    } else if hook.on {
        "on by default"
    } else {
        "off by default"
    }
}

/// The type of a setting, in one word.
fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Text => "text",
        Kind::Bool => "bool",
        Kind::Integer => "integer",
        Kind::Float => "float",
        Kind::List => "list",
    }
}

/// What `brocanth.toml` records in `[bindings]`: Kind, provider, why.
pub type Recorded = Vec<(String, String, String)>;

/// Why `key` is in a project of `picks`, given the bindings.
fn why_in(key: &str, picks: &[&'static str], bindings: &Recorded) -> Vec<String> {
    let m = about(key);
    let mut out = Vec::new();
    if let Some(parent) = m.parent.filter(|p| picks.contains(p)) {
        out.push(format!("part of {parent}"));
    }
    for other in picks {
        if let Some((_, why)) = about(other).needs.iter().find(|(n, _)| *n == key) {
            out.push(format!("{other} needs it: {why}"));
        }
    }
    for (kind, provider, why) in bindings {
        if provider == key && why != "chosen" {
            out.push(format!("provides {kind}: {why}"));
        }
    }
    if out.is_empty() {
        out.push("chosen".to_string());
    }
    out
}

/// The `brocanth explain` report for a project of `picks`. `recorded` is
/// `[bindings]` from `brocanth.toml`, `None` when the file has no table.
/// `settings` is every setting the generator declares for these picks.
pub fn report(
    name: &str,
    picks: &[&'static str],
    recorded: Option<&Recorded>,
    settings: &[(String, Declared)],
) -> String {
    let computed: Recorded = crate::resolve::bindings(picks)
        .into_iter()
        .map(|(kind, provider)| {
            (
                kind.to_string(),
                provider.to_string(),
                crate::resolve::why(picks, kind),
            )
        })
        .collect();
    let bindings = recorded.unwrap_or(&computed);
    let ident = name.replace('-', "_").to_uppercase();
    let width = picks.iter().map(|k| k.len()).max().unwrap_or(0);
    let mut out = String::new();
    let mut section = |title: &str, lines: Vec<String>| {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(title);
        out.push('\n');
        if lines.is_empty() {
            out.push_str("  none\n");
        }
        for line in lines {
            out.push_str("  ");
            out.push_str(&line);
            out.push('\n');
        }
    };

    section(
        &format!("Modules: why each is in ({} modules)", picks.len()),
        picks
            .iter()
            .map(|key| format!("{key:width$}  {}", why_in(key, picks, bindings).join("; ")))
            .collect(),
    );

    // Each Kind of cardinality one that a module binds, requires or uses.
    let mut kinds: Vec<brocanth_scaffold::KindRef> = Vec::new();
    for key in picks {
        let m = about(key);
        let named = m
            .provides
            .iter()
            .map(|(k, _)| *k)
            .chain(m.requires.iter().map(|r| r.kind))
            .chain(m.uses.iter().map(|(k, _)| *k));
        for k in named {
            if k.cardinality == Cardinality::One && !kinds.iter().any(|seen| seen.name == k.name) {
                kinds.push(k);
            }
        }
    }
    let mut lines = Vec::new();
    for k in &kinds {
        let users: Vec<&str> = picks
            .iter()
            .copied()
            .filter(|p| {
                let m = about(p);
                m.requirements(picks).any(|r| r.kind.name == k.name)
                    || m.uses.iter().any(|(u, _)| u.name == k.name)
            })
            .collect();
        let read_by = if users.is_empty() {
            String::new()
        } else {
            format!("; read by {}", users.join(", "))
        };
        match bindings.iter().find(|(kind, _, _)| kind == k.name) {
            Some((_, provider, why)) => {
                lines.push(format!("{:14} <- {provider} ({why}){read_by}", k.name))
            }
            None => lines.push(format!(
                "{:14} <- none: each reader writes its code without {}{read_by}",
                k.name, k.gist
            )),
        }
    }
    if let Some(recorded) = recorded {
        for (kind, provider, _) in &computed {
            if !recorded.iter().any(|(k, p, _)| k == kind && p == provider) {
                lines.push(format!(
                    "! brocanth.toml does not record {kind} <- {provider}; run `brocanth add` or `brocanth remove` once to rewrite it"
                ));
            }
        }
    } else {
        lines.push(
            "! brocanth.toml has no [bindings] table; the bindings above are computed from the modules"
                .to_string(),
        );
    }
    section("Kinds: the provider of each", lines);

    let collectors = crate::resolve::collectors(picks);
    let mut lines = Vec::new();
    for key in picks {
        let m = about(key);
        for (k, c, on) in &m.contributions {
            let to = match (
                k.collected_by,
                collectors.iter().find(|(n, _)| *n == k.name),
            ) {
                (Some(by), _) => format!("collected by {by}"),
                (None, Some((_, by))) => {
                    format!("collected by {by}; integrate.{} = {on} by default", k.group)
                }
                (None, None) => "not written: no module collects it".to_string(),
            };
            lines.push(format!(
                "{:13} <- {key}: `{}` ({to}): {}",
                k.name, c.name, c.what
            ));
        }
        for o in m.options.iter().filter(|o| o.mount) {
            lines.push(format!(
                "{:13} <- {key}: {} = {} ({})",
                "Mount",
                o.key,
                o.default,
                o.role.name()
            ));
        }
    }
    lines.sort_by(|a, b| a.split(" <- ").next().cmp(&b.split(" <- ").next()));
    section(
        "Contributions: what each module adds, and who collects it",
        lines,
    );

    let mut lines = Vec::new();
    for key in picks {
        for (hook, what, h) in &about(key).hooks {
            let phase = h.phase.map_or("-".to_string(), |p| p.to_string());
            lines.push(format!(
                "{key:width$}  {hook:16} {phase:8} {:14} {what}",
                hook_state(h)
            ));
        }
    }
    section(
        "Hooks: phase, and whether a project may turn each off",
        lines,
    );

    let file = picks.iter().any(|p| {
        about(p).gives(<brocanth_scaffold::ConfigSource as brocanth_scaffold::kind::Kind>::NAME)
    });
    let mut lines = Vec::new();
    for (by, d) in settings {
        let s = &d.setting;
        let env = format!(
            "${ident}_{}__{}",
            d.section.replace('-', "_").to_uppercase(),
            s.key.replace('.', "__").to_uppercase()
        );
        let flag = if s.secret {
            format!("--{0}.{1}_file, --{0}.{1}_env", d.section, s.key)
        } else if s.flag {
            format!("--{}.{}", d.section, s.key)
        } else {
            "no flag".to_string()
        };
        let mut layers = vec!["default".to_string(), crate::choices::path_of(by)];
        if file {
            layers.push(format!("config/<env>.toml [{}]", d.section));
        }
        layers.push(env);
        layers.push(flag);
        let value = if s.secret {
            "<secret>".to_string()
        } else {
            s.value.clone()
        };
        lines.push(format!(
            "[{}] {} ({}) = {value}  run: {}",
            d.section,
            s.key,
            kind_name(s.kind),
            layers.join(" < ")
        ));
    }
    for key in picks {
        for o in &about(key).options {
            lines.push(format!(
                "{key}.{} = {}  build: {}",
                o.key,
                o.default,
                crate::choices::path_of(key)
            ));
        }
    }
    section(
        "Settings: each with its layers, lowest first (run time), or its module file (build time)",
        lines,
    );
    out
}

/// The table on each `docs/modules/*.md` page. Only `docs_check` writes
/// and compares it, so it is built for the tests alone.
#[cfg(test)]
pub mod page {
    use super::*;
    use brocanth_scaffold::{Manifest, Module};

    /// The first line of the generated block on a module page.
    pub const BEGIN: &str = "<!-- generated: capabilities -->";
    /// The last line of the generated block on a module page.
    pub const END: &str = "<!-- end generated: capabilities -->";

    /// The rows of a page's top table that the generated block replaced.
    pub const REPLACED_ROWS: &[&str] = &[
        "Needs",
        "After",
        "Provides",
        "Requires",
        "Uses",
        "Adapts to",
    ];
    /// The settings `module` declares when it is written alone: with its
    /// parent, what it needs, and a provider bound for each Kind it requires.
    /// Empty for a blocked module, which writes nothing.
    pub fn settings_alone(module: &dyn Module) -> Vec<Declared> {
        let m = module.manifest();
        if m.blocked.is_some() {
            return Vec::new();
        }
        let known = crate::commands::create::all();
        let mut alone = vec![m.key];
        alone.extend(m.needs.iter().map(|(need, _)| *need));
        alone.extend(m.parent);
        let mut bound: Vec<(&str, &str)> = m
            .requirements(&alone)
            .map(|r| {
                (
                    r.kind.name,
                    crate::resolve::single_provider(r.kind.name).unwrap_or("(provider)"),
                )
            })
            .collect();
        for (kind, _) in &m.provides {
            bound.push((kind.name, m.key));
        }
        let project = brocanth_scaffold::Project::new("my-app", "My App", &alone, &known)
            .with_bindings(&bound);
        let mut s = brocanth_scaffold::Scaffold::new();
        s.now_writing(m.key);
        module.contribute(&project, &mut s);
        s.settings()
            .filter(|(by, _, _)| *by == m.key)
            .map(|(_, section, setting)| Declared {
                section: section.to_string(),
                setting: setting.clone(),
            })
            .collect()
    }

    /// A page link to another module.
    fn link(key: &str) -> String {
        format!("[`{key}`]({key}.md)")
    }

    /// Text that is safe inside one cell of a Markdown table.
    fn cell(text: &str) -> String {
        text.replace('|', "\\|").replace('\n', " ")
    }

    /// `items` joined by `sep`, or `none`.
    fn or_none(items: Vec<String>, sep: &str) -> String {
        if items.is_empty() {
            "none".to_string()
        } else {
            items.join(sep)
        }
    }

    /// The rows of the generated table, label first.
    pub fn rows(m: &Manifest, settings: &[Declared]) -> Vec<(&'static str, String)> {
        let needs = m
            .needs
            .iter()
            .map(|(key, why)| format!("{}: {}", link(key), cell(why)))
            .collect();
        let after = m.after.iter().map(|key| link(key)).collect();
        let provides = m
            .provides
            .iter()
            .map(|(k, what)| format!("`{}` ({}): {}", k.name, k.gist, cell(what)))
            .collect();
        let requires = m
            .requires
            .iter()
            .map(|r| {
                let providers: Vec<String> = crate::resolve::providers(r.kind.name)
                    .into_iter()
                    .map(link)
                    .collect();
                let mut text = format!("`{}` ({}): {}", r.kind.name, r.kind.gist, cell(r.why));
                if !providers.is_empty() {
                    text.push_str(&format!("; providers: {}", providers.join(", ")));
                }
                if !r.unless.is_empty() {
                    let unless: Vec<String> = r.unless.iter().map(|k| link(k)).collect();
                    text.push_str(&format!("; not with {}", unless.join(", ")));
                }
                text
            })
            .collect();
        let uses = m
            .uses
            .iter()
            .map(|(k, what)| format!("`{}` ({}): {}", k.name, k.gist, cell(what)))
            .collect();
        let adapts = m
            .adapts_to
            .iter()
            .map(|(key, what)| format!("{}: {}", link(key), cell(what)))
            .collect();
        let contributes = m
            .contributions
            .iter()
            .map(|(k, c, on)| {
                let toggle = if k.collected_by.is_some() {
                    String::new()
                } else if *on {
                    ", on".to_string()
                } else {
                    ", off".to_string()
                };
                format!("`{}` `{}`{toggle}: {}", k.name, c.name, cell(c.what))
            })
            .collect();
        let collects = m
            .collects
            .iter()
            .map(|(k, what)| format!("`{}`: {}", k.name, cell(what)))
            .collect();
        let exports = m
            .exports
            .iter()
            .map(|e| {
                let values: Vec<String> = e.values.iter().map(|v| format!("`{v}`")).collect();
                format!("`{}` ({}): {}", e.name, values.join(", "), cell(e.what))
            })
            .collect();
        let hooks = m
            .hooks
            .iter()
            .map(|(name, what, hook)| {
                let phase = hook.phase.map_or("no phase".to_string(), |p| p.to_string());
                format!("`{name}` ({phase}, {}): {}", hook_state(hook), cell(what))
            })
            .collect();
        let options = m
            .options
            .iter()
            .map(|o| {
                let mount = if o.mount {
                    format!(", mount, {}", o.role.name())
                } else {
                    String::new()
                };
                format!(
                    "`{}` = `{}`{mount}: {}",
                    o.key,
                    cell(&o.default),
                    cell(o.help)
                )
            })
            .collect();
        let mut sections: Vec<(String, Vec<String>)> = Vec::new();
        for d in settings {
            let name = if d.setting.secret {
                format!("`{}` (secret)", d.setting.key)
            } else {
                format!("`{}`", d.setting.key)
            };
            match sections.iter_mut().find(|(s, _)| *s == d.section) {
                Some((_, keys)) => keys.push(name),
                None => sections.push((d.section.clone(), vec![name])),
            }
        }
        let settings = sections
            .into_iter()
            .map(|(section, keys)| format!("`[{section}]`: {}", keys.join(", ")))
            .collect();
        vec![
            ("Needs", or_none(needs, "; ")),
            ("After", or_none(after, ", ")),
            ("Provides", or_none(provides, "; ")),
            ("Requires", or_none(requires, "; ")),
            ("Uses", or_none(uses, "; ")),
            ("Adapts to", or_none(adapts, "; ")),
            ("Contributes", or_none(contributes, "; ")),
            ("Collects", or_none(collects, "; ")),
            ("Exports", or_none(exports, "; ")),
            ("Hooks", or_none(hooks, "; ")),
            ("Options (build time)", or_none(options, "; ")),
            ("Settings (run time)", or_none(settings, "; ")),
        ]
    }

    /// The generated block of a module page, from [`BEGIN`] to [`END`], with
    /// no newline after the last line.
    pub fn page_block(m: &Manifest, settings: &[Declared]) -> String {
        let mut out = format!(
            "{BEGIN}\n## Capabilities\n\n\
             Generated from the manifest; do not edit. `docs_check` compares it. After a \
             manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.\n\
             An option is set at build time in `{file}`. A setting is read at run \
             time from these layers, lowest first: the default, the module file, the config \
             file (with a `ConfigSource`), the environment, the flags.\n\n\
             | | |\n|---|---|\n",
            file = crate::choices::path_of(m.key)
        );
        for (label, value) in rows(m, settings) {
            out.push_str(&format!("| {label} | {value} |\n"));
        }
        out.push_str(END);
        out
    }
}
