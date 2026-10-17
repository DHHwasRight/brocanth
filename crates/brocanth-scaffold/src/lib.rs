//! The contract a scaffold module implements.
//!
//! `brocanth create` writes a project. What goes in it is a list of choices,
//! and this crate is how a choice that the CLI does not know about joins that
//! list. A module is a crate implementing [`Module`]; the CLI depends on it
//! and names it in one array; the capability is then on the menu.
//!
//! ```
//! use brocanth_scaffold::{snippet, Dep, Manifest, Module, Project, Scaffold, Slot};
//!
//! pub struct Cache;
//!
//! impl Module for Cache {
//!     fn manifest(&self) -> Manifest {
//!         Manifest::new("cache", "a cache in Postgres, on an unlogged table")
//!             .krate(Dep::brocanth("brocanth-cache"))
//!             .after("config")
//!     }
//!
//!     fn contribute(&self, project: &Project, s: &mut Scaffold) {
//!         s.file("src/plugins/cache.rs", "// ...\n");
//!         s.at(Slot::PluginsMod).line("pub mod cache;");
//!         s.at(Slot::AppPlugins)
//!             .block(snippet::plugin_line(project, "cache", "Cache"));
//!     }
//! }
//! ```
//!
//! # Why a crate rather than a plugin directory
//!
//! A scaffold module writes Rust, and the conditions it writes it under are
//! Rust conditions: what else was chosen, which database, whether this
//! project has a command line. A template language that could express that
//! would be a worse Rust, so the module is Rust, it is compiled into the CLI,
//! and a module that does not compile is not a module anybody ships.
//!
//! # Why the CLI still names every module
//!
//! A distributed slice (`inventory`, `linkme`) would let a module register
//! itself from anywhere in the dependency graph, and this crate deliberately
//! does not use one. ADR 1 in `PLAN.md` gives the reasons for the framework;
//! the first one applies here with more force, not less. Linking follows the
//! *transitive* graph, so one contributed module that happens to depend on
//! another would put a second entry on the menu that nobody added. The menu
//! is the receipt for what a generated binary contains. A receipt that grows
//! a line through a dependency is not a receipt.
//!
//! So adding somebody's module is two lines in the CLI -- a dependency and an
//! entry in the array -- and both are lines a reader can delete.
//!
//! # What a module may not do
//!
//! It writes files it owns, and it writes into [slots](Slot) the generator
//! published. It cannot edit a file another module wrote, invent a slot, or
//! see the filesystem. Two modules claiming one path is an error that names
//! both, rather than a last-writer-wins that depends on array order.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt;

/// A capability `brocanth create` can offer.
///
/// Implementors are usually unit structs: a module holds no state, because it
/// is asked once per run and everything it needs arrives in [`Project`].
pub trait Module: Sync {
    /// What this module is called, what it costs, and what it depends on.
    fn manifest(&self) -> Manifest;

    /// Write. Called only when this module was chosen, in dependency order.
    fn contribute(&self, project: &Project, scaffold: &mut Scaffold);
}

// -- what a module says about itself ----------------------------------------

/// The module's own description: everything the CLI needs before it decides
/// whether to call [`Module::contribute`].
#[derive(Debug, Clone)]
pub struct Manifest {
    /// The word on the command line and in the chooser. Lower case, and
    /// hyphenated when it is part of another choice: `rsx-tailwind`.
    pub key: &'static str,
    /// One line a reader can decide from. The right-hand half says what the
    /// binary pays, because that is the question the framework keeps asking.
    pub gist: &'static str,
    /// The choice this one is part of, for a choice that is part of one.
    ///
    /// The chooser nests it and hides it while the parent is off. The flags
    /// refuse it without the parent, so a flat command line still says what
    /// a screen says with an indent.
    pub parent: Option<&'static str>,
    /// Whether the chooser starts this ticked once its parent is. Ignored
    /// without a parent, and never applied to the flags: a flag that quietly
    /// brings two more is a flag nobody can read.
    pub on_with_parent: bool,
    /// Modules whose contributions must land before this one's.
    ///
    /// Declared rather than accidental. Two modules writing into one slot
    /// otherwise depend on the order of the array in the CLI, which is a
    /// thing nobody should have to know.
    pub after: Vec<&'static str>,
    /// What this choice puts in the generated project's dependency tree.
    pub crates: Vec<Dep>,
    /// Why this choice is part of its parent, in one sentence, for the
    /// diagnostic a flat command line produces when it is named alone.
    pub because: &'static str,
    /// What to say in the generated manifest when this choice was *not*
    /// taken: the name of the thing that is absent, and what its absence
    /// means. The receipt is read from both ends, so a project says what it
    /// does not contain as plainly as what it does.
    pub absent: Option<(&'static str, &'static str)>,
}

