//! `src/modules/`: one Rust file of choices per module, and the types those
//! files fill (`docs/module-config.md`).
//!
//! Four files come from here:
//!
//! * `src/modules/mod.rs`, the generator's: a `pub mod` line per module and
//!   `ALL`, the module list of `brocanth.toml`.
//! * `src/modules/<key>.rs`, the project's: written when it is missing,
//!   never again. Every field is written out with its comment, then
//!   `..Module::DEFAULT`, so a field that a later version adds takes its
//!   default and an old file still compiles.
//! * `src/modules/brocanth.rs`, the project's: the framework's own file,
//!   with the project flag rule and the `[server]` renames.
//! * `src/plugins/shape.rs`, the generator's: the types.
//!
//! **Decision: a module file depends on its own module only.** The fields
//! come from the module's manifest and its declared settings, never from
//! the other modules. So `add` and `remove` of another module never need a
//! change in a file the project owns. `Integrate::admin` exists in a project
//! without `admin` for that reason.

use brocanth_scaffold::{module_ident, Hook, Kind, Opt, Scaffold as Built};

use crate::commands::create::{about, Answers};

/// The owner the framework's own settings are recorded under, and the name
/// of its module file.
pub const FRAMEWORK: &str = "brocanth";

/// What one module file holds, from the manifest and the declared settings.
#[derive(Debug, Clone)]
pub struct Shape {
    /// The module key, or [`FRAMEWORK`].
    pub key: String,
    /// One line on the module, for the file's header.
    pub gist: String,
    /// The sections whose settings have flags here.
    pub sections: Vec<String>,
    /// Each setting with a flag: the `Key` variant and the setting's path.
    pub keys: Vec<(String, String)>,
    /// Every hook the module declares, the required ones included.
    pub hooks: Vec<(String, String, Hook)>,
    /// The hooking plugin's instance name, when the module recorded one.
    pub instance: Option<String>,
    /// Each integration: the host, what it does, and the default.
    pub integrations: Vec<(String, String, bool)>,
    /// The static options.
    pub options: Vec<Opt>,
}

impl Shape {
    /// The Rust name of the module: `middleware_rate_limit`, `r#static`.
    pub fn ident(&self) -> String {
        module_ident(&self.key)
    }

    /// Whether the file has `flags` and `rename`.
    pub fn has_flags(&self) -> bool {
        !self.keys.is_empty()
    }

    /// The hooks a file may turn off.
    pub fn toggles(&self) -> impl Iterator<Item = &(String, String, Hook)> {
        self.hooks.iter().filter(|(_, _, hook)| !hook.required)
    }

    fn framework(&self) -> bool {
        self.key == FRAMEWORK
    }

    /// The hook's id, as the core names it: `analytics.count`, or the bare
    /// name when the module recorded no instance.
    pub fn hook_id(&self, name: &str) -> String {
        match &self.instance {
            Some(instance) => format!("{instance}.{name}"),
            None => name.to_string(),
        }
    }
}

/// The path of a module's file: `src/modules/middleware_rate_limit.rs`.
///
/// The file name has no `r#`: `pub mod r#static;` is `static.rs`.
pub fn path_of(key: &str) -> String {
    format!("src/modules/{}.rs", key.replace('-', "_"))
}

/// Whether `path` is a module file the project owns: `src/modules/<x>.rs`
/// and not the generator's `mod.rs`.
pub fn is_module_file(path: &str) -> bool {
    path.strip_prefix("src/modules/")
        .is_some_and(|rest| rest != "mod.rs" && rest.ends_with(".rs") && !rest.contains('/'))
}

/// The shape of every module file: the framework's first, then each
/// module in `brocanth.toml` order.
pub fn shapes(a: &Answers, built: &Built) -> Vec<Shape> {
    let mut out = vec![shape(FRAMEWORK, built)];
    for key in &a.picks {
        out.push(shape(key, built));
    }
    out
}

