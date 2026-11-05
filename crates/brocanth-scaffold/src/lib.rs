// The README comes first: what the crate is for, the contract in short, and
// examples. The `//!` notes below keep the design reasons.
#![doc = include_str!("../README.md")]
//!
//! # Design
//!
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
//!         s.file("src/plugins/cache/mod.rs", "// ...\n");
//!         s.at(Slot::PluginsMod).line("pub mod cache;");
//!         // Writes the chain line *and* the provider list, which have
//!         // to agree. `plugin_line` alone writes only the first.
//!         snippet::plugin(s, project, "cache", "Cache");
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

#[cfg(feature = "testing")]
pub mod testing;

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
    /// Modules this one cannot be written without.
    ///
    /// The third relation, and not either of the other two. [`after`] is
    /// order between two modules that were both chosen. [`parent`] is one
    /// choice drawn inside another on a screen. This one is a requirement:
    /// `rsx-tailwind` writes a subcommand, and a project with no command
    /// line has nowhere to put it.
    ///
    /// The flags refuse a pick whose needs are missing, and name the fix.
    /// The chooser ticks what a tick requires, and unticks what an untick
    /// leaves without. So neither route can produce a project that does not
    /// compile.
    ///
    /// Each entry carries its own reason, because it is a different
    /// question from [`because`] -- that one says why a choice sits inside
    /// another on a screen, this one says what breaks without it.
    ///
    /// [`after`]: Manifest::after
    /// [`parent`]: Manifest::parent
    /// [`because`]: Manifest::because
    pub needs: Vec<(&'static str, &'static str)>,
    /// What this choice puts in the generated project's dependency tree.
    pub crates: Vec<Dep>,
    /// Why this choice is part of its parent, in one sentence, for the
    /// diagnostic a flat command line produces when it is named alone.
    pub because: &'static str,
    /// Why this choice cannot be taken yet, for a module that is on the
    /// menu and not available.
    ///
    /// A listed refusal rather than silence. The generated manifest already
    /// says what a project does not contain and why; a chooser that hides
    /// the two transports realtime has no core support for answers the same
    /// question with nothing, and the person asks it again next month. So
    /// the row is drawn, the reason is on it, and the cursor steps over it.
    ///
    /// [`Module::contribute`] is never called for one. Implement it as a
    /// refusal anyway: a caller that does not honour this field should stop
    /// rather than write half a project.
    pub blocked: Option<&'static str>,
    /// What to say in the generated manifest when this choice was *not*
    /// taken: the name of the thing that is absent, and what its absence
    /// means. The receipt is read from both ends, so a project says what it
    /// does not contain as plainly as what it does.
    pub absent: Option<(&'static str, &'static str)>,
    /// Which of the chooser's questions asks about this module.
    ///
    /// The chooser has a guided route that asks one question per screen:
    /// where data lives, how HTML is made, who gets in. A module says which
    /// question it answers, because that is knowledge about the module and
    /// not about the menu -- and because a module from another repository
    /// must be able to join a question without editing the CLI.
    ///
    /// Empty means the module did not say. It is still asked about, in a
    /// question at the end for everything that named no other.
    pub section: &'static str,
    /// The shorter phrasing the guided route uses, for a module whose
    /// [`gist`] is written for the flat list.
    ///
    /// Two voices for one module, because the two screens ask different
    /// questions. The flat list sets nineteen rows beside each other, so a
    /// row has to say what it costs: "a cache on an unlogged Postgres
    /// table, not Redis". One question on one screen has already said that
    /// data is the subject, so the row can answer it: "a cache, in the
    /// database you already have".
    ///
    /// Empty falls back to the gist, which is what most modules want.
    ///
    /// [`gist`]: Manifest::gist
    pub short: &'static str,
    /// Whether this module adds nothing that only a running server uses: a
    /// build step, a template engine, a CI file, the config file, the
    /// command line. Set with [`Manifest::outside_runtime`].
    ///
    /// A project whose output is files and not a process takes only these.
    pub outside_runtime: bool,
    /// Whether this module turns the project into files rather than a
    /// server. Set with [`Manifest::static_output`].
    ///
    /// With one of these chosen, every other choice must be
    /// [`outside_runtime`](Manifest::outside_runtime): there is no process
    /// to run the rest in.
    pub static_output: bool,
    /// Which kinds of seam this module hooks into. Set with
    /// [`Manifest::category`], once per category; the first is the main
    /// one.
    ///
    /// A set, because a module can be two things: `observability` wraps
    /// requests and owns probe routes. A test holds every module to the
    /// slots its categories open ([`Slot::open_to`]), so the category is a
    /// fact about the code and not a label somebody chose.
    pub categories: Vec<Category>,
    /// Modules this one cannot be chosen beside, each with the reason.
    /// Set with [`Manifest::conflicts`].
    ///
    /// Declared on one side and read from both: `create`, `add` and the
    /// chooser all ask [`clash`], which looks in either manifest. The rule
    /// between a static site and a server module is the same question, so
    /// [`clash`] answers it too.
    pub conflicts: Vec<(&'static str, &'static str)>,
    /// Modules this one writes different code beside, each with what
    /// changes. Set with [`Manifest::adapts_to`].
    ///
    /// The optional dependency: neither required ([`needs`]) nor ordered
    /// ([`after`]), but read. Every `project.has(key)` in
    /// [`Module::contribute`] names a key declared here, in `needs`, or the
    /// parent; a test in the CLI runs every module and holds it to that.
    ///
    /// [`needs`]: Manifest::needs
    /// [`after`]: Manifest::after
    pub adapts_to: Vec<(&'static str, &'static str)>,
    /// Modules a deployed project with this one usually wants, each with
    /// the reason. Set with [`Manifest::recommends`].
    ///
    /// Advice, and the weakest of the relations. [`needs`] refuses a
    /// selection; this one only speaks. `create` and `add` print the
    /// recommended modules that were not chosen, `brocanth check` warns
    /// about them, and `--questions` lists the edges, so a person (or the
    /// agent that asks them) sees the choice before the deploy does.
    ///
    /// [`needs`]: Manifest::needs
    pub recommends: Vec<(&'static str, &'static str)>,
    /// The version of the code that writes this module, when it is not the
    /// CLI's own. Set with [`Manifest::version`].
    ///
    /// `brocanth.toml` records it per module, and `brocanth check` says when
    /// the installed CLI writes a module at another version than the one
    /// that wrote the project. `None` is the CLI's version: every crate in
    /// the brocanth workspace takes the workspace version, so a module that
    /// lives there writes at the CLI's version by construction. A module
    /// from another repository sets it, with its own crate's version.
    pub version: Option<&'static str>,
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
            needs: Vec::new(),
            crates: Vec::new(),
            because: "",
            blocked: None,
            absent: None,
            section: "",
            short: "",
            outside_runtime: false,
            static_output: false,
            categories: Vec::new(),
            conflicts: Vec::new(),
            adapts_to: Vec::new(),
            recommends: Vec::new(),
            version: None,
        }
    }

    /// The version of the code that writes this module. Pass
    /// `env!("CARGO_PKG_VERSION")`, so the number is the crate's own.
    ///
    /// Only a module outside the brocanth workspace needs it: inside, every
    /// crate takes the workspace version, which is the CLI's.
    pub fn version(mut self, version: &'static str) -> Self {
        self.version = Some(version);
        self
    }

    /// One of the kinds of seam this module hooks into. Call it once per
    /// category; the first is the main one, and lists show it first.
    pub fn category(mut self, category: Category) -> Self {
        if !self.categories.contains(&category) {
            self.categories.push(category);
        }
        self
    }

    /// Refuse to be chosen beside another module, and say why.
    ///
    /// `why` is printed to whoever picked both, so it says what the two
    /// would do twice. Declare it on one side, the side that gives way:
    /// [`clash`] reads both manifests and tells the user to drop this one.
    pub fn conflicts(mut self, key: &'static str, why: &'static str) -> Self {
        self.conflicts.push((key, why));
        self
    }

    /// Write different code when another module is chosen, and say what
    /// changes. Declare one for every key `contribute` asks
    /// [`Project::has`] about that is not a need or the parent.
    pub fn adapts_to(mut self, key: &'static str, what_changes: &'static str) -> Self {
        self.adapts_to.push((key, what_changes));
        self
    }

    /// Advise another module beside this one, and say what goes wrong
    /// without it.
    ///
    /// `why` is printed to whoever left the other module out, so it names
    /// the consequence in production, not the arrow. Never refuses: a
    /// project without the other module still builds and runs.
    pub fn recommends(mut self, key: &'static str, why: &'static str) -> Self {
        self.recommends.push((key, why));
        self
    }

    /// Every key this module may ask [`Project::has`] about: what it adapts
    /// to, what it needs, and its parent.
    pub fn may_ask(&self) -> Vec<&'static str> {
        self.adapts_to
            .iter()
            .chain(&self.needs)
            .map(|(key, _)| *key)
            .chain(self.parent)
            .collect()
    }

    /// This module adds nothing that only a running server uses, so a
    /// static site may take it.
    pub fn outside_runtime(mut self) -> Self {
        self.outside_runtime = true;
        self
    }

    /// This module compiles the project to files. Every other module chosen
    /// with it must be [`outside_runtime`](Manifest::outside_runtime).
    pub fn static_output(mut self) -> Self {
        self.static_output = true;
        self
    }

    /// The guided route's question this module answers, and the phrasing it
    /// uses there.
    ///
    /// Both at once, because a module that joins a question nearly always
    /// wants the shorter voice that question allows. Pass an empty `short`
    /// to keep the gist.
    pub fn asked_in(mut self, section: &'static str, short: &'static str) -> Self {
        self.section = section;
        self.short = short;
        self
    }

    /// What the guided route puts on this module's row.
    pub fn phrasing(&self) -> &'static str {
        if self.short.is_empty() {
            self.gist
        } else {
            self.short
        }
    }

    /// Mark this choice as listed and not available, and say why in a
    /// phrase that fits on its row.
    pub fn blocked(mut self, why: &'static str) -> Self {
        self.blocked = Some(why);
        self
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

    /// Refuse to be written without another module, and contribute after it.
    ///
    /// `why` is printed to whoever named this choice alone, so it says what
    /// breaks: not "it depends on commands" but what this module writes
    /// that a project without one has nowhere to put.
    ///
    /// Requiring implies ordering: a module that cannot exist without
    /// another writes into whatever that one put there, so declaring both
    /// separately would only let them disagree.
    pub fn needs(mut self, key: &'static str, why: &'static str) -> Self {
        self.needs.push((key, why));
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

/// The five kinds of seam a module hooks into. `docs/module-map.md` draws
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    /// Runs around a request, and may put a value on the context.
    Middleware,
    /// Makes a response body: a file type in the page router, or a
    /// function the pages call.
    Renderer,
    /// Touches no code: builds, ships or checks the project.
    External,
    /// Adds a capability to the codebase: a plugin with a lifecycle, a
    /// subcommand.
    Enhancer,
    /// Answers at a path the application chooses.
    Routes,
}

impl Category {
    /// All five, in the order lists print them.
    pub const ALL: [Category; 5] = [
        Category::Middleware,
        Category::Renderer,
        Category::External,
        Category::Enhancer,
        Category::Routes,
    ];

    /// The word lists and `brocanth.toml` use.
    pub fn name(self) -> &'static str {
        match self {
            Category::Middleware => "middleware",
            Category::Renderer => "renderer",
            Category::External => "external",
            Category::Enhancer => "enhancer",
            Category::Routes => "routes",
        }
    }

    /// Whether a module of this category may write into `slot`.
    pub fn allows(self, slot: Slot) -> bool {
        let open = slot.open_to();
        open.is_empty() || open.contains(&self)
    }
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Why two modules cannot both be chosen: what to print, and about which.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clash {
    /// The module the diagnostic is about: the one to drop.
    pub about: &'static str,
    /// One line, naming both.
    pub message: String,
    /// The reason, from a manifest or from the static-site rule.
    pub why: String,
    /// What to do, without the flag: the CLI adds where to do it.
    pub help: String,
}

/// Why `a` and `b` cannot both be chosen, or `None` when they can.
///
/// The one rule `create`, `add` and the chooser all ask, so the three
/// cannot disagree. Two sources:
///
/// * a [`Manifest::conflicts`] entry, in either manifest;
/// * the static-site rule: a module that compiles the project to files
///   ([`Manifest::static_output`]) takes only modules that run outside a
///   server ([`Manifest::outside_runtime`]). It is a rule over two flags
///   rather than a list, so a module from another repository follows it by
///   saying what it is, without naming every module it excludes.
pub fn clash(a: &Manifest, b: &Manifest) -> Option<Clash> {
    let declared = a
        .conflicts
        .iter()
        .find(|(key, _)| *key == b.key)
        .map(|(_, why)| (a.key, b.key, *why))
        .or_else(|| {
            b.conflicts
                .iter()
                .find(|(key, _)| *key == a.key)
                .map(|(_, why)| (b.key, a.key, *why))
        });
    if let Some((declarer, other, why)) = declared {
        return Some(Clash {
            about: declarer,
            message: format!("`{declarer}` and `{other}` cannot both be chosen"),
            why: why.to_string(),
            help: format!("keep `{other}` and drop `{declarer}`"),
        });
    }
    let needs_server = |m: &Manifest| !m.outside_runtime && !m.static_output;
    let (site, server) = if a.static_output && needs_server(b) {
        (a, b)
    } else if b.static_output && needs_server(a) {
        (b, a)
    } else {
        return None;
    };
    Some(Clash {
        about: server.key,
        message: format!(
            "`{}` needs a running server, and `{}` compiles to files",
            server.key, site.key
        ),
        why: "a static site takes only modules outside the runtime: templates, CSS, config, \
              commands, CI"
            .to_string(),
        help: format!(
            "drop `{}`, or drop `{}` to build a server",
            server.key, site.key
        ),
    })
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
    /// Features turned on only when another module is chosen, as
    /// `(module key, feature)`. Set with [`Dep::feature_with`].
    pub with: Vec<(&'static str, &'static str)>,
    /// Whether `default-features = false` is written.
    pub defaults: bool,
    /// A trailing comment on the dependency line. Short, and about why this
    /// crate is here rather than what it is.
    pub note: Option<&'static str>,
    /// Whether it goes in `[build-dependencies]`: the build script uses it,
    /// the binary does not. Set with [`Dep::for_build`].
    pub build: bool,
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
            with: Vec::new(),
            defaults: true,
            note: None,
            build: false,
        }
    }

    /// Turn a feature on.
    pub fn feature(mut self, feature: &'static str) -> Self {
        self.features.push(feature);
        self
    }

    /// Turn a feature on when the project also has the module `key`.
    ///
    /// A manifest is static, and some features exist only for another
    /// module: `brocanth-middleware`'s `config` derives `Deserialize` on
    /// its settings, which only the config file reads. Without `config`
    /// the feature would put a parser in a project that asked for none.
    pub fn feature_with(mut self, key: &'static str, feature: &'static str) -> Self {
        self.with.push((key, feature));
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

    /// Put it in `[build-dependencies]`, for a `build.rs` the module writes.
    ///
    /// One crate may be both: `brocanth-i18n` is a dependency of the binary
    /// with its runtime and a build dependency with its compiler. Cargo
    /// resolves the two feature sets apart, so the binary links no TOML
    /// parser and the build script links no CLDR data.
    pub fn for_build(mut self) -> Self {
        self.build = true;
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
    /// Every key asked about, in order, for the test that holds a module
    /// to its [`Manifest::may_ask`].
    asked: std::cell::RefCell<Vec<String>>,
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
            asked: std::cell::RefCell::new(Vec::new()),
        }
    }

    /// Whether another choice is part of this project.
    pub fn has(&self, key: &str) -> bool {
        self.asked.borrow_mut().push(key.to_string());
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

    /// Every key asked about through [`Project::has`], in order.
    pub fn asked_keys(&self) -> Vec<String> {
        self.asked.borrow().clone()
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
///
/// A variant needs a reader. Three had none -- `CargoExtra`, `AppItems` and
/// `AppBeforeRouter` -- so a module that wrote into one was recorded and
/// discarded in silence. They were deleted rather than read, because no
/// module had a use for them: `src/app.rs` is the developer's file, so a
/// module writes its own file under `src/plugins/<key>/` instead, and an
/// answer before the router is a route in the one table
/// ([`Slot::RouterMounts`]). Add one back with its first reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Slot {
    /// The `use` lines the registration chain needs. Use
    /// [`Writer::once`](crate::Writer::once), because two modules needing
    /// one import is the normal case and `use` twice does not compile.
    ///
    /// They land inside `plugins::register!`, in `src/plugins/mod.rs`, and
    /// the macro expands in `src/app.rs`. The name is the old one because
    /// renaming it would break every module outside this repository for a
    /// word, and the slot still means what it meant: whatever the builder
    /// chain needs in scope.
    AppUses,
    /// Inside the `AppBuilder` chain. One `.method(..)` per line, no
    /// trailing semicolon.
    ///
    /// The chain is `plugins::register!` in `src/plugins/mod.rs`, which is
    /// a file the generator rewrites. It used to be written straight into
    /// `src/app.rs`, and that put a generated chain in the one file the
    /// developer owns, so `brocanth add` could not touch it.
    AppPlugins,
    /// `src/app.rs`, inside `setup`, after the builder is done. Where a
    /// middleware offer is mounted, at a pattern the application chooses.
    AppSetup,
    /// One line per module that provides a value on the per-request
    /// context.
    ///
    /// They land inside `plugins::announce!`, in `src/plugins/mod.rs`. The
    /// generator writes `let cx = $app.context();` above them. A path a
    /// module writes here is read in `src/app.rs`, where the macro
    /// expands, so spell it from the crate root: `crate::plugins::..`.
    ///
    /// The lines are a demonstration
    /// and they are load-bearing: the installed provider set lives in the
    /// application's *type*, so an `extract` for a plugin nobody registered
    /// does not compile. That is the modularity claim, checked by the
    /// compiler rather than written in a README.
    AppContext,
    /// `src/plugins/mod.rs`, one `pub mod` line per plugin.
    PluginsMod,
    /// One plugin type path per registered plugin, in registration order:
    /// `crate::plugins::logging::Logging`.
    ///
    /// The generated `App` is a written struct, and one of its fields is a
    /// context whose type carries the provider list. That list is one
    /// `Provided::Install` per registered plugin, in the order
    /// `plugins::register!` applies them, and it has to be *the same* list
    /// or the two types do not match.
    ///
    /// So it is not written by hand. [`snippet::plugin`] writes the chain
    /// line and this one together, which is the only way the two cannot
    /// drift.
    PluginsInstalled,
    /// One hooking plugin type per line, in registration order:
    /// `brocanth_middleware::Csrf`.
    ///
    /// `AppBuilder::plugin_hooking` widens the application's hook record
    /// to `Both<P::Families, H>`, and the generated `plugins::Hooks` alias
    /// has to name that exact type, so it is built from these lines the
    /// way `Installed` is built from [`Slot::PluginsInstalled`].
    /// [`snippet::hooking`] writes this line, the provider line and the
    /// registration together.
    PluginsHooks,
    /// `src/plugins/mod.rs`, one arm of `layer`, for a module that offers
    /// middleware: `"session" => Some(sessions::layer),`.
    ///
    /// The arm used to be written in `src/app.rs` by a generator that knew
    /// two module keys by name. Mounting a layer is the application's
    /// decision and stays in `src/app.rs`; *finding* the function for a
    /// mounted name is mechanism, and mechanism the generator owns.
    PluginsLayers,
    /// One server wrap per line: the path of a type, from `crate::plugins`,
    /// that implements the generated `plugins::ServerWrap` trait.
    /// `observability::serving::Serving`.
    ///
    /// A server wrap sits around the whole handler, outside the request
    /// hooks: it sees every request, a static file included, and the
    /// response after the hooks. It can also open a listener of its own
    /// (`side_listeners`) and delay the stop (`before_stop`).
    ///
    /// **Order.** The first line is the outermost wrap: it sees a request
    /// first and the response last. Lines land in the order the modules
    /// contribute, so a wrap that must sit inside another declares
    /// `after(other)`. `side_listeners` and `before_stop` run in the same
    /// order, one after the other.
    ///
    /// The generator wrote these three functions itself for one module,
    /// `observability`, so no other module could wrap the server.
    ServerWrap,
    /// `src/main.rs`, one `mod` line per module a module puts at the crate
    /// root: `mod assets;`. The generator sorts them among its own, as
    /// rustfmt does.
    ///
    /// `src/main.rs` is the developer's file. `brocanth add` writes a new
    /// line there only while the file is as the generator left it.
    MainMods,
    /// `src/pages/mod.rs`, the renderers: one `pub mod` per template
    /// engine, each wrapping that engine's `load_dir!`, plus whatever the
    /// engine needs beside it.
    ///
    /// Written **before** [`Slot::PagesItems`], because a block there may
    /// call a renderer's bridge.
    ///
    /// This is the seam a second template engine attaches to. Until it
    /// existed, `rsx` was named as a literal in the generator and a
    /// project could have exactly one way to make a page. A renderer
    /// writes its own `pub mod`, so two of them in one project is two
    /// blocks and not a conflict: a page tree holds `index.rsx` beside
    /// `report.jt`, the route table ignores both because neither is a
    /// `.rs` file, and each engine compiles its own tree.
    PagesRenderers,
    /// One line per route source a module contributes, placed inside the
    /// generated `pages::mounted`.
    ///
    /// A generated project has one route table. The pages fill it from the
    /// directory; a module fills it from here; a declarative routes file
    /// fills it the same way. Because it is one table, a path claimed
    /// twice is a build error rather than whichever source ran last.
    ///
    /// The line calls a function with the shape every route source in a
    /// generated project offers:
    ///
    /// ```ignore
    /// pub fn mount(
    ///     routes: &mut RouterBuilder<Handler>,
    ///     prefix: &str,
    /// ) -> Result<(), BuildError>;
    /// ```
    ///
    /// The prefix is the application's decision and not the module's,
    /// which is the rule `plugin_routes` already states for the core: a
    /// path is a decision, and a module that mounted itself would be
    /// making it.
    RouterMounts,
    /// One route pattern per page a module writes, as the route table
    /// spells it: `/status`, `/users/{id}`.
    ///
    /// `src/pages/mod.rs` carries a test asserting the whole table, which
    /// is the point of a route being a file: the list is written down once
    /// and the directory has to match it. A module that writes a page adds
    /// a route, so without this the generated test failed the moment the
    /// module was chosen -- which it did, for `openapi`, in every project
    /// that picked it.
    PagesRoutes,
    /// `src/pages/mod.rs`, items the pages share.
    PagesItems,
    /// `src/commands/mod.rs`, one `pub mod` line per subcommand a module
    /// writes.
    ///
    /// A project with no command line has nowhere to put one, so a module
    /// that writes here declares `needs("commands", ..)`.
    CommandsMod,
    /// `config/<env>.toml`, one section per module that claims one.
    ConfigSections,
    /// `.gitignore`, one path per line.
    GitIgnore,
    /// `mise.toml`, one `[tasks.x]` table per task.
    MiseTasks,
    /// `package.json`, one node package per line: `name version`, as
    /// [`snippet::node_dependency`] writes it.
    ///
    /// The generator writes `package.json` when this slot has a line, with
    /// every package under `devDependencies`, sorted by name. A module does
    /// not write `package.json` itself: two modules that did would clash on
    /// the file. Two modules may name one package with one version; two
    /// versions of one package is an error that names both modules.
    ///
    /// `devDependencies` and not `dependencies`: node builds an asset here,
    /// and the binary that ships runs without node.
    NodeDependencies,
    /// `AGENTS.md`, one `##` section per module that has a rule a coding
    /// agent must follow. The sections land after "Adding a route", before
    /// the generic sections, in the order the modules contribute.
    AgentsSections,
}

impl Slot {
    /// Every slot, in declaration order.
    pub const ALL: [Slot; 20] = [
        Slot::AppUses,
        Slot::AppPlugins,
        Slot::AppSetup,
        Slot::AppContext,
        Slot::PluginsMod,
        Slot::PluginsInstalled,
        Slot::PluginsHooks,
        Slot::PluginsLayers,
        Slot::ServerWrap,
        Slot::MainMods,
        Slot::PagesRenderers,
        Slot::RouterMounts,
        Slot::PagesRoutes,
        Slot::PagesItems,
        Slot::CommandsMod,
        Slot::ConfigSections,
        Slot::GitIgnore,
        Slot::MiseTasks,
        Slot::NodeDependencies,
        Slot::AgentsSections,
    ];

    /// The categories whose modules may write here. Empty means any
    /// module: settings, `.gitignore` lines and agent rules are not a
    /// kind of seam.
    ///
    /// Registering a plugin is open to every category that runs in the
    /// server, because a middleware, a route source and a service are all
    /// plugins. What is specific is the seam past registration: a hook, a
    /// layer or a context value (middleware), a renderer or a page function
    /// (renderer), a mount or a page route (routes), a subcommand
    /// (enhancer), a build task (external).
    pub fn open_to(self) -> &'static [Category] {
        use Category::*;
        match self {
            Slot::AppUses | Slot::AppPlugins | Slot::PluginsMod | Slot::PluginsInstalled => {
                &[Middleware, Renderer, Enhancer, Routes]
            }
            Slot::AppSetup | Slot::AppContext | Slot::PluginsHooks | Slot::PluginsLayers => {
                &[Middleware]
            }
            // A middleware wraps every request; a renderer serves the static
            // files its pages link to, before the router.
            Slot::ServerWrap => &[Middleware, Renderer],
            Slot::PagesRenderers | Slot::PagesItems => &[Renderer],
            Slot::RouterMounts | Slot::PagesRoutes => &[Routes],
            Slot::CommandsMod => &[Enhancer],
            Slot::MiseTasks | Slot::NodeDependencies => &[External],
            Slot::ConfigSections | Slot::GitIgnore | Slot::AgentsSections | Slot::MainMods => &[],
        }
    }
}

