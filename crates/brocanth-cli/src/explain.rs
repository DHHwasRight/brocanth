//! What a manifest says, as text: the table on each `docs/modules/*.md`
//! page, and the report `brocanth explain` prints for a project, as text
//! or as JSON (`--json`). [`facts`] builds the report once; [`text`] and
//! [`json`] only print it.
//!
//! One source for both. The page shows a module alone; the report shows a
//! project's modules together, with the bindings `brocanth.toml` records.
//! Neither builds nor connects: both read the manifests and run each
//! module's `contribute` in memory for its settings
//! (`docs/module-capabilities.md` §12, stage 6).

use brocanth_scaffold::kind::Cardinality;
use brocanth_scaffold::{Kind, Setting};
use serde::{Deserialize, Serialize};

use crate::commands::create::about;

/// A declared setting, as the page and the report show it.
#[derive(Debug, Clone)]
pub struct Declared {
    /// The section in the file and the first part of the flag: `jobs`.
    pub section: String,
    /// The setting.
    pub setting: Setting,
}

/// How a hook may be turned off, in a few words.
fn state_word(required: bool, on: bool) -> &'static str {
    if required {
        "required"
    } else if on {
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

/// The version of the `brocanth explain --json` document. A field that is
/// renamed, removed or changes its meaning bumps it; a new field does not.
pub const JSON_VERSION: u32 = 1;

/// What `brocanth explain` knows about a project: the text report and the
/// `--json` document are both this, so neither can say what the other
/// does not. The field names are the JSON's, documented in
/// `crates/brocanth-cli/README.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Explained {
    /// [`JSON_VERSION`].
    pub version: u32,
    /// The project name in `brocanth.toml`.
    pub project: String,
    /// Each module, in the order the generator writes them.
    pub modules: Vec<ModuleIn>,
    /// Each Kind of cardinality one that a module provides, requires or
    /// uses, in the order the modules name them.
    pub kinds: Vec<KindBinding>,
    /// Whether `[bindings]` in `brocanth.toml` agrees with the modules.
    pub bindings: BindingsRecord,
    /// What each module adds to a collection, sorted by Kind.
    pub contributions: Vec<Contribution>,
    /// Each hook of each module.
    pub hooks: Vec<HookFact>,
    /// Each run-time setting, with its layers.
    pub settings: Vec<SettingFact>,
    /// Each build-time option, with its module file.
    pub options: Vec<OptionFact>,
}

/// A module and why it is in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleIn {
    pub key: String,
    /// `chosen`, `part of <parent>`, `<module> needs it: <why>`, or
    /// `provides <Kind>: <why>`.
    pub why: Vec<String>,
}

/// A Kind of cardinality one, its provider and its readers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KindBinding {
    pub kind: String,
    /// What the Kind is, in a few words.
    pub gist: String,
    /// The module bound to it, or `null` when none is.
    pub provider: Option<String>,
    /// Why that module, as `brocanth.toml` records it; `null` with no provider.
    pub why: Option<String>,
    /// The modules that require or use it.
    pub read_by: Vec<String>,
}

/// `[bindings]` in `brocanth.toml`, against the modules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindingsRecord {
    /// The file has a `[bindings]` table. Without one, `kinds` holds the
    /// bindings computed from the modules.
    pub recorded: bool,
    /// Bindings the modules make that the table does not record.
    pub unrecorded: Vec<Binding>,
}

/// One Kind bound to one module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub kind: String,
    pub provider: String,
}

/// One item a module adds to a collection: a declared contribution, or the
/// mount path a module option sets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum Contribution {
    /// A contribution the manifest declares.
    Contribution {
        kind: String,
        module: String,
        /// The contribution's name.
        name: String,
        what: String,
        /// The collector, or `null` when no module of the project collects it.
        collected_by: Option<String>,
        /// The `integrate.<group>` toggle that turns it off, for a Kind a
        /// module collects; `null` for one a generator collects.
        toggle: Option<Toggle>,
    },
    /// A module option that is a mount path.
    Mount {
        /// Always `Mount`.
        kind: String,
        module: String,
        /// The option key, such as `mount_at`.
        option: String,
        /// Its default, as Rust source.
        default: String,
        /// What is mounted there.
        role: String,
    },
}

impl Contribution {
    fn kind(&self) -> &str {
        match self {
            Contribution::Contribution { kind, .. } | Contribution::Mount { kind, .. } => kind,
        }
    }
}

/// The toggle a contribution is turned off with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Toggle {
    /// `integrate.<group>` in the module file.
    pub group: String,
    /// On unless the project turns it off.
    pub default: bool,
}

/// One hook of one module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookFact {
    pub module: String,
    pub hook: String,
    /// `Identify`, `Guard`, `Cache` or `Encode`, or `null` with no phase.
    pub phase: Option<String>,
    /// A project may not turn it off.
    pub required: bool,
    /// On when the project says nothing.
    pub on: bool,
    pub what: String,
}