fn shape(key: &str, built: &Built) -> Shape {
    let mut sections: Vec<String> = Vec::new();
    let mut keys: Vec<(String, String)> = Vec::new();
    let flagged: Vec<(&str, &str)> = built
        .settings()
        .filter(|(by, _, setting)| *by == key && setting.flag && setting.kind != Kind::List)
        .map(|(_, section, setting)| (section, setting.key))
        .collect();
    for (section, setting) in &flagged {
        if !sections.iter().any(|s| s == section) {
            sections.push((*section).to_string());
        }
        // `Key::Cookie` for `sessions.cookie`. Two sections of one module
        // with one key name: the section goes in front of both.
        let short = camel(setting);
        let twice = flagged
            .iter()
            .filter(|(_, other)| camel(other) == short)
            .count()
            > 1;
        let variant = if twice {
            camel(&format!("{section}_{setting}"))
        } else {
            short
        };
        keys.push((variant, format!("{section}.{setting}")));
    }
    if key == FRAMEWORK {
        return Shape {
            key: key.to_string(),
            gist: "the framework itself: the flag rule of the project, and `[server]`".to_string(),
            sections,
            keys,
            hooks: Vec::new(),
            instance: None,
            integrations: Vec::new(),
            options: Vec::new(),
        };
    }
    let m = about(key);
    Shape {
        key: key.to_string(),
        gist: m.gist.to_string(),
        sections,
        keys,
        hooks: m
            .hooks
            .iter()
            .map(|(name, what, hook)| (name.to_string(), what.to_string(), *hook))
            .collect(),
        instance: built.instance_of(key).map(str::to_string),
        integrations: m
            .integrations
            .iter()
            .map(|(host, what, on)| (host.to_string(), what.to_string(), *on))
            .collect(),
        options: m.options.clone(),
    }
}