impl Manifest {
    /// A module with a key and a gist, and nothing else yet.
    pub fn new(key: &'static str, gist: &'static str) -> Self {
        Self {
            key,
            gist,
            parent: None,
            on_with_parent: false,
            after: Vec::new(),
            crates: Vec::new(),
            because: "",
            absent: None,
        }
    }

    /// Make this choice part of another, and ticked with it in the chooser.
    pub fn part_of(mut self, parent: &'static str) -> Self {
        self.parent = Some(parent);
        self.on_with_parent = true;
        self
    }

    /// Part of another choice, but off until it is asked for.
    pub fn part_of_but_off(mut self, parent: &'static str) -> Self {
        self.parent = Some(parent);
        self.on_with_parent = false;
        self
    }

    /// Contribute after another module, wherever both write.
    pub fn after(mut self, key: &'static str) -> Self {
        self.after.push(key);
        self
    }

    /// A dependency this choice adds to the generated project.
    pub fn krate(mut self, dep: Dep) -> Self {
        self.crates.push(dep);
        self
    }

    /// Why this choice needs its parent. One sentence, and the real reason:
    /// it is printed to somebody who typed the child's name on its own.
    pub fn because(mut self, why: &'static str) -> Self {
        self.because = why;
        self
    }

    /// What the generated manifest says when this choice was not taken.
    pub fn absent(mut self, what: &'static str, why: &'static str) -> Self {
        self.absent = Some((what, why));
        self
    }
}

/// A dependency of the *generated* project, not of the CLI.
///
/// A module says which crate it needs and leaves the source alone, because
/// where brocanth's own crates come from is one decision the CLI makes for
/// every module at once -- the published repository normally, and a local
/// checkout under `--brocanth-path`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dep {
    /// The crate name, as cargo spells it.
    pub name: &'static str,
    /// Where it comes from.
    pub source: Source,
    /// Features to turn on. Default features stay on; a module that needs
    /// them off says so with [`Dep::no_default_features`].
    pub features: Vec<&'static str>,
    /// Whether `default-features = false` is written.
    pub defaults: bool,
    /// A trailing comment on the dependency line. Short, and about why this
    /// crate is here rather than what it is.
    pub note: Option<&'static str>,
}

/// Where a generated project's dependency is fetched from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// One of brocanth's own crates. The CLI decides the source: the
    /// repository, or the local checkout `--brocanth-path` names.
    Brocanth,
    /// A git repository and a branch.
    Git {
        /// The clone URL.
        url: &'static str,
        /// The branch to track.
        branch: &'static str,
    },
    /// A version requirement from the registry.
    Version(&'static str),
}

impl Dep {
    /// One of brocanth's own crates.
    pub fn brocanth(name: &'static str) -> Self {
        Self::of(name, Source::Brocanth)
    }

    /// A crate from a git repository.
    pub fn git(name: &'static str, url: &'static str, branch: &'static str) -> Self {
        Self::of(name, Source::Git { url, branch })
    }

    /// A crate from the registry.
    pub fn version(name: &'static str, req: &'static str) -> Self {
        Self::of(name, Source::Version(req))
    }