/// One run-time setting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingFact {
    /// The module that declares it.
    pub module: String,
    /// The section in the file and the first part of the flag.
    pub section: String,
    pub key: String,
    /// `text`, `bool`, `integer`, `float` or `list`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The default, as the config file writes it; `null` for a secret.
    pub value: Option<String>,
    pub secret: bool,
    /// The environment variable, without the `$`.
    pub env: String,
    /// The flags that set it; empty with no flag.
    pub flags: Vec<String>,
    /// Every place it is read from, lowest first: `default`, the module
    /// file, the config file (with a `ConfigSource`), `env`, the flags (or
    /// `no flag`).
    pub layers: Vec<String>,
}

/// One build-time option.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OptionFact {
    pub module: String,
    pub key: String,
    /// The default, as Rust source.
    pub default: String,
    /// The module file it is set in.
    pub file: String,
}

/// What `brocanth explain` reports for a project of `picks`. `recorded` is
/// `[bindings]` from `brocanth.toml`, `None` when the file has no table.
/// `settings` is every setting the generator declares for these picks.
pub fn facts(
    name: &str,
    picks: &[&'static str],
    recorded: Option<&Recorded>,
    settings: &[(String, Declared)],
) -> Explained {
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

    let modules = picks
        .iter()
        .map(|key| ModuleIn {
            key: key.to_string(),
            why: why_in(key, picks, bindings),
        })
        .collect();

    // Each Kind of cardinality one that a module binds, requires or uses.
    let mut named: Vec<brocanth_scaffold::KindRef> = Vec::new();
    for key in picks {
        let m = about(key);
        let refs = m
            .provides
            .iter()
            .map(|(k, _)| *k)
            .chain(m.requires.iter().map(|r| r.kind))
            .chain(m.uses.iter().map(|(k, _)| *k));
        for k in refs {
            if k.cardinality == Cardinality::One && !named.iter().any(|seen| seen.name == k.name) {
                named.push(k);
            }
        }
    }
    let kinds = named
        .iter()
        .map(|k| {
            let read_by = picks
                .iter()
                .filter(|p| {
                    let m = about(p);
                    m.requirements(picks).any(|r| r.kind.name == k.name)
                        || m.uses.iter().any(|(u, _)| u.name == k.name)
                })
                .map(|p| p.to_string())
                .collect();
            let bound = bindings.iter().find(|(kind, _, _)| kind == k.name);
            KindBinding {
                kind: k.name.to_string(),
                gist: k.gist.to_string(),
                provider: bound.map(|(_, provider, _)| provider.clone()),
                why: bound.map(|(_, _, why)| why.clone()),
                read_by,
            }
        })
        .collect();
    let unrecorded = match recorded {
        Some(recorded) => computed
            .iter()
            .filter(|(kind, provider, _)| {
                !recorded.iter().any(|(k, p, _)| k == kind && p == provider)
            })
            .map(|(kind, provider, _)| Binding {
                kind: kind.clone(),
                provider: provider.clone(),
            })
            .collect(),
        None => Vec::new(),
    };

    let collectors = crate::resolve::collectors(picks);
    let mut contributions = Vec::new();
    for key in picks {
        let m = about(key);
        for (k, c, on) in &m.contributions {
            let (collected_by, toggle) = match (
                k.collected_by,
                collectors.iter().find(|(n, _)| *n == k.name),
            ) {
                (Some(by), _) => (Some(by.to_string()), None),
                (None, Some((_, by))) => (
                    Some(by.to_string()),
                    Some(Toggle {
                        group: k.group.to_string(),
                        default: *on,
                    }),
                ),
                (None, None) => (None, None),
            };
            contributions.push(Contribution::Contribution {
                kind: k.name.to_string(),
                module: key.to_string(),
                name: c.name.to_string(),
                what: c.what.to_string(),
                collected_by,
                toggle,
            });
        }
        for o in m.options.iter().filter(|o| o.mount) {
            contributions.push(Contribution::Mount {
                kind: "Mount".to_string(),
                module: key.to_string(),
                option: o.key.to_string(),
                default: o.default.to_string(),
                role: o.role.name().to_string(),
            });
        }
    }
    contributions.sort_by(|a, b| a.kind().cmp(b.kind()));

    let mut hooks = Vec::new();
    for key in picks {
        for (hook, what, h) in &about(key).hooks {
            hooks.push(HookFact {
                module: key.to_string(),
                hook: hook.to_string(),
                phase: h.phase.map(|p| p.to_string()),
                required: h.required,
                on: h.on,
                what: what.to_string(),
            });
        }
    }

    let file = picks.iter().any(|p| {
        about(p).gives(<brocanth_scaffold::ConfigSource as brocanth_scaffold::kind::Kind>::NAME)
    });
    let settings = settings
        .iter()
        .map(|(by, d)| {
            let s = &d.setting;
            let env = format!(
                "{ident}_{}__{}",
                d.section.replace('-', "_").to_uppercase(),
                s.key.replace('.', "__").to_uppercase()
            );
            let flags = if s.secret {
                vec![
                    format!("--{}.{}_file", d.section, s.key),
                    format!("--{}.{}_env", d.section, s.key),
                ]
            } else if s.flag {
                vec![format!("--{}.{}", d.section, s.key)]
            } else {
                Vec::new()
            };
            let mut layers = vec!["default".to_string(), crate::choices::path_of(by)];
            if file {
                layers.push(format!("config/<env>.toml [{}]", d.section));
            }
            layers.push(format!("${env}"));
            layers.push(if flags.is_empty() {
                "no flag".to_string()
            } else {
                flags.join(", ")
            });
            SettingFact {
                module: by.clone(),
                section: d.section.clone(),
                key: s.key.to_string(),
                kind: kind_name(s.kind).to_string(),
                value: (!s.secret).then(|| s.value.clone()),
                secret: s.secret,
                env,
                flags,
                layers,
            }
        })
        .collect();

    let mut options = Vec::new();
    for key in picks {
        for o in &about(key).options {
            options.push(OptionFact {
                module: key.to_string(),
                key: o.key.to_string(),
                default: o.default.to_string(),
                file: crate::choices::path_of(key),
            });
        }
    }

    Explained {
        version: JSON_VERSION,
        project: name.to_string(),
        modules,
        kinds,
        bindings: BindingsRecord {
            recorded: recorded.is_some(),
            unrecorded,
        },
        contributions,
        hooks,
        settings,
        options,
    }
}

/// The plain-text report: [`Explained`] and nothing else, so every fact it
/// prints is in the JSON.
pub fn text(e: &Explained) -> String {
    let width = e.modules.iter().map(|m| m.key.len()).max().unwrap_or(0);
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
        &format!("Modules: why each is in ({} modules)", e.modules.len()),
        e.modules
            .iter()
            .map(|m| format!("{:width$}  {}", m.key, m.why.join("; ")))
            .collect(),
    );

    let mut lines = Vec::new();
    for k in &e.kinds {
        let read_by = if k.read_by.is_empty() {
            String::new()
        } else {
            format!("; read by {}", k.read_by.join(", "))
        };
        match &k.provider {
            Some(provider) => lines.push(format!(
                "{:14} <- {provider} ({}){read_by}",
                k.kind,
                k.why.as_deref().unwrap_or_default()
            )),
            None => lines.push(format!(
                "{:14} <- none: each reader writes its code without {}{read_by}",
                k.kind, k.gist
            )),
        }
    }
    if e.bindings.recorded {
        for b in &e.bindings.unrecorded {
            lines.push(format!(
                "! brocanth.toml does not record {} <- {}; run `brocanth add` or `brocanth remove` once to rewrite it",
                b.kind, b.provider
            ));
        }
    } else {
        lines.push(
            "! brocanth.toml has no [bindings] table; the bindings above are computed from the modules"
                .to_string(),
        );
    }
    section("Kinds: the provider of each", lines);

    let lines = e
        .contributions
        .iter()
        .map(|c| match c {
            Contribution::Contribution {
                kind,
                module,
                name,
                what,
                collected_by,
                toggle,
            } => {
                let to = match (collected_by, toggle) {
                    (Some(by), None) => format!("collected by {by}"),
                    (Some(by), Some(t)) => format!(
                        "collected by {by}; integrate.{} = {} by default",
                        t.group, t.default
                    ),
                    (None, _) => "not written: no module collects it".to_string(),
                };
                format!("{kind:13} <- {module}: `{name}` ({to}): {what}")
            }
            Contribution::Mount {
                kind,
                module,
                option,
                default,
                role,
            } => format!("{kind:13} <- {module}: {option} = {default} ({role})"),
        })
        .collect();
    section(
        "Contributions: what each module adds, and who collects it",
        lines,
    );

    let lines = e
        .hooks
        .iter()
        .map(|h| {
            format!(
                "{:width$}  {:16} {:8} {:14} {}",
                h.module,
                h.hook,
                h.phase.as_deref().unwrap_or("-"),
                state_word(h.required, h.on),
                h.what
            )
        })
        .collect();
    section(
        "Hooks: phase, and whether a project may turn each off",
        lines,
    );

    let mut lines: Vec<String> = e
        .settings
        .iter()
        .map(|s| {
            format!(
                "[{}] {} ({}) = {}  run: {}",
                s.section,
                s.key,
                s.kind,
                s.value.as_deref().unwrap_or("<secret>"),
                s.layers.join(" < ")
            )
        })
        .collect();
    lines.extend(
        e.options
            .iter()
            .map(|o| format!("{}.{} = {}  build: {}", o.module, o.key, o.default, o.file)),
    );
    section(
        "Settings: each with its layers, lowest first (run time), or its module file (build time)",
        lines,
    );
    out
}

/// The `--json` document: [`Explained`], pretty-printed, keys in the order
/// the struct declares them.
pub fn json(e: &Explained) -> String {
    serde_json::to_string_pretty(e).expect("the facts are plain strings, lists and booleans")
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
                    crate::resolve::addable_provider(r.kind.name).unwrap_or("(provider)"),
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
                format!(
                    "`{name}` ({phase}, {}): {}",
                    state_word(hook.required, hook.on),
                    cell(what)
                )
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