/// `pool.max_size` -> `PoolMaxSize`.
fn camel(key: &str) -> String {
    key.split(['.', '_', '-'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// A field name from a hook or host name: `consent-log` -> `consent_log`.
pub fn field(name: &str) -> String {
    name.replace('-', "_")
}

// -- src/modules/mod.rs -------------------------------------------------------

/// `src/modules/mod.rs`: the list.
pub fn mod_rs(a: &Answers) -> String {
    let mut out = String::from(
        "//! The modules this project integrates, and the choices each one offers.\n\
         //!\n\
         //! One file per module beside this one, and `brocanth.rs` for the framework\n\
         //! itself. Those files are yours: `brocanth add` writes one when it is\n\
         //! missing and never again. The types they fill are in `src/plugins/shape.rs`.\n\
         \n\
         // A choice this project does not read, such as an integration with a module\n\
         // it does not have, is not dead code.\n\
         #![allow(dead_code)]\n\
         \n\
         /// The framework itself: `[server]`, and whether flags work beside a config\n\
         /// file.\n\
         pub mod brocanth;\n",
    );
    // Sorted, as rustfmt sorts `mod` lines.
    let mut mods: Vec<String> = a
        .picks
        .iter()
        .map(|key| format!("pub mod {};\n", module_ident(key)))
        .collect();
    mods.sort_unstable_by_key(|line| line.replace("r#", ""));
    if !mods.is_empty() {
        out.push('\n');
        out.push_str(&mods.concat());
    }
    out.push_str(
        "\n/// Every module in `brocanth.toml`, in registration order: the list of\n\
         /// `[modules] enabled`, which `brocanth add` and `brocanth remove` keep.\n",
    );
    out.push_str(&string_array("pub const ALL: &[&str] = &[", &a.picks, "];"));
    out
}

/// A `&[..]` of string literals, as rustfmt lays it out: on one line when
/// it fits, else one per line, or packed when every item is short.
fn string_array(head: &str, items: &[&str], tail: &str) -> String {
    let quoted: Vec<String> = items.iter().map(|item| format!("{item:?}")).collect();
    let one = format!("{head}{}]{}", quoted.join(", "), &tail[1..]);
    // rustfmt's `array_width` is 60 columns of content.
    if quoted.join(", ").len() <= 60 && one.len() <= 100 {
        return format!("{one}\n");
    }
    let mut out = format!("{head}\n");
    // `short_array_element_width_threshold`: every item at most 10 wide
    // packs several to a line.
    if quoted.iter().all(|q| q.len() <= 10) {
        let mut line = String::from("   ");
        for q in &quoted {
            if line.len() + 1 + q.len() + 1 > 100 {
                out.push_str(&line);
                out.push('\n');
                line = String::from("   ");
            }
            line.push(' ');
            line.push_str(q);
            line.push(',');
        }
        out.push_str(&line);
        out.push('\n');
    } else {
        for q in &quoted {
            out.push_str(&format!("    {q},\n"));
        }
    }
    out.push_str(tail);
    out.push('\n');
    out
}

// -- src/plugins/shape.rs -----------------------------------------------------

/// `src/plugins/shape.rs`: one Rust module per module file.
pub fn shape_rs(shapes: &[Shape]) -> String {
    let mut out = String::from(SHAPE_HEAD);
    for shape in shapes {
        out.push('\n');
        out.push_str(&shape_mod(shape));
    }
    out
}

const SHAPE_HEAD: &str = r#"//! The shape of each file in src/modules/: one Rust module per file.
//!
//! Each type has a `DEFAULT`, and each file ends with `..Module::DEFAULT`, so
//! a field that a later version adds never breaks a file that predates it.

// A choice this project does not read, such as an integration with a module
// it does not have, is not dead code. `Flag` and `Flags` are in each module
// for its file's one `use` line.
#![allow(dead_code, unused_imports)]

/// Whether a module's settings have flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flags {
    /// The project rule: on without a config file; with one,
    /// `flags_with_config` in src/modules/brocanth.rs.
    Project,
    /// On, beside a config file too.
    On,
    /// Off. The variable and the config file still set each value.
    Off,
}

/// The flag of one setting, when it is not the declared name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flag {
    /// `--<name>` in place of `--<section>.<key>`.
    As(&'static str),
    /// No flag. The variable and the config key still set the value.
    Hidden,
}

impl Flag {
    /// The name the flag answers to. Empty for no flag.
    pub const fn name(self) -> &'static str {
        match self {
            Flag::As(name) => name,
            Flag::Hidden => "",
        }
    }
}
"#;

fn shape_mod(shape: &Shape) -> String {
    let ident = shape.ident();
    let mut body = String::new();
    if shape.has_flags() {
        body.push_str("    pub use super::{Flag, Flags};\n\n");
        let sections: Vec<String> = shape.sections.iter().map(|s| format!("`[{s}]`")).collect();
        let doc = format!(
            "A setting of {} that has a flag. A secret or a list has no flag, so it \
             has no variant here and cannot be renamed.",
            sections.join(", ")
        );
        for line in wrap(&doc, 72) {
            body.push_str(&format!("    /// {line}\n"));
        }
        body.push_str(
            "    #[derive(Clone, Copy, Debug, PartialEq, Eq)]\n\
             \x20   pub enum Key {\n",
        );
        for (variant, _) in &shape.keys {
            body.push_str(&format!("        {variant},\n"));
        }
        body.push_str(
            "    }\n\
             \n\
             \x20   impl Key {\n\
             \x20       /// The setting, as `config/<env>.toml` and the flag spell it.\n\
             \x20       pub const fn path(self) -> &'static str {\n\
             \x20           match self {\n",
        );
        for (variant, path) in &shape.keys {
            let arm = format!("                Key::{variant} => {path:?},");
            if arm.len() <= 100 {
                body.push_str(&arm);
                body.push('\n');
            } else {
                body.push_str(&format!(
                    "                Key::{variant} => {{\n\
                     \x20                   {path:?}\n\
                     \x20               }}\n"
                ));
            }
        }
        body.push_str("            }\n        }\n    }\n\n");
    }
    let toggles: Vec<&(String, String, Hook)> = shape.toggles().collect();
    if !toggles.is_empty() {
        body.push_str(&format!(
            "    /// The hooks of `{}` that a file may turn off. A hook the module\n\
             \x20   /// cannot work without has no field.\n\
             \x20   pub struct Hooks {{\n",
            shape.key
        ));
        for (name, what, _) in &toggles {
            for line in wrap(&format!("`{}`: {what}.", shape.hook_id(name)), 72) {
                body.push_str(&format!("        /// {line}\n"));
            }
            body.push_str(&format!("        pub {}: bool,\n", field(name)));
        }
        body.push_str("    }\n\n    impl Hooks {\n");
        body.push_str(&default_const(
            &toggles
                .iter()
                .map(|(name, _, hook)| (field(name), hook.on.to_string()))
                .collect::<Vec<_>>(),
        ));
        body.push_str(
            "\n\
             \x20       /// The hook names this file turns off, for `AppBuilder::hooks_off`.\n\
             \x20       pub fn off(&self) -> Vec<&'static str> {\n\
             \x20           let mut off = Vec::new();\n",
        );
        for (name, _, _) in &toggles {
            body.push_str(&format!(
                "            if !self.{} {{\n                off.push({name:?});\n            }}\n",
                field(name)
            ));
        }
        body.push_str("            off\n        }\n    }\n\n");
    }
    if !shape.integrations.is_empty() {
        body.push_str(&format!(
            "    /// The code `{}` writes into another module. A field exists whether\n\
             \x20   /// that module is in the project or not, and does nothing without it.\n\
             \x20   pub struct Integrate {{\n",
            shape.key
        ));
        for (host, what, _) in &shape.integrations {
            for line in wrap(&format!("With `{host}`: {what}."), 72) {
                body.push_str(&format!("        /// {line}\n"));
            }
            body.push_str(&format!("        pub {}: bool,\n", field(host)));
        }
        body.push_str("    }\n\n    impl Integrate {\n");
        body.push_str(&default_const(
            &shape
                .integrations
                .iter()
                .map(|(host, _, on)| (field(host), on.to_string()))
                .collect::<Vec<_>>(),
        ));
        body.push_str("    }\n\n");
    }

    // The module itself.
    let file = path_of(&shape.key);
    let mut fields: Vec<(String, String, String)> = Vec::new();
    if shape.framework() {
        fields.push((
            "flags_with_config".to_string(),
            "bool".to_string(),
            "With the `config` module, whether `Flags::Project` reads flags.".to_string(),
        ));
    } else if shape.has_flags() {
        fields.push((
            "flags".to_string(),
            "Flags".to_string(),
            "Whether the settings of this module have flags.".to_string(),
        ));
    }
    if shape.has_flags() {
        fields.push((
            "rename".to_string(),
            "&'static [(Key, Flag)]".to_string(),
            "Flags under another name, or no flag.".to_string(),
        ));
    }
    if !toggles.is_empty() {
        fields.push((
            "hooks".to_string(),
            "Hooks".to_string(),
            "The hooks that run.".to_string(),
        ));
    }
    if !shape.integrations.is_empty() {
        fields.push((
            "integrate".to_string(),
            "Integrate".to_string(),
            "The code written into another module.".to_string(),
        ));
    }
    for opt in &shape.options {
        fields.push((
            opt.key.to_string(),
            opt.kind.rust_type().to_string(),
            opt.help.to_string(),
        ));
    }
    body.push_str(&format!("    /// What {file} holds.\n"));
    if fields.is_empty() {
        body.push_str("    pub struct Module {}\n");
    } else {
        body.push_str("    pub struct Module {\n");
        for (name, ty, doc) in &fields {
            for line in wrap(doc, 72) {
                body.push_str(&format!("        /// {line}\n"));
            }
            body.push_str(&format!("        pub {name}: {ty},\n"));
        }
        body.push_str("    }\n");
    }
    body.push_str("\n    impl Module {\n");
    let mut defaults: Vec<(String, String)> = Vec::new();
    for (name, _, _) in &fields {
        let value = match name.as_str() {
            "flags_with_config" => "false".to_string(),
            "flags" => "Flags::Project".to_string(),
            "rename" => "&[]".to_string(),
            "hooks" => "Hooks::DEFAULT".to_string(),
            "integrate" => "Integrate::DEFAULT".to_string(),
            option => shape
                .options
                .iter()
                .find(|opt| opt.key == option)
                .map(|opt| opt.default.clone())
                .unwrap_or_default(),
        };
        defaults.push((name.clone(), value));
    }
    body.push_str(&default_const(&defaults));
    body.push_str("    }\n");

    format!("/// The shape of {file}.\npub mod {ident} {{\n{body}}}\n",)
}

/// `pub const DEFAULT: Self = Self { .. };`, as rustfmt lays it out: on one
/// line while the fields fit in `struct_lit_width` (18 columns).
fn default_const(fields: &[(String, String)]) -> String {
    let doc = "        /// The value of each field the file does not name.\n";
    if fields.is_empty() {
        return format!("{doc}        pub const DEFAULT: Self = Self {{}};\n");
    }
    let inline: Vec<String> = fields
        .iter()
        .map(|(name, value)| format!("{name}: {value}"))
        .collect();
    let inline = inline.join(", ");
    if inline.len() <= 18 {
        return format!("{doc}        pub const DEFAULT: Self = Self {{ {inline} }};\n");
    }
    let mut out = format!("{doc}        pub const DEFAULT: Self = Self {{\n");
    for (name, value) in fields {
        out.push_str(&format!("            {name}: {value},\n"));
    }
    out.push_str("        };\n");
    out
}

// -- src/modules/<key>.rs -------------------------------------------------------

/// Values a module file starts with in place of the defaults: what an old
/// `src/flags.rs` held, when `add` moves it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Choices {
    /// `flags_with_config` in `brocanth.rs`.
    pub flags_with_config: bool,
    /// Each rename: the `Key` variant, and the flag's name, empty for none.
    pub rename: Vec<(String, String)>,
}