impl fmt::Display for Slot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Slot::AppUses => "src/plugins/mod.rs (uses)",
            Slot::AppPlugins => "src/plugins/mod.rs (the builder chain)",
            Slot::AppSetup => "src/app.rs (setup)",
            Slot::AppContext => "src/plugins/mod.rs (the context)",
            Slot::PluginsMod => "src/plugins/mod.rs",
            Slot::PluginsInstalled => "src/plugins/mod.rs (the provider list)",
            Slot::PluginsHooks => "src/plugins/mod.rs (the hook record)",
            Slot::PluginsLayers => "src/plugins/mod.rs (layer)",
            Slot::ServerWrap => "src/plugins/mod.rs (the server wraps)",
            Slot::MainMods => "src/main.rs (the crate-root modules)",
            Slot::PagesRenderers => "src/pages/mod.rs (the renderers)",
            Slot::RouterMounts => "src/pages/mod.rs (the mounted routes)",
            Slot::PagesRoutes => "src/pages/mod.rs (the route table test)",
            Slot::PagesItems => "src/pages/mod.rs",
            Slot::CommandsMod => "src/commands/mod.rs",
            Slot::ConfigSections => "config/<env>.toml",
            Slot::GitIgnore => ".gitignore",
            Slot::MiseTasks => "mise.toml",
            Slot::NodeDependencies => "package.json",
            Slot::AgentsSections => "AGENTS.md",
        };
        f.write_str(name)
    }
}