    fn of(name: &'static str, source: Source) -> Self {
        Self {
            name,
            source,
            features: Vec::new(),
            defaults: true,
            note: None,
        }
    }

    /// Turn a feature on.
    pub fn feature(mut self, feature: &'static str) -> Self {
        self.features.push(feature);
        self
    }

    /// Write `default-features = false`.
    pub fn no_default_features(mut self) -> Self {
        self.defaults = false;
        self
    }

    /// A trailing comment on the dependency line.
    pub fn note(mut self, note: &'static str) -> Self {
        self.note = Some(note);
        self
    }
}

// -- what a module is told --------------------------------------------------

/// The answers, as a module may read them. Nothing here is writable: a module
/// contributes through [`Scaffold`], so what it changed is in one place.
pub struct Project<'a> {
    /// The cargo package name, which is also the directory.
    pub name: &'a str,
    /// The human name, for prose and page titles.
    pub title: &'a str,
    on: &'a [&'a str],
    known: &'a [&'a str],
    /// Keys asked about that no module declares. Collected rather than
    /// answered `false`, because a typo that reads as "the user did not want
    /// it" is a bug that produces a working project with a piece missing.
    unknown: std::cell::RefCell<Vec<String>>,
}

impl<'a> Project<'a> {
    /// Build the view a module sees. `on` is every chosen key and `known` is
    /// every key any registered module declares.
    pub fn new(name: &'a str, title: &'a str, on: &'a [&'a str], known: &'a [&'a str]) -> Self {
        Self {
            name,
            title,
            on,
            known,
            unknown: std::cell::RefCell::new(Vec::new()),
        }
    }

    /// Whether another choice is part of this project.
    pub fn has(&self, key: &str) -> bool {
        if !self.known.contains(&key) {
            self.unknown.borrow_mut().push(key.to_string());
            return false;
        }
        self.on.contains(&key)
    }

    /// `my-blog` -> `my_blog`: the crate identifier, and the prefix of every
    /// environment variable the generated binary reads.
    pub fn ident(&self) -> String {
        self.name.replace('-', "_")
    }

    /// Every key asked about that no module declares, for the CLI to report.
    pub fn unknown_keys(&self) -> Vec<String> {
        self.unknown.borrow().clone()
    }
}

// -- where a module may write -----------------------------------------------

/// A place in a file the generator owns where a module may add lines.
///
/// A closed list, deliberately. The generator publishes seams rather than
/// letting a module edit a file it did not write, which is the same rule the
/// framework's core follows for plugins. A module that needs a seam this list
/// does not have needs a change here, in the open, with a name and a comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Slot {
    /// `Cargo.toml`, after the dependencies the choices brought. For a
    /// `[features]` table or anything else a dependency line cannot say.
    CargoExtra,
    /// `src/app.rs`, the `use` lines at the top. Use
    /// [`Writer::once`](crate::Writer::once), because two modules needing
    /// one import is the normal case and `use` twice does not compile.
    AppUses,
    /// `src/app.rs`, inside the `AppBuilder` chain in `build`. One
    /// `.method(..)` per line, no trailing semicolon.
    AppPlugins,
    /// `src/app.rs`, inside `setup`, after the builder is done. Where a
    /// middleware offer is mounted, at a pattern the application chooses.
    AppSetup,
    /// `src/app.rs`, inside `setup`, one line per module that provides a
    /// value on the per-request context.
    ///
    /// The generator writes `let cx = app.context();` above whatever lands
    /// here, and only when something does. The lines are a demonstration
    /// and they are load-bearing: the installed provider set lives in the
    /// application's *type*, so an `extract` for a plugin nobody registered
    /// does not compile. That is the modularity claim, checked by the
    /// compiler rather than written in a README.
    AppContext,
    /// `src/app.rs`, free items beside the dispatcher.
    AppItems,
    /// `src/app.rs`, inside `dispatch`, before the router is consulted. For
    /// an answer that is not a route, such as a static file.
    AppBeforeRouter,
    /// `src/plugins/mod.rs`, one `pub mod` line per plugin.
    PluginsMod,
    /// `src/pages/mod.rs`, items the pages share.
    PagesItems,
    /// `config/<env>.toml`, one section per module that claims one.
    ConfigSections,
    /// `.gitignore`, one path per line.
    GitIgnore,
    /// `mise.toml`, one `[tasks.x]` table per task.
    MiseTasks,
    /// `package.json`, dependencies for a project that needs node.
    NodeDependencies,
    /// `AGENTS.md`, one `##` section per module that has a rule a coding
    /// agent must follow.
    AgentsSections,
}