/// A module file, as `create` and `add` write it once.
pub fn module_file(shape: &Shape, choices: &Choices) -> String {
    let ident = shape.ident();
    let mut out = String::new();
    // -- the header
    if shape.framework() {
        out.push_str("//! The framework itself: the flag rule of the project, and `[server]`.\n");
    } else {
        for line in wrap(&format!("`{}`: {}.", shape.key, shape.gist), 76) {
            out.push_str(&format!("//! {line}\n"));
        }
    }
    out.push_str(
        "//!\n\
         //! Written once by `brocanth`. This file is yours: no command rewrites it.\n\
         //! A change here takes effect on the next `cargo build`.\n",
    );
    let toggles: Vec<&(String, String, Hook)> = shape.toggles().collect();
    let nothing = !shape.has_flags()
        && toggles.is_empty()
        && shape.integrations.is_empty()
        && shape.options.is_empty();
    if !shape.sections.is_empty() && !shape.framework() {
        let sections: Vec<String> = shape.sections.iter().map(|s| format!("[{s}]")).collect();
        let text = format!(
            "A value that differs per deployment is a setting, not a choice: {} in the \
             config file, an environment variable, or a flag.",
            sections.join(", ")
        );
        out.push_str("//!\n");
        for line in wrap(&text, 76) {
            out.push_str(&format!("//! {line}\n"));
        }
    }
    let required: Vec<&(String, String, Hook)> = shape
        .hooks
        .iter()
        .filter(|(_, _, hook)| hook.required)
        .collect();
    if !required.is_empty() {
        out.push_str(&format!(
            "//!\n//! No field turns {} off, because the module needs {}:\n",
            if required.len() == 1 {
                "this hook"
            } else {
                "these hooks"
            },
            if required.len() == 1 { "it" } else { "them" },
        ));
        for (name, what, _) in required {
            let text = format!("`{}`: {what}.", shape.hook_id(name));
            for (i, line) in wrap(&text, 72).iter().enumerate() {
                let lead = if i == 0 { "- " } else { "  " };
                out.push_str(&format!("//! {lead}{line}\n"));
            }
        }
    }
    if nothing {
        out.push_str(
            "//!\n\
             //! Nothing to choose yet. When a later version of the module adds a\n\
             //! choice, this file takes its default.\n",
        );
    }
    out.push('\n');

    // -- the imports
    let mut names: Vec<&str> = Vec::new();
    if shape.has_flags() {
        names.extend(["Flag"]);
        if !shape.framework() {
            names.push("Flags");
        }
    }
    if !toggles.is_empty() {
        names.push("Hooks");
    }
    if !shape.integrations.is_empty() {
        names.push("Integrate");
    }
    if shape.has_flags() {
        names.push("Key");
    }
    names.push("Module");
    let path = format!("crate::plugins::shape::{ident}");
    if shape.has_flags() {
        out.push_str("// `Flag` and `Key` are for `rename`: unused until it names a setting.\n");
        out.push_str("#[allow(unused_imports)]\n");
    }
    out.push_str(&use_line(&path, &names));
    out.push('\n');

    // -- the value
    if nothing {
        out.push_str(
            "#[allow(clippy::needless_update)]\n\
             pub const MODULE: Module = Module { ..Module::DEFAULT };\n",
        );
        return out;
    }
    out.push_str(
        "// Every field is written out so you can read the choices. The last line\n\
         // gives a default to a field that a later version of the module adds.\n\
         #[allow(clippy::needless_update)]\n\
         pub const MODULE: Module = Module {\n",
    );
    let mut groups: Vec<String> = Vec::new();
    if shape.framework() {
        groups.push(format!(
            "    // With the `config` module, whether the binary reads flags too. Off:\n\
             \x20   // the file and the environment are the sources, and a flag is refused\n\
             \x20   // with a pointer to this line. Without `config`, flags are always read.\n\
             \x20   // A module file with `Flags::On` or `Flags::Off` overrides this.\n\
             \x20   flags_with_config: {},\n",
            choices.flags_with_config
        ));
    } else if shape.has_flags() {
        let sections: Vec<String> = shape.sections.iter().map(|s| format!("[{s}]")).collect();
        groups.push(format!(
            "    // The command-line flags of {}. `Flags::Project` follows\n\
             \x20   // src/modules/brocanth.rs. `Flags::On` and `Flags::Off` decide here.\n\
             \x20   flags: Flags::Project,\n",
            sections.join(", ")
        ));
    }
    if shape.has_flags() {
        let (variant, path) = &shape.keys[0];
        let example = path.rsplit('.').next().unwrap_or(path).replace('_', "-");
        let mut group = format!(
            "    // Another name for a flag, or no flag. The variable and the config key\n\
             \x20   // keep the declared name. Examples:\n\
             \x20   //   (Key::{variant}, Flag::As({example:?})),\n\
             \x20   //   (Key::{variant}, Flag::Hidden),\n"
        );
        group.push_str(&rename_line(&choices.rename));
        groups.push(group);
    }
    if !toggles.is_empty() {
        let mut group = String::from("    hooks: Hooks {\n");
        for (name, what, hook) in &toggles {
            for line in wrap(&format!("`{}`: {what}.", shape.hook_id(name)), 70) {
                group.push_str(&format!("        // {line}\n"));
            }
            group.push_str(&format!("        {}: {},\n", field(name), hook.on));
        }
        group.push_str("        ..Hooks::DEFAULT\n    },\n");
        groups.push(group);
    }
    if !shape.integrations.is_empty() {
        let mut group = String::from("    integrate: Integrate {\n");
        for (host, what, on) in &shape.integrations {
            let text = format!(
                "{}. Without the `{host}` module this does nothing.",
                capitalised(what)
            );
            for line in wrap(&text, 70) {
                group.push_str(&format!("        // {line}\n"));
            }
            group.push_str(&format!("        {}: {on},\n", field(host)));
        }
        group.push_str("        ..Integrate::DEFAULT\n    },\n");
        groups.push(group);
    }
    for opt in &shape.options {
        let mut group = String::new();
        for line in wrap(opt.help, 72) {
            group.push_str(&format!("    // {line}\n"));
        }
        group.push_str(&format!("    {}: {},\n", opt.key, opt.default));
        groups.push(group);
    }
    groups.push("    ..Module::DEFAULT\n".to_string());
    out.push_str(&groups.join("\n"));
    out.push_str("};\n");
    out
}