/// One value a module can be configured with, declared once.
///
/// From one declaration the generated project takes two things: a key in
/// `config/<env>.toml` when the project has the `config` module, and a flag
/// on the binary's command line, `--<section>.<key>=VALUE`. The two cannot
/// disagree about the name or the type, because neither is written by hand.
///
/// The settings struct in the module's generated plugin still holds the
/// field, and its `Default` still says the value a project without a file
/// starts from. A key here is a path into that struct: `pool.max_size` is
/// `settings.pool.max_size`, which is also what the file's `[x.pool]`
/// deserialises into.
///
/// See `docs/module-flags.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    /// The key inside the section, dotted for a subsection: `cookie`,
    /// `pool.max_size`. Lower case, digits and `_`, as a TOML key and a
    /// Rust field are both spelled.
    pub key: &'static str,
    /// What the value is.
    pub kind: Kind,
    /// The value `config/<env>.toml` is written with, as a TOML literal.
    pub value: String,
    /// What it does, in one line: the flag's `--help`, and the first line
    /// of the file's comment.
    pub help: &'static str,
    /// More for the file's comment, under the help: what a reader editing
    /// the file needs and a `--help` line has no room for. A `\n` starts
    /// another comment line. Empty for none.
    pub note: String,
    /// Whether the command line may set it. Off for a secret: every user on
    /// the machine can read another process's arguments.
    pub flag: bool,
    /// Whether the field is an `Option`. An empty value, `--x=` or `X=`,
    /// sets `None`; an absent one leaves the layer below in charge. The file
    /// writes the key commented out, because `None` has no TOML spelling.
    pub optional: bool,
    /// Whether the file writes the key commented out: an example, and the
    /// plugin's default until somebody removes the `#`. Implied by
    /// `optional`.
    pub commented: bool,
    /// A value that also means `None`, for an optional setting whose
    /// operators think in a sentinel: `-1` for "no limit". Set with
    /// [`Setting::none_as`]. Not a global rule: `-1` is invalid for an
    /// unsigned field and a real value for some signed ones.
    pub none_as: Option<&'static str>,
}