impl fmt::Display for Slot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Slot::CargoExtra => "Cargo.toml",
            Slot::AppUses => "src/app.rs (uses)",
            Slot::AppPlugins => "src/app.rs (the builder chain)",
            Slot::AppSetup => "src/app.rs (setup)",
            Slot::AppContext => "src/app.rs (the context)",
            Slot::AppItems => "src/app.rs (items)",
            Slot::AppBeforeRouter => "src/app.rs (before the router)",
            Slot::PluginsMod => "src/plugins/mod.rs",
            Slot::PagesItems => "src/pages/mod.rs",
            Slot::ConfigSections => "config/<env>.toml",
            Slot::GitIgnore => ".gitignore",
            Slot::MiseTasks => "mise.toml",
            Slot::NodeDependencies => "package.json",
            Slot::AgentsSections => "AGENTS.md",
        };
        f.write_str(name)
    }
}

/// A second thing to compile: another `[[bin]]` in the generated manifest.
///
/// A backoffice that is its own binary is the case this exists for. The
/// module writes the source file as usual and declares the target here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binary {
    /// The binary's name, and so the command that runs it.
    pub name: String,
    /// The source file, relative to the project root.
    pub path: String,
}

// -- what a module writes ---------------------------------------------------

/// Everything the modules contributed, and the complaints collected on the
/// way.
///
/// The CLI makes one of these, hands it to each chosen module in order, and
/// then assembles the files it owns around what came back.
#[derive(Debug, Default)]
pub struct Scaffold {
    files: BTreeMap<String, Owned>,
    slots: BTreeMap<Slot, Vec<Owned>>,
    bins: Vec<(String, Binary)>,
    problems: Vec<String>,
    writing: String,
}

#[derive(Debug, Clone)]
struct Owned {
    by: String,
    text: String,
}

impl Scaffold {
    /// A new, empty one.
    pub fn new() -> Self {
        Self::default()
    }

    /// Say which module is writing. The CLI calls this before each module, so
    /// a complaint can name the module rather than the line.
    pub fn now_writing(&mut self, key: &str) {
        self.writing = key.to_string();
    }

    /// Write a whole file the module owns.
    ///
    /// Two modules claiming one path is a problem rather than a last writer:
    /// which one won would otherwise depend on the order of an array in the
    /// CLI, and the loser's contribution would vanish in silence.
    pub fn file(&mut self, path: impl Into<String>, contents: impl Into<String>) {
        let path = path.into();
        let mine = Owned {
            by: self.writing.clone(),
            text: contents.into(),
        };
        if let Some(theirs) = self.files.get(&path) {
            self.problems.push(format!(
                "`{}` and `{}` both write {path}",
                theirs.by, mine.by
            ));
            return;
        }
        self.files.insert(path, mine);
    }