/// `rename: &[..],`, laid out as rustfmt lays out an array of tuples.
fn rename_line(rename: &[(String, String)]) -> String {
    let items: Vec<String> = rename
        .iter()
        .map(|(variant, name)| {
            if name.is_empty() {
                format!("(Key::{variant}, Flag::Hidden)")
            } else {
                format!("(Key::{variant}, Flag::As({name:?}))")
            }
        })
        .collect();
    let inline = items.join(", ");
    let one = format!("    rename: &[{inline}],");
    if inline.len() <= 60 && one.len() <= 100 {
        return format!("{one}\n");
    }
    let mut out = String::from("    rename: &[\n");
    for item in items {
        out.push_str(&format!("        {item},\n"));
    }
    out.push_str("    ],\n");
    out
}

/// One `use` line, as rustfmt writes it: braces for more than one name,
/// and the names on a line of their own past 100 columns.
fn use_line(path: &str, names: &[&str]) -> String {
    if let [one] = names {
        return format!("use {path}::{one};\n");
    }
    let joined = names.join(", ");
    let line = format!("use {path}::{{{joined}}};");
    if line.len() <= 100 {
        format!("{line}\n")
    } else {
        format!("use {path}::{{\n    {joined},\n}};\n")
    }
}

/// `text` with its first letter in upper case.
fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// `text` in lines of at most `width` characters, broken at spaces.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.len() + 1 + word.len() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// The `Key` variant of a setting path in `shapes`, and the module file that
/// holds it: what `add` needs to move a rename out of `src/flags.rs`.
pub fn key_of<'a>(shapes: &'a [Shape], path: &str) -> Option<(&'a Shape, &'a str)> {
    shapes.iter().find_map(|shape| {
        shape
            .keys
            .iter()
            .find(|(_, p)| p == path)
            .map(|(variant, _)| (shape, variant.as_str()))
    })
}