/// What a [`Setting`] holds.
///
/// Three kinds, because a flag arrives as text and the file needs to know
/// whether to quote it. The generated code parses a flag into the field's
/// own type, so any field that parses from a string can be declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A string, quoted in the file and never typed on the command line:
    /// `--sessions.cookie=123` is the string `123`.
    Text,
    /// `true` or `false`.
    Bool,
    /// A whole number.
    Integer,
    /// A number with a fractional part.
    Float,
    /// A list of strings. In the file only, and in the environment by index
    /// when the project has `config`: a flag has no spelling for a list that
    /// would not also be a way to get one wrong.
    List,
}

impl Setting {
    /// A string.
    pub fn text(key: &'static str, value: &str, help: &'static str) -> Self {
        Self::new(key, Kind::Text, format!("{value:?}"), help)
    }

    /// A boolean.
    pub fn bool(key: &'static str, value: bool, help: &'static str) -> Self {
        Self::new(key, Kind::Bool, value.to_string(), help)
    }

    /// A whole number.
    pub fn integer(key: &'static str, value: i64, help: &'static str) -> Self {
        Self::new(key, Kind::Integer, value.to_string(), help)
    }

    /// A number with a fractional part.
    pub fn float(key: &'static str, value: f64, help: &'static str) -> Self {
        Self::new(key, Kind::Float, format!("{value:?}"), help)
    }