    /// Add to a slot. Returns a writer so several lines read as several
    /// lines:
    ///
    /// ```
    /// # use brocanth_scaffold::{Scaffold, Slot};
    /// # let mut s = Scaffold::new();
    /// s.at(Slot::PluginsMod).line("pub mod cache;");
    /// ```
    pub fn at(&mut self, slot: Slot) -> Writer<'_> {
        Writer {
            scaffold: self,
            slot,
        }
    }

    /// Declare a second compile target.
    pub fn binary(&mut self, name: impl Into<String>, path: impl Into<String>) {
        let bin = Binary {
            name: name.into(),
            path: path.into(),
        };
        if let Some((by, other)) = self.bins.iter().find(|(_, b)| b.name == bin.name) {
            self.problems.push(format!(
                "`{}` and `{}` both want a binary called `{}`",
                by, self.writing, other.name
            ));
            return;
        }
        self.bins.push((self.writing.clone(), bin));
    }

    /// Complain about something only this module can know is wrong. The CLI
    /// prints it and writes nothing.
    pub fn refuse(&mut self, why: impl Into<String>) {
        let why = why.into();
        self.problems.push(if self.writing.is_empty() {
            why
        } else {
            format!("`{}`: {why}", self.writing)
        });
    }

    /// Every file the modules wrote, by path.
    pub fn files(&self) -> impl Iterator<Item = (&str, &str)> {
        self.files
            .iter()
            .map(|(path, owned)| (path.as_str(), owned.text.as_str()))
    }

    /// What landed in one slot, in the order the modules ran.
    pub fn slot(&self, slot: Slot) -> impl Iterator<Item = &str> {
        self.slots
            .get(&slot)
            .into_iter()
            .flatten()
            .map(|owned| owned.text.as_str())
    }

    /// Which modules wrote into a slot, in the order they ran.
    ///
    /// For the few questions that are about the module rather than the
    /// text: which plugin instances exist, and therefore which config
    /// sections something will claim.
    pub fn slot_owners(&self, slot: Slot) -> impl Iterator<Item = &str> {
        self.slots
            .get(&slot)
            .into_iter()
            .flatten()
            .map(|owned| owned.by.as_str())
    }

    /// Whether anything landed in a slot, without building the strings.
    pub fn slot_is_empty(&self, slot: Slot) -> bool {
        self.slots.get(&slot).is_none_or(Vec::is_empty)
    }

    /// Every second binary a module asked for.
    pub fn binaries(&self) -> impl Iterator<Item = &Binary> {
        self.bins.iter().map(|(_, bin)| bin)
    }

    /// Everything that was wrong, in the order it was found.
    pub fn problems(&self) -> &[String] {
        &self.problems
    }
}

/// The handle [`Scaffold::at`] returns. Each call appends one contribution.
pub struct Writer<'a> {
    scaffold: &'a mut Scaffold,
    slot: Slot,
}

impl Writer<'_> {
    /// One line. A newline is added, and is not wanted in the argument.
    pub fn line(&mut self, text: impl AsRef<str>) -> &mut Self {
        let text = text.as_ref();
        self.push(format!("{text}\n"))
    }

    /// One line, unless an earlier module already wrote exactly it.
    ///
    /// For an import. Two modules both needing
    /// `use brocanth::plugin_api::Plugin;` is the ordinary case, and a file
    /// with it twice does not compile. Deliberately exact-match: two lines
    /// that differ by a space are two lines, because a slot cannot know
    /// what a near-miss meant.
    pub fn once(&mut self, text: impl AsRef<str>) -> &mut Self {
        let line = format!("{}\n", text.as_ref());
        let already = self
            .scaffold
            .slots
            .get(&self.slot)
            .is_some_and(|lines| lines.iter().any(|owned| owned.text == line));
        if already {
            return self;
        }
        self.push(line)
    }

    /// Several lines, written as they are. Use this for a block that already
    /// ends in a newline, such as a TOML section or a Markdown heading.
    pub fn block(&mut self, text: impl Into<String>) -> &mut Self {
        self.push(text.into())
    }

    fn push(&mut self, text: String) -> &mut Self {
        let by = self.scaffold.writing.clone();
        self.scaffold
            .slots
            .entry(self.slot)
            .or_default()
            .push(Owned { by, text });
        self
    }
}

// -- the registry -----------------------------------------------------------