// -- the old src/flags.rs -------------------------------------------------------

/// The file the flag choices lived in before `src/modules/`.
pub const LEGACY_FLAGS: &str = "src/flags.rs";

/// What an earlier `brocanth remove` wrote in front of a rename it parked.
const LEGACY_PARKED: &str = "// Parked by `brocanth remove`; `brocanth add` restores it: ";

/// What an old `src/flags.rs` holds, read as text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Legacy {
    /// `WITH_CONFIG`.
    pub with_config: bool,
    /// Each rename, parked ones included: the path, and the flag's name.
    pub renames: Vec<(String, String)>,
}

/// Reads an old `src/flags.rs`. It accepts the two forms the generator
/// wrote, `pub const WITH_CONFIG: bool = <bool>;` and `RENAMES` entries of
/// two string literals, with comments around them. Anything else comes
/// back as the lines it does not understand, for the person to move.
pub fn read_legacy(text: &str) -> Result<Legacy, Vec<String>> {
    let mut legacy = Legacy::default();
    let mut unknown = Vec::new();
    let mut in_renames = false;
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix(LEGACY_PARKED) {
            match entries(rest) {
                Some(found) => legacy.renames.extend(found),
                None => unknown.push(line.to_string()),
            }
            continue;
        }
        if t.is_empty() || t.starts_with("//") || t == "#[allow(dead_code)]" {
            continue;
        }
        let code = t.split_once("//").map_or(t, |(code, _)| code.trim_end());
        if in_renames {
            if code == "];" {
                in_renames = false;
            } else {
                match entries(code) {
                    Some(found) => legacy.renames.extend(found),
                    None => unknown.push(line.to_string()),
                }
            }
            continue;
        }
        if let Some(value) = code.strip_prefix("pub const WITH_CONFIG: bool = ") {
            match value {
                "true;" => legacy.with_config = true,
                "false;" => legacy.with_config = false,
                _ => unknown.push(line.to_string()),
            }
        } else if let Some(rest) = code.strip_prefix("pub const RENAMES: &[(&str, &str)] = &[") {
            let (list, closed) = match rest.strip_suffix("];") {
                Some(list) => (list, true),
                None => (rest, false),
            };
            match entries(list) {
                Some(found) => legacy.renames.extend(found),
                None => unknown.push(line.to_string()),
            }
            in_renames = !closed;
        } else {
            unknown.push(line.to_string());
        }
    }
    if unknown.is_empty() {
        Ok(legacy)
    } else {
        Err(unknown)
    }
}