    /// A list of strings. No flag; see [`Kind::List`].
    pub fn list(key: &'static str, value: &[&str], help: &'static str) -> Self {
        let items: Vec<String> = value.iter().map(|item| format!("{item:?}")).collect();
        let mut setting = Self::new(key, Kind::List, format!("[{}]", items.join(", ")), help);
        setting.flag = false;
        setting
    }

    fn new(key: &'static str, kind: Kind, value: String, help: &'static str) -> Self {
        Self {
            key,
            kind,
            value,
            help,
            note: String::new(),
            flag: true,
            optional: false,
            commented: false,
            none_as: None,
        }
    }

    /// The file writes this key commented out, as an example.
    pub fn commented(mut self) -> Self {
        self.commented = true;
        self
    }

    /// The field is an `Option`, and `alias` means `None` as well as an
    /// empty value does. `--help` and the file's comment say so.
    pub fn none_as(mut self, alias: &'static str) -> Self {
        self.optional = true;
        self.none_as = Some(alias);
        self
    }

    /// The help, with what means `None` for an optional setting.
    pub fn full_help(&self) -> String {
        match (self.optional, self.none_as) {
            (false, _) => self.help.to_string(),
            (true, None) => format!("{} Empty means none.", self.help),
            (true, Some(alias)) => format!("{} Empty or `{alias}` means none.", self.help),
        }
    }