/// Put the chosen modules in the order they must contribute.
///
/// A module's `after` list is a promise the CLI keeps, so two modules writing
/// into one slot do not depend on the order of the array they were listed in.
/// Within that, the given order is kept, so a reader of the array can predict
/// the output.
///
/// Returns the keys in order, or the keys caught in a cycle.
pub fn order<'a>(
    keys: &[&'a str],
    after: impl Fn(&str) -> Vec<&'static str>,
) -> Result<Vec<&'a str>, Vec<&'a str>> {
    let mut done: Vec<&str> = Vec::with_capacity(keys.len());
    let mut left: Vec<&str> = keys.to_vec();
    while !left.is_empty() {
        // One pass. Anything whose predecessors are all placed, or are not
        // part of this project at all, can go now.
        let ready: Vec<&str> = left
            .iter()
            .copied()
            .filter(|key| {
                after(key)
                    .iter()
                    .all(|need| !keys.contains(need) || done.contains(need))
            })
            .collect();
        if ready.is_empty() {
            return Err(left);
        }
        left.retain(|key| !ready.contains(key));
        done.extend(ready);
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Noop;
    impl Module for Noop {
        fn manifest(&self) -> Manifest {
            Manifest::new("noop", "nothing")
        }
        fn contribute(&self, _: &Project, _: &mut Scaffold) {}
    }

    #[test]
    fn a_module_is_object_safe_so_the_registry_can_be_an_array() {
        // The whole design rests on this: one array of trait objects, one
        // line per module, and a line a reader can delete.
        let all: &[&dyn Module] = &[&Noop];
        assert_eq!(all[0].manifest().key, "noop");
    }

    #[test]
    fn two_modules_writing_one_path_is_a_problem_naming_both() {
        let mut s = Scaffold::new();
        s.now_writing("cache");
        s.file("src/plugins/thing.rs", "// cache\n");
        s.now_writing("jobs");
        s.file("src/plugins/thing.rs", "// jobs\n");
        assert_eq!(s.problems().len(), 1);
        let said = &s.problems()[0];
        assert!(said.contains("cache") && said.contains("jobs"), "{said}");
        // And the first writer still has its file: a problem stops the run,
        // so the second write must not have silently replaced anything.
        assert_eq!(
            s.files().next(),
            Some(("src/plugins/thing.rs", "// cache\n"))
        );
    }

    #[test]
    fn a_slot_keeps_the_order_the_modules_ran_in() {
        let mut s = Scaffold::new();
        s.now_writing("logging");
        s.at(Slot::PluginsMod).line("pub mod logging;");
        s.now_writing("sessions");
        s.at(Slot::PluginsMod).line("pub mod sessions;");
        let lines: Vec<&str> = s.slot(Slot::PluginsMod).collect();
        assert_eq!(lines, ["pub mod logging;\n", "pub mod sessions;\n"]);
    }

    #[test]
    fn asking_about_a_key_nobody_declares_is_reported_not_answered() {
        // `has("tailwind")` after the key became `rsx-tailwind` would
        // otherwise read as "the user did not want it", and the project
        // would build with a piece missing and no complaint anywhere.
        let known = ["rsx", "rsx-tailwind"];
        let on = ["rsx"];
        let p = Project::new("blog", "Blog", &on, &known);
        assert!(p.has("rsx"));
        assert!(!p.has("rsx-tailwind"));
        assert!(p.unknown_keys().is_empty());
        assert!(!p.has("tailwind"));
        assert_eq!(p.unknown_keys(), ["tailwind"]);
    }

    #[test]
    fn order_puts_a_module_after_the_one_it_named() {
        let keys = ["cache", "config"];
        let after = |key: &str| match key {
            "cache" => vec!["config"],
            _ => vec![],
        };
        assert_eq!(order(&keys, after).expect("no cycle"), ["config", "cache"]);
    }

    #[test]
    fn order_ignores_a_predecessor_this_project_does_not_have() {
        // `after` is "if both are here, this one second", not a dependency.
        // A module that needs another to exist says so with `krate`.
        let keys = ["cache"];
        let after = |_: &str| vec!["config"];
        assert_eq!(order(&keys, after).expect("no cycle"), ["cache"]);
    }

    #[test]
    fn a_cycle_is_returned_rather_than_hung_on() {
        let keys = ["a", "b"];
        let after = |key: &str| match key {
            "a" => vec!["b"],
            "b" => vec!["a"],
            _ => vec![],
        };
        let caught = order(&keys, after).expect_err("a and b wait for each other");
        assert_eq!(caught.len(), 2);
    }
}

/// Fragments a module writes into the project it is scaffolding.
///
/// Not sugar: each one is a place where two modules must agree, and the
/// agreement belongs in one function rather than in each of them.
pub mod snippet {
    use super::Project;

    /// The line that registers a plugin instance in the application.
    ///
    /// Two forms, and which one is right is not the module's choice: with
    /// the config plugin the instance is built from a named section, and
    /// without it `main` hands it a literal the compiler checks.
    ///
    /// It is here, and not written out in each module, for a duller reason
    /// than that. The literal form is longer than rustfmt's `chain_width`,
    /// so it has to arrive already broken across three lines or a
    /// developer's first `cargo fmt` rewrites a file they never touched.
    /// That is a fact about rustfmt, not about caching or logging, and
    /// every module getting it right separately is every module getting it
    /// wrong once.
    ///
    /// `key` is the module's key, which is also the plugin's module name
    /// and its config section. `ty` is the plugin type in that module.
    pub fn plugin_line(project: &Project, key: &str, ty: &str) -> String {
        if project.has("config") {
            format!(
                "        .plugin_from_config::<plugins::{key}::{ty}>({key:?})\n\
                 \x20       .map_err(|e| e.to_string())?\n"
            )
        } else {
            format!(
                "        .plugin(plugins::{key}::{ty}::new(\n\
                 \x20           plugins::{key}::Settings::default(),\n\
                 \x20       ))\n"
            )
        }
    }

    /// The import the literal form needs, and the configured form does not.
    ///
    /// `Plugin::new` is a trait method, so a file that calls it must have
    /// the trait in scope; `plugin_from_config` is the builder's own method
    /// and needs nothing. Write it with
    /// [`Writer::once`](crate::Writer::once) into
    /// [`Slot::AppUses`](crate::Slot::AppUses), because every plugin module
    /// needs the same line and a file with it twice does not compile.
    pub const PLUGIN_TRAIT_USE: &str = "use brocanth::plugin_api::Plugin;";

    /// The derive a settings struct carries, and the sentence explaining it.
    ///
    /// This is the modularity claim at its sharpest. With the config plugin
    /// the struct is deserialised and derives for it; without, nothing
    /// deserialises anything and the derive is not there to be compiled. A
    /// module that wrote `serde::Deserialize` unconditionally would put a
    /// parser in the tree of a project that asked for none.
    ///
    /// Returns the derive lines, and a sentence for the struct's own doc
    /// comment that says why they are what they are.
    pub fn settings_derive(project: &Project, section: &str) -> (String, String) {
        if project.has("config") {
            (
                "#[derive(Debug, serde::Deserialize)]\n#[serde(default, deny_unknown_fields)]"
                    .to_string(),
                format!(
                    "It derives `Deserialize` because this application reads it from\n\
                     /// `[{section}]` in a TOML file. Drop the config plugin and the derive goes\n\
                     /// with it: `Plugin::Config` carries no bounds the core imposes."
                ),
            )
        } else {
            (
                "#[derive(Debug)]".to_string(),
                "Nothing deserialises it: `main` builds it from a literal, and the\n\
                 /// compiler checks that literal. `Plugin::Config` carries no bounds the\n\
                 /// core imposes, so there is no deserialiser here to link."
                    .to_string(),
            )
        }
    }
}