/// `("a.b", "c"), ("d.e", ""),` as pairs, or `None` for anything else.
fn entries(text: &str) -> Option<Vec<(String, String)>> {
    let mut out = Vec::new();
    let mut rest = text.trim();
    loop {
        rest = rest.trim_start_matches([',', ' ']);
        if rest.is_empty() {
            return Some(out);
        }
        rest = rest.strip_prefix('(')?.trim_start();
        let (path, after) = literal(rest)?;
        rest = after.trim_start().strip_prefix(',')?.trim_start();
        let (name, after) = literal(rest)?;
        rest = after.trim_start().strip_prefix(')')?;
        out.push((path, name));
    }
}

/// A string literal without escapes at the start of `text`, and the rest.
fn literal(text: &str) -> Option<(String, &str)> {
    let body = text.strip_prefix('"')?;
    let end = body.find('"')?;
    let value = &body[..end];
    (!value.contains('\\')).then(|| (value.to_string(), &body[end + 1..]))
}

/// An old `src/flags.rs`, moved into module files.
#[derive(Debug, Default)]
pub struct Moved {
    /// The text of each module file that starts with a value of the old
    /// file in place of a default, by path.
    pub files: std::collections::BTreeMap<String, String>,
    /// One line per moved value, for the person to read.
    pub lines: Vec<String>,
    /// Each rename no module of `shapes` declares.
    pub stranded: Vec<String>,
}