    /// The field is an `Option`: an empty value sets `None`.
    pub fn optional(mut self) -> Self {
        self.optional = true;
        self
    }

    /// More for the file's comment, under the help.
    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.note = note.into();
        self
    }

    /// No flag for this one: the file and the environment only.
    ///
    /// For a secret. A flag is in `ps` output for every user on the host and
    /// in the shell history of whoever typed it.
    pub fn secret(mut self) -> Self {
        self.flag = false;
        self
    }
}

/// The TOML a section's declared settings are written as.
///
/// Keys without a dot first, under `[section]`; then each subsection under
/// its own header, in the order its first key was declared. Each key carries
/// its help as a comment.
pub fn config_section(section: &str, settings: &[Setting]) -> String {
    let mut out = format!("\n[{section}]\n");
    let mut subsections: Vec<&str> = Vec::new();
    for setting in settings {
        match setting.key.rsplit_once('.') {
            None => out.push_str(&config_line(setting.key, setting)),
            Some((sub, _)) => {
                if !subsections.contains(&sub) {
                    subsections.push(sub);
                }
            }
        }
    }
    for sub in subsections {
        out.push_str(&format!("\n[{section}.{sub}]\n"));
        for setting in settings {
            if let Some((this, key)) = setting.key.rsplit_once('.') {
                if this == sub {
                    out.push_str(&config_line(key, setting));
                }
            }
        }
    }
    out
}

fn config_line(key: &str, setting: &Setting) -> String {
    let mut out = format!("# {}\n", setting.full_help());
    if !setting.note.is_empty() {
        for line in setting.note.split('\n') {
            out.push_str(format!("# {line}").trim_end());
            out.push('\n');
        }
    }
    let comment = if setting.optional || setting.commented {
        "# "
    } else {
        ""
    };
    out.push_str(&format!("{comment}{key} = {}\n", setting.value));
    out
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
    settings: Vec<(String, String, Setting)>,
    settings_types: BTreeMap<String, String>,
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
    /// # A module owns a directory, not a file
    ///
    /// A plugin goes in `src/plugins/<key>/mod.rs`, never
    /// `src/plugins/<key>.rs`. Rust accepts both and the difference is not
    /// about Rust: a module that owns a directory can be removed by
    /// deleting it, and a developer who adds a second file beside the
    /// first does not move anything to do it. Once projects exist in the
    /// world the choice cannot be revisited, so it is made here.
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

    /// Declare one setting of a section this module owns.
    ///
    /// Prefer [`snippet::configurable`], which declares the settings and
    /// writes everything that follows from them. This is the half that
    /// records them, for a module that registers its plugin some other way.
    ///
    /// Two declarations of one key are a problem, named with both modules:
    /// the second would be a flag `clap` refuses and a key the file holds
    /// once.
    pub fn setting(&mut self, section: &str, setting: Setting) {
        let path = format!("{section}.{}", setting.key);
        if let Some((by, _, _)) = self
            .settings
            .iter()
            .find(|(_, s, other)| format!("{s}.{}", other.key) == path)
        {
            self.problems.push(format!(
                "`{by}` and `{}` both declare the setting `{path}`",
                self.writing
            ));
            return;
        }
        self.settings
            .push((self.writing.clone(), section.to_string(), setting));
    }

    /// Name the type a section's settings deserialise into, when it is not
    /// the generated `crate::plugins::<section>::Settings`.
    ///
    /// A plugin from a library crate has its own configuration type,
    /// `brocanth_middleware::CsrfConfig`, and no generated settings struct.
    /// The generated readers start from that type's `Default` and set its
    /// fields, so the path must name a type with `Default` and public
    /// fields.
    pub fn settings_type(&mut self, section: &str, path: &str) {
        self.settings_types
            .insert(section.to_string(), path.to_string());
    }

    /// The type a section's settings deserialise into: the one
    /// [`Scaffold::settings_type`] named, or the generated
    /// `crate::plugins::<section>::Settings`.
    pub fn settings_type_of(&self, section: &str) -> String {
        self.settings_types
            .get(section)
            .cloned()
            .unwrap_or_else(|| format!("crate::plugins::{section}::Settings"))
    }

    /// Every declared setting: the module that declared it, its section,
    /// and the setting. In the order the modules ran.
    pub fn settings(&self) -> impl Iterator<Item = (&str, &str, &Setting)> {
        self.settings
            .iter()
            .map(|(by, section, setting)| (by.as_str(), section.as_str(), setting))
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

    #[test]
    fn a_dotted_key_is_a_subsection_after_the_plain_keys() {
        let text = config_section(
            "db",
            &[
                Setting::integer("pool.max_size", 5, "Connections at most."),
                Setting::text("url", "sqlite://db", "Where the database is.").note("Two\nlines"),
                Setting::bool("pool.lazy", true, "Open on first use."),
            ],
        );
        assert_eq!(
            text,
            "\n[db]\n# Where the database is.\n# Two\n# lines\nurl = \"sqlite://db\"\n\
             \n[db.pool]\n# Connections at most.\nmax_size = 5\n# Open on first use.\nlazy = true\n"
        );
    }

    #[test]
    fn one_key_declared_twice_names_both_modules() {
        let mut s = Scaffold::new();
        s.now_writing("sessions");
        s.setting("sessions", Setting::text("cookie", "a", "A."));
        s.now_writing("other");
        s.setting("sessions", Setting::text("cookie", "b", "B."));
        assert_eq!(
            s.problems(),
            ["`sessions` and `other` both declare the setting `sessions.cookie`"]
        );
        assert_eq!(s.settings().count(), 1);
    }

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
    use super::{Kind, Project, Scaffold, Setting, Slot};

    /// Registers a plugin whose settings are declared, and writes everything
    /// that follows from the declaration.
    ///
    /// * With the `config` module: the `[key]` section, generated from the
    ///   settings, and the configured registration. The flags reach the
    ///   file's loader as its top layer when the project turns them on.
    /// * Without it: a registration that starts from `Settings::default()`
    ///   and applies whatever flags this run was given, through a function
    ///   the CLI generates per section in `src/plugins/mod.rs`.
    ///
    /// The provider line too, as [`plugin`] writes it. `key` is the module's
    /// key, its plugin's module name and its section; `ty` the plugin type.
    pub fn configurable(
        s: &mut Scaffold,
        project: &Project,
        key: &str,
        ty: &str,
        settings: Vec<Setting>,
    ) {
        if project.has("config") {
            s.at(Slot::ConfigSections)
                .block(super::config_section(key, &settings));
        }
        // Without a file, a declared setting still has the environment
        // and, unless it is a secret, a flag.
        if project.has("config") || settings.iter().all(|setting| setting.kind == Kind::List) {
            s.at(Slot::AppPlugins).block(plugin_line(project, key, ty));
        } else {
            s.at(Slot::AppPlugins).block(format!(
                "        let builder = builder.plugin(crate::plugins::{key}::{ty}::new(\n\
                 \x20           crate::plugins::{ident}_settings(&given)?,\n\
                 \x20       ));\n",
                ident = key.replace('-', "_"),
            ));
        }
        s.at(Slot::PluginsInstalled)
            .line(format!("{key} crate::plugins::{key}::{ty}"));
        for setting in settings {
            s.setting(key, setting);
        }
    }

    /// A plugin from a library crate that installs hooks, with declared
    /// settings. See [`configurable_hooking`].
    #[derive(Debug, Clone)]
    pub struct Hooking<'a> {
        /// The module's key, which names the provider alias.
        pub key: &'a str,
        /// The config section, which is also the plugin's instance name:
        /// `csrf`, `security_headers`. Flags are `--<section>.<key>`.
        pub section: &'a str,
        /// The plugin type: `brocanth_middleware::Csrf`.
        pub ty: &'a str,
        /// Its configuration type: `brocanth_middleware::CsrfConfig`. The
        /// generated readers start from its `Default` and set its public
        /// fields, and the file deserialises into it.
        pub config_ty: &'a str,
        /// What comes before the registration statement, already indented
        /// for `register!`: the comment that explains it, and any `let`
        /// the adjustment needs. Empty for none.
        pub prelude: String,
        /// A method chain applied to the configuration before the plugin is
        /// built, for a value only code can give: `.inline_script(TEXT)`.
        /// The file and the flags still decide every key it leaves alone.
        pub adjust: Option<String>,
    }

    /// A method chain, one call per line, as rustfmt breaks a chain that
    /// does not fit: `.a(x).b(y)` at `indent` spaces, `last` after the
    /// final call.
    fn broken_chain(chain: &str, indent: usize, last: &str) -> String {
        let mut calls = Vec::new();
        let (mut depth, mut start) = (0usize, 0usize);
        for (at, c) in chain.char_indices() {
            match c {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth = depth.saturating_sub(1),
                '.' if depth == 0 && at > start => {
                    calls.push(&chain[start..at]);
                    start = at;
                }
                _ => {}
            }
        }
        calls.push(&chain[start..]);
        let pad = " ".repeat(indent);
        let count = calls.len();
        calls
            .iter()
            .enumerate()
            .map(|(i, call)| {
                let end = if i + 1 == count { last } else { "" };
                format!("{pad}{call}{end}\n")
            })
            .collect()
    }

    /// Registers a hooking plugin whose settings are declared, and writes
    /// everything that follows from the declaration.
    ///
    /// [`configurable`] for a plugin that installs hooks and whose
    /// configuration type lives in its own crate:
    ///
    /// * With `config`: the `[section]` block and a registration through
    ///   `plugin_hooking_from_config`, so the file, the environment and the
    ///   flags reach the plugin.
    /// * Without it: a registration that starts from the configuration
    ///   type's `Default` and applies the environment and the flags.
    ///
    /// Also the provider line and the hook record line, as [`hooking`]
    /// writes them, and the settings type, so the generated readers name
    /// the plugin's own type.
    pub fn configurable_hooking(
        s: &mut Scaffold,
        project: &Project,
        hooking: Hooking<'_>,
        settings: Vec<Setting>,
    ) {
        let Hooking {
            key,
            section,
            ty,
            config_ty,
            prelude,
            adjust,
        } = hooking;
        if project.has("config") {
            s.at(Slot::ConfigSections)
                .block(super::config_section(section, &settings));
        }
        let statement = if project.has("config") {
            match &adjust {
                None => format!(
                    "        let builder = builder\n\
                     \x20           .plugin_hooking_from_config::<{ty}>({section:?})\n\
                     \x20           .map_err(|e| e.to_string())?;\n"
                ),
                Some(adjust) => {
                    let one = format!("                |config| config{adjust},\n");
                    let closure = if one.trim_end().len() <= 100 {
                        one
                    } else {
                        format!(
                            "                |config| {{\n\
                             \x20                   config\n\
                             {}\
                             \x20               }},\n",
                            broken_chain(adjust, 24, "")
                        )
                    };
                    format!(
                        "        let builder = builder\n\
                         \x20           .plugin_hooking_from_config_with::<{ty}>(\n\
                         \x20               {section:?},\n\
                         {closure}\
                         \x20           )\n\
                         \x20           .map_err(|e| e.to_string())?;\n"
                    )
                }
            }
        } else {
            let ident = section.replace('-', "_");
            let value = match &adjust {
                None => format!("crate::plugins::{ident}_settings(&given)?"),
                Some(adjust) => {
                    let one = format!("crate::plugins::{ident}_settings(&given)?{adjust}");
                    // 16 columns of indent and a comma: what the argument
                    // line costs inside the registration.
                    if one.len() < 100 - 16 {
                        one
                    } else {
                        format!(
                            "crate::plugins::{ident}_settings(&given)?\n{}",
                            broken_chain(adjust, 4, "")
                        )
                    }
                }
            };
            hooking_line(ty, &value)
        };
        s.at(Slot::AppPlugins)
            .block(format!("{prelude}{statement}"));
        s.at(Slot::PluginsInstalled).line(format!("{key} {ty}"));
        s.at(Slot::PluginsHooks).line(ty);
        s.settings_type(section, config_ty);
        for setting in settings {
            s.setting(section, setting);
        }
    }

    /// The statement that registers a plugin instance in the application.
    ///
    /// Two forms, and which one is right is not the module's choice: with
    /// the config plugin the instance is built from a named section, and
    /// without it the generator hands it a literal the compiler checks.
    ///
    /// It is here, and not written out in each module, for a duller reason
    /// than that. Both forms are longer than rustfmt's `chain_width`, so
    /// they have to arrive already broken or a developer's first
    /// `cargo fmt` rewrites a file they never touched. That is a fact
    /// about rustfmt, not about caching or logging, and every module
    /// getting it right separately is every module getting it wrong once.
    ///
    /// # Why a rebinding `let` and not one builder chain
    ///
    /// It was a chain, written straight into `src/app.rs`. The chain now
    /// lives in `plugins::register!`, and a chain of exactly one call --
    /// a project with one plugin, which is the common first project -- is
    /// one rustfmt folds back onto its receiver. One statement per
    /// registration is formatted on its own, so what the generator writes
    /// does not depend on how many other modules were chosen.
    ///
    /// Each one still reads as one line a reader can delete.
    ///
    /// The paths are spelled from the crate root because the macro these
    /// land in expands in `src/app.rs`, not in the file they are written
    /// into.
    ///
    /// `key` is the module's key, which is also the plugin's module name
    /// and its config section. `ty` is the plugin type in that module.
    /// Registers a plugin: the line in the chain, and the type in the
    /// provider list.
    ///
    /// Both, from one call, because they have to agree. The chain's type
    /// is `Install` applied once per plugin in registration order, and the
    /// generated `App` names that type to hold a context. Writing the two
    /// separately is writing the same list twice.
    pub fn plugin(s: &mut Scaffold, project: &Project, key: &str, ty: &str) {
        s.at(Slot::AppPlugins).block(plugin_line(project, key, ty));
        s.at(Slot::PluginsInstalled)
            .line(format!("{key} crate::plugins::{key}::{ty}"));
    }

    /// Registers a plugin that installs hooks: the registration block, the
    /// provider line, and the hook record line.
    ///
    /// `block` is the whole statement, comments included, and it must call
    /// `plugin_hooking` on `builder`. Build the statement with
    /// [`hooking_line`] and put a comment before it, if any: do not write
    /// the statement by hand. `ty` is the plugin type as a path the
    /// generated `src/plugins/mod.rs` can name: `brocanth_middleware::Csrf`.
    /// `key` names the provider alias, so it is the module's key.
    ///
    /// Three slots from one call, because the three have to agree: the
    /// chain's type is the provider list and the hook record, each applied
    /// once per plugin in registration order.
    pub fn hooking(s: &mut Scaffold, key: &str, ty: &str, block: String) {
        s.at(Slot::AppPlugins).block(block);
        s.at(Slot::PluginsInstalled).line(format!("{key} {ty}"));
        s.at(Slot::PluginsHooks).line(ty);
    }

    /// The statement that registers a hooking plugin `ty`, built from the
    /// expression `config`, in the shape rustfmt gives it.
    ///
    /// ```
    /// let line = brocanth_scaffold::snippet::hooking_line(
    ///     "brocanth_correlation::Correlation",
    ///     "brocanth_correlation::CorrelationConfig::default()",
    /// );
    /// assert_eq!(
    ///     line,
    ///     "        let builder = builder.plugin_hooking(\n\
    ///      \x20           <brocanth_correlation::Correlation as brocanth::plugin_api::Plugin>::new(\n\
    ///      \x20               brocanth_correlation::CorrelationConfig::default(),\n\
    ///      \x20           ),\n\
    ///      \x20       );\n",
    /// );
    /// ```
    ///
    /// Pass the result to [`hooking`]. The statement calls
    /// `<ty as Plugin>::new(config)`, because a hooking plugin is built
    /// before `plugin_hooking` takes it, and `Plugin::new` is a trait
    /// method.
    ///
    /// `config` is one expression. Write it at column 0. A line after the
    /// first keeps its own indent, relative to the first: a chain that
    /// rustfmt breaks is `"Config::default()\n    .a(1)\n    .b(2)"`.
    /// This function indents it and adds the trailing comma.
    ///
    /// No `Project` argument: the statement is the same with and without
    /// `config`. A plugin whose configuration comes from the file uses
    /// [`configurable_hooking`], which calls this function for its
    /// literal branch.
    pub fn hooking_line(ty: &str, config: &str) -> String {
        // The argument, at `indent` columns, with its trailing comma.
        let value = |indent: usize| -> String {
            let pad = " ".repeat(indent);
            let mut lines: Vec<String> = config
                .lines()
                .map(|line| format!("{pad}{line}\n"))
                .collect();
            if let Some(last) = lines.last_mut() {
                last.insert(last.len() - 1, ',');
            }
            lines.concat()
        };
        // rustfmt keeps the call and the constructor on the `let` line when
        // they fit in 100 columns. Else on one line after `let builder =`,
        // when they fit there, which a short type name does. Else it breaks
        // both calls.
        let call = format!("builder.plugin_hooking(<{ty} as brocanth::plugin_api::Plugin>::new(");
        let head = format!("            {call}");
        if 8 + "let builder = ".len() + call.len() <= 100 {
            format!(
                "        let builder = {call}\n\
                 {}\
                 \x20       ));\n",
                value(12)
            )
        } else if head.len() <= 100 {
            format!(
                "        let builder =\n\
                 {head}\n\
                 {}\
                 \x20           ));\n",
                value(16)
            )
        } else {
            format!(
                "        let builder = builder.plugin_hooking(\n\
                 \x20           <{ty} as brocanth::plugin_api::Plugin>::new(\n\
                 {}\
                 \x20           ),\n\
                 \x20       );\n",
                value(16)
            )
        }
    }

    /// Asks for a node package in the generated `package.json`:
    /// `node_dependency(s, "@tailwindcss/cli", "^4.1.14")`.
    ///
    /// The generator writes the file from [`Slot::NodeDependencies`], so
    /// several modules can each name what they need.
    pub fn node_dependency(s: &mut Scaffold, name: &str, version: &str) {
        debug_assert!(
            !name.contains(char::is_whitespace) && !version.contains(char::is_whitespace),
            "a node package name or version with a space: `{name}` `{version}`"
        );
        s.at(Slot::NodeDependencies)
            .once(format!("{name} {version}"));
    }

    /// The registration statement alone, without the provider list.
    ///
    /// **Prefer [`plugin`].** This one writes half of what a registration
    /// is, and a module that writes only this half registers a plugin the
    /// generated `Installed` type does not know about. If that plugin
    /// provides a context item, the generated `src/app.rs` fails to
    /// compile on a type the developer did not write and cannot fix.
    ///
    /// It stays public for a caller that builds the chain some other way
    /// and writes [`Slot::PluginsInstalled`] itself.
    pub fn plugin_line(project: &Project, key: &str, ty: &str) -> String {
        if project.has("config") {
            format!(
                "        let builder = builder\n\
                 \x20           .plugin_from_config::<crate::plugins::{key}::{ty}>({key:?})\n\
                 \x20           .map_err(|e| e.to_string())?;\n"
            )
        } else {
            format!(
                "        let builder = builder.plugin(crate::plugins::{key}::{ty}::new(\n\
                 \x20           crate::plugins::{key}::Settings::default(),\n\
                 \x20       ));\n"
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