/// Where each value of `legacy` goes in `shapes`.
pub fn move_legacy(legacy: &Legacy, shapes: &[Shape]) -> Moved {
    let mut moved = Moved::default();
    let mut choices: Vec<Choices> = vec![Choices::default(); shapes.len()];
    if legacy.with_config {
        if let Some(at) = shapes.iter().position(Shape::framework) {
            choices[at].flags_with_config = true;
            moved.lines.push(format!(
                "WITH_CONFIG = true -> flags_with_config: true in {}",
                path_of(FRAMEWORK)
            ));
        }
    }
    for (path, name) in &legacy.renames {
        match key_of(shapes, path) {
            Some((shape, variant)) => {
                let at = shapes
                    .iter()
                    .position(|s| s.key == shape.key)
                    .unwrap_or_default();
                choices[at].rename.push((variant.to_string(), name.clone()));
                moved.lines.push(format!(
                    "({path:?}, {name:?}) -> rename in {}",
                    path_of(&shape.key)
                ));
            }
            None => moved.stranded.push(format!("({path:?}, {name:?})")),
        }
    }
    for (shape, choices) in shapes.iter().zip(choices) {
        if choices != Choices::default() {
            moved
                .files
                .insert(path_of(&shape.key), module_file(shape, &choices));
        }
    }
    moved
}

/// The `Key::<Variant>` names a file uses outside comments.
pub fn keys_named(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let code = line.split_once("//").map_or(line, |(code, _)| code);
        let mut rest = code;
        while let Some(at) = rest.find("Key::") {
            let tail = &rest[at + 5..];
            let end = tail
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .unwrap_or(tail.len());
            if end > 0 && !out.iter().any(|k: &String| k == &tail[..end]) {
                out.push(tail[..end].to_string());
            }
            rest = &tail[end..];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dotted_key_is_one_word_per_part() {
        assert_eq!(camel("pool.max_size"), "PoolMaxSize");
        assert_eq!(camel("cookie"), "Cookie");
    }

    #[test]
    fn a_module_file_is_under_src_modules_and_never_the_list() {
        assert!(is_module_file("src/modules/sessions.rs"));
        assert!(is_module_file("src/modules/brocanth.rs"));
        assert!(!is_module_file("src/modules/mod.rs"));
        assert!(!is_module_file("src/plugins/sessions/mod.rs"));
        assert_eq!(
            path_of("middleware-rate-limit"),
            "src/modules/middleware_rate_limit.rs"
        );
        assert_eq!(path_of("static"), "src/modules/static.rs");
    }

    #[test]
    fn a_short_list_is_one_line_and_a_long_one_is_a_column() {
        assert_eq!(
            string_array("pub const ALL: &[&str] = &[", &["db"], "];"),
            "pub const ALL: &[&str] = &[\"db\"];\n"
        );
        let long = string_array(
            "pub const ALL: &[&str] = &[",
            &[
                "observability",
                "observability-health",
                "config",
                "sessions",
            ],
            "];",
        );
        assert!(long.contains("\n    \"observability\",\n"), "{long}");
    }

    /// The file the generator wrote before `src/modules/`, with a rename on
    /// a line of its own and a parked one.
    const OLD: &str = r#"//! The command-line flags of this binary.

/// Whether a project with a config file reads the flags too.
#[allow(dead_code)]
pub const WITH_CONFIG: bool = true;

/// Flags under another name.
///
/// ```text
/// ("sessions.cookie", "cookie"),
/// ```
pub const RENAMES: &[(&str, &str)] = &[
    ("sessions.cookie", "cookie"), // --cookie
    // Parked by `brocanth remove`; `brocanth add` restores it: ("jobs.workers", "w"),
];
"#;

    #[test]
    fn the_old_flags_file_is_read_with_its_parked_renames() {
        let legacy = read_legacy(OLD).expect("the generator's own form");
        assert!(legacy.with_config);
        assert_eq!(
            legacy.renames,
            [
                ("sessions.cookie".to_string(), "cookie".to_string()),
                ("jobs.workers".to_string(), "w".to_string())
            ]
        );
        let one = read_legacy("pub const RENAMES: &[(&str, &str)] = &[(\"a.b\", \"\")];\n")
            .expect("one line");
        assert_eq!(one.renames, [("a.b".to_string(), String::new())]);
    }

    #[test]
    fn anything_else_in_the_old_flags_file_comes_back_as_its_lines() {
        let text = format!("{OLD}\npub fn mine() {{}}\n");
        assert_eq!(
            read_legacy(&text),
            Err(vec!["pub fn mine() {}".to_string()])
        );
    }

    #[test]
    fn a_key_in_a_comment_is_not_named() {
        let text = "    rename: &[(Key::Cookie, Flag::As(\"c\"))], // Key::Secure\n    //   (Key::Site, Flag::Hidden),\n";
        assert_eq!(keys_named(text), ["Cookie"]);
    }
}
