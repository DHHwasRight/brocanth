//! The resolver: each Kind a chosen module requires, bound to one provider,
//! at generation time. `docs/module-capabilities.md` §4.1 and §8.
//!
//! A module says `requires::<PgPool>`, not `needs("db")`. Here the CLI finds
//! the provider:
//!
//! * none chosen, and one in the registry that is not development-only: it
//!   is added, and the front door prints `added db: jobs requires PgPool`;
//! * none chosen, several, and one of them the Kind's default
//!   ([`Manifest::default_for`]): the default is added, and said:
//!   `added storage-fs: storage requires ObjectStore`;
//! * none chosen, and several with no default (or only development-only
//!   ones): an error that names each, because a backend is a real choice;
//! * two chosen for a Kind that takes one: an error that names both.
//!
//! A development-only provider is never added for a requirement. It binds
//! when it is chosen by name.
//!
//! The result stays explicit (ADR 1): the binding is a comment over the
//! plugin chain and a `[bindings]` entry in `brocanth.toml`, and the chain
//! names every plugin as before.
//!
//! [`Manifest::default_for`]: brocanth_scaffold::Manifest::default_for

use brocanth_scaffold::kind::Cardinality;
use brocanth_scaffold::{KindRef, Requirement};

use crate::commands::create::{about, available, Door};
use crate::ui::Diagnostic;

/// A module the resolver added to a selection, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Added {
    /// The provider added.
    pub key: &'static str,
    /// The chosen module whose requirement brought it.
    pub by: &'static str,
    /// The Kind required.
    pub kind: KindRef,
}

impl Added {
    /// The line the front doors print: `added db: jobs requires PgPool (a
    /// Postgres pool)`.
    pub fn line(&self) -> String {
        format!(
            "added {}: {} requires {} ({})",
            self.key, self.by, self.kind, self.kind.gist
        )
    }
}

/// Every module in this build that provides `kind`, in registry order.
pub fn providers(kind: &str) -> Vec<&'static str> {
    available()
        .into_iter()
        .filter(|key| about(key).gives(kind))
        .collect()
}

/// The one provider of `kind` the resolver may add: the only one that is
/// not development-only. `None` when there is no such provider or more
/// than one.
pub fn single_provider(kind: &str) -> Option<&'static str> {
    let mut deployable = providers(kind)
        .into_iter()
        .filter(|key| about(key).development_only.is_none());
    let first = deployable.next()?;
    deployable.next().is_none().then_some(first)
}

/// The default provider of `kind` ([`Manifest::default_for`]), when the
/// registry names one that is not development-only.
///
/// [`Manifest::default_for`]: brocanth_scaffold::Manifest::default_for
pub fn default_provider(kind: &str) -> Option<&'static str> {
    providers(kind)
        .into_iter()
        .find(|key| about(key).is_default_for(kind) && about(key).development_only.is_none())
}

/// The provider the resolver adds for a requirement on `kind` nobody
/// meets: the single deployable one, else the default.
pub fn addable_provider(kind: &str) -> Option<&'static str> {
    single_provider(kind).or_else(|| default_provider(kind))
}

/// A provider `add` took out because the module being added provides the
/// same Kind, and the Kind is a backend choice (it has a default).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replaced {
    /// The provider taken out.
    pub key: &'static str,
    /// The module being added that replaces it.
    pub by: &'static str,
    /// The Kind both provide.
    pub kind: KindRef,
}

impl Replaced {
    /// `replaced storage-fs: storage-s3 provides ObjectStore (an object store)`.
    pub fn line(&self) -> String {
        format!(
            "replaced {}: {} provides {} ({})",
            self.key, self.by, self.kind, self.kind.gist
        )
    }
}

/// `picks` with every other provider of a backend-choice Kind taken out,
/// for each module in `adding` that provides one. A Kind is a backend
/// choice when the registry names a default for it: `ObjectStore` is,
/// `SessionCookie` is not, so adding `sessions` beside `auth` is still
/// refused as two cookies (`docs/object-storage.md` §3.3).
pub fn replace(
    picks: &[&'static str],
    adding: &[&'static str],
) -> (Vec<&'static str>, Vec<Replaced>) {
    let mut out = picks.to_vec();
    let mut replaced = Vec::new();
    for by in adding {
        for (kind, _) in about(by).provides {
            if kind.cardinality != Cardinality::One || default_provider(kind.name).is_none() {
                continue;
            }
            out.retain(|key| {
                let goes = !adding.contains(key) && about(key).gives(kind.name);
                if goes {
                    replaced.push(Replaced { key, by, kind });
                }
                !goes
            });
        }
    }
    (out, replaced)
}

/// A project recorded before the `ObjectStore` Kind: `storage` and no
/// provider of it. Today's `storage` with `db` kept the bytes in Postgres,
/// so it reads as `storage-db`; without `db`, as `storage-fs`. `add` and
/// `remove` call it on the record, so neither moves a project's files in
/// silence (`docs/object-storage.md` §3.4). Answers what it added.
pub fn upgrade_record(picks: &mut Vec<&'static str>) -> Option<&'static str> {
    if !picks.contains(&"storage") || picks.iter().any(|p| about(p).gives("ObjectStore")) {
        return None;
    }
    let backend = if picks.iter().any(|p| about(p).gives("PgPool")) {
        "storage-db"
    } else {
        "storage-fs"
    };
    let order = crate::commands::create::all();
    let key = order.iter().copied().find(|k| *k == backend)?;
    picks.push(key);
    picks.sort_by_key(|p| order.iter().position(|k| k == p));
    Some(key)
}

/// Each chosen module's requirement that no chosen module provides, in
/// registry order.
fn unmet(picks: &[&'static str]) -> Vec<(&'static str, Requirement)> {
    let mut out = Vec::new();
    for pick in picks {
        let m = about(pick);
        for r in m.requirements(picks) {
            if !picks.iter().any(|p| about(p).gives(r.kind.name)) {
                out.push((*pick, r.clone()));
            }
        }
    }
    out
}

/// `picks` with the provider of every requirement nobody chose, and what
/// was added. The provider goes in at its place in the registry, so the
/// same set of words still makes the same project.
///
/// Refuses, with one diagnostic per requirement, when a requirement has no
/// provider to add.
pub fn resolve(
    picks: &[&'static str],
    door: Door,
) -> Result<(Vec<&'static str>, Vec<Added>), Vec<Diagnostic>> {
    let mut picks = picks.to_vec();
    let mut added: Vec<Added> = Vec::new();
    loop {
        let missing = unmet(&picks);
        if missing.is_empty() {
            return Ok((picks, added));
        }
        // A single provider first, a default after: the provider a default
        // requires may arrive with a single-provider requirement
        // (`storage-db` and `jobs` both bring `db`, once).
        let pick = missing
            .iter()
            .find_map(|(by, r)| single_provider(r.kind.name).map(|key| (by, r, key)))
            .or_else(|| {
                missing
                    .iter()
                    .find_map(|(by, r)| default_provider(r.kind.name).map(|key| (by, r, key)))
            });
        let Some((by, r, key)) = pick else {
            return Err(refusals(&picks, door));
        };
        added.push(Added {
            key,
            by,
            kind: r.kind,
        });
        picks.push(key);
        let order = crate::commands::create::all();
        picks.sort_by_key(|p| order.iter().position(|k| k == p));
    }
}

/// One error per requirement no chosen module provides. The floor under
/// the front doors: [`crate::commands::create::selection`] calls it after
/// they resolved, so only a requirement with no single provider is left.
pub fn refusals(picks: &[&'static str], door: Door) -> Vec<Diagnostic> {
    unmet(picks)
        .into_iter()
        .map(|(pick, r)| {
            let kind = r.kind;
            let all = providers(kind.name);
            let d = Diagnostic::error(
                pick,
                format!("`{pick}` requires {kind} ({})", kind.gist),
            )
            .note(r.why);
            match (addable_provider(kind.name), all.as_slice()) {
                (_, []) => d.help(format!(
                    "no module in this build provides {kind}; remove `{pick}`, or build the CLI with a module that provides it"
                )),
                (Some(one), _) => d.help(door.bring(picks, one, pick)),
                (None, many) => d.help(format!("add one of: {}", many.join(", "))),
            }
        })
        .collect()
}

/// Two chosen providers of a Kind that takes one. A child that provides
/// its parent's Kind refines the parent, so that pair is not a conflict;
/// two children of a parent that does not provide it (`storage-fs` and
/// `storage-s3` under `storage`) are.
pub fn doubled(picks: &[&'static str]) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for pick in picks {
        for (kind, _) in about(pick).provides {
            if kind.cardinality != Cardinality::One || seen.contains(&kind.name) {
                continue;
            }
            seen.push(kind.name);
            let chosen: Vec<&str> = picks
                .iter()
                .copied()
                .filter(|p| about(p).gives(kind.name))
                .filter(|p| {
                    about(p).parent.is_none_or(|parent| {
                        !picks.contains(&parent) || !about(parent).gives(kind.name)
                    })
                })
                .collect();
            if chosen.len() > 1 {
                out.push(
                    Diagnostic::error(
                        *pick,
                        format!(
                            "{} each provide {kind}, and a project takes one",
                            chosen
                                .iter()
                                .map(|k| format!("`{k}`"))
                                .collect::<Vec<_>>()
                                .join(" and ")
                        ),
                    )
                    .note(format!(
                        "{kind} is {}: two of them is a bug, not a choice",
                        kind.gist
                    ))
                    .help(format!("keep one of: {}", chosen.join(", "))),
                );
            }
        }
    }
    out
}

/// The provider bound to each Kind in a project of `picks`, as `(Kind
/// name, module key)`: the first chosen provider in registry order.
pub fn bindings(picks: &[&'static str]) -> Vec<(&'static str, &'static str)> {
    let mut out: Vec<(&'static str, &'static str)> = Vec::new();
    for pick in picks {
        for (kind, _) in about(pick).provides {
            if kind.cardinality == Cardinality::One && !out.iter().any(|(k, _)| *k == kind.name) {
                out.push((kind.name, pick));
            }
        }
    }
    out
}

/// The module that collects each Kind of cardinality many in a project of
/// `picks`, as `(Kind name, module key)`: the first chosen collector in
/// registry order. `admin` collects `AdminPanel`. A contributor asks
/// `Project::collector`, so it never names the collector's key.
pub fn collectors(picks: &[&'static str]) -> Vec<(&'static str, &'static str)> {
    let mut out: Vec<(&'static str, &'static str)> = Vec::new();
    for pick in picks {
        for (kind, _) in about(pick).collects {
            if !out.iter().any(|(k, _)| *k == kind.name) {
                out.push((kind.name, pick));
            }
        }
    }
    out
}

/// Why `kind` is bound in a project of `picks`: the first chosen module
/// that requires it, or `chosen` when none does.
///
/// Derived from the selection, so `add` and `remove` re-resolve it with no
/// state of their own, and the record never disagrees with the modules.
pub fn why(picks: &[&'static str], kind: &str) -> String {
    picks
        .iter()
        .find(|p| about(p).requirements(picks).any(|r| r.kind.name == kind))
        .map_or_else(
            || "chosen".to_string(),
            |by| format!("{by} requires {kind}"),
        )
}

/// What `key` contributes after: its own `after` list, and the chosen
/// provider of each Kind it requires or uses.
pub fn after(key: &str, picks: &[&'static str]) -> Vec<&'static str> {
    let m = about(key);
    let mut out = m.after.clone();
    let kinds: Vec<&str> = m
        .requirements(picks)
        .map(|r| r.kind.name)
        .chain(m.uses.iter().map(|(k, _)| k.name))
        .collect();
    for (kind, provider) in bindings(picks) {
        if kinds.contains(&kind) && provider != key && !out.contains(&provider) {
            out.push(provider);
        }
    }
    out
}

/// The modules among `stay` that require a Kind only `gone` provides, each
/// with the Kind and the reason. `remove` refuses on them.
///
/// A Kind whose default provider is not `gone` orphans nobody: `remove`
/// adds the default back (`brocanth remove storage-s3` returns to
/// `storage-fs`).
pub fn orphaned_by(
    gone: &str,
    stay: &[&'static str],
) -> Vec<(&'static str, KindRef, &'static str)> {
    let mut out = Vec::new();
    for (kind, _) in about(gone).provides {
        if stay.iter().any(|p| about(p).gives(kind.name)) {
            continue;
        }
        if default_provider(kind.name).is_some_and(|d| d != gone) {
            continue;
        }
        for key in stay {
            if let Some(r) = about(key)
                .requirements(stay)
                .find(|r| r.kind.name == kind.name)
            {
                out.push((*key, kind, r.why));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use brocanth_scaffold::{Category, Manifest, Module, PgPool, Project, Scaffold};

    /// A module made of a manifest, for the resolver tests.
    struct Fake(fn() -> Manifest);

    impl Module for Fake {
        fn manifest(&self) -> Manifest {
            (self.0)()
        }
        fn contribute(&self, _: &Project, _: &mut Scaffold) {}
    }

    fn base(key: &'static str) -> Manifest {
        Manifest::new(key, "a test module").category(Category::Enhancer)
    }

    static NEEDS_POOL: Fake = Fake(|| base("needspool").requires::<PgPool>("rows"));
    static NEEDS_POOL_UNLESS: Fake = Fake(|| {
        base("lifted").requires_unless::<PgPool>(&["lifted-memory"], "rows unless in memory")
    });
    static LIFTED_MEMORY: Fake = Fake(|| {
        base("lifted-memory")
            .part_of_but_off("lifted")
            .because("a backend")
    });
    static OTHER_POOL: Fake = Fake(|| base("otherpool").provides::<PgPool>("a second pool"));
    static DEV_POOL: Fake = Fake(|| {
        base("devpool")
            .provides::<PgPool>("a pool in the binary")
            .development_only("the data lives in a temporary directory")
    });

    fn extra(modules: &[&'static dyn Module], f: impl FnOnce()) {
        crate::modules::with_extra(modules, f);
    }

    #[test]
    fn a_single_provider_is_added_and_said() {
        extra(&[&NEEDS_POOL], || {
            let (picks, added) = resolve(&["needspool"], Door::Create).expect("resolves");
            assert!(picks.contains(&"db"), "{picks:?}");
            assert_eq!(added.len(), 1);
            assert_eq!(
                added[0].line(),
                "added db: needspool requires PgPool (a Postgres pool)"
            );
            assert_eq!(why(&picks, "PgPool"), "needspool requires PgPool");
        });
    }

    #[test]
    fn jobs_brings_db_and_jobs_memory_does_not() {
        let (picks, added) = resolve(&["jobs"], Door::Create).expect("resolves");
        assert_eq!(picks, ["db", "jobs"]);
        assert_eq!(
            added[0].line(),
            "added db: jobs requires PgPool (a Postgres pool)"
        );
        let (picks, added) = resolve(&["jobs", "jobs-memory"], Door::Create).expect("resolves");
        assert_eq!(picks, ["jobs", "jobs-memory"]);
        assert!(added.is_empty());
        let (picks, _) = resolve(&["cache", "cache-memory"], Door::Create).expect("resolves");
        assert!(!picks.contains(&"db"), "{picks:?}");
    }

    #[test]
    fn a_chosen_provider_is_bound_and_nothing_is_added() {
        let (picks, added) = resolve(&["db", "jobs"], Door::Create).expect("resolves");
        assert_eq!(picks, ["db", "jobs"]);
        assert!(added.is_empty());
        assert_eq!(bindings(&picks), [("PgPool", "db"), ("JobQueue", "jobs")]);
        assert!(after("jobs", &picks).contains(&"db"));
        assert_eq!(why(&["db"], "PgPool"), "chosen");
    }

    #[test]
    fn a_requirement_lifted_by_a_child_adds_nothing() {
        extra(&[&NEEDS_POOL_UNLESS, &LIFTED_MEMORY], || {
            let (picks, added) =
                resolve(&["lifted", "lifted-memory"], Door::Create).expect("resolves");
            assert!(added.is_empty());
            assert!(!picks.contains(&"db"));
            let (picks, _) = resolve(&["lifted"], Door::Create).expect("resolves");
            assert!(picks.contains(&"db"));
        });
    }

    #[test]
    fn several_providers_are_an_error_that_names_each() {
        extra(&[&NEEDS_POOL, &OTHER_POOL], || {
            let refused = resolve(&["needspool"], Door::Create).expect_err("two providers");
            let said = refused[0].render();
            assert!(said.contains("`needspool` requires PgPool"), "{said}");
            assert!(said.contains("add one of: db, otherpool"), "{said}");
        });
    }

    #[test]
    fn a_development_only_provider_is_never_added_and_binds_when_chosen() {
        extra(&[&NEEDS_POOL, &DEV_POOL], || {
            assert_eq!(providers("PgPool"), ["db", "devpool"]);
            assert_eq!(single_provider("PgPool"), Some("db"));
            let (picks, added) = resolve(&["needspool"], Door::Create).expect("resolves");
            assert_eq!(added[0].key, "db");
            assert!(!picks.contains(&"devpool"));
            let (picks, added) =
                resolve(&["needspool", "devpool"], Door::Create).expect("resolves");
            assert!(added.is_empty());
            assert_eq!(bindings(&picks), [("PgPool", "devpool")]);
        });
    }

    #[test]
    fn a_missing_provider_is_an_error_with_a_help_line() {
        // Only a development-only provider is not one to add.
        let dev_only = Fake(|| {
            base("db")
                .provides::<PgPool>("a pool")
                .development_only("test")
        });
        let dev_only: &'static Fake = Box::leak(Box::new(dev_only));
        extra(&[&NEEDS_POOL, dev_only], || {
            let refused = resolve(&["needspool"], Door::Add).expect_err("nothing to add");
            let said = refused[0].render();
            assert!(said.contains("requires PgPool"), "{said}");
            assert!(said.contains("add one of: db"), "{said}");
        });
        // The floor under the front doors: no resolution, and the help
        // names the one module to add.
        let said = refusals(&["jobs"], Door::Add)[0].render();
        assert!(said.contains("brocanth add db jobs"), "{said}");
    }

    #[test]
    fn two_chosen_providers_conflict() {
        extra(&[&OTHER_POOL], || {
            let said = doubled(&["db", "otherpool"])[0].render();
            assert!(
                said.contains("`db` and `otherpool` each provide PgPool"),
                "{said}"
            );
            assert!(said.contains("keep one of: db, otherpool"), "{said}");
        });
    }

    #[test]
    fn the_capability_kinds_bind_their_provider_and_a_child_refines_it() {
        let picks = ["db", "cache", "cache-redis", "jobs", "jobs-memory", "auth"];
        assert!(doubled(&picks).is_empty());
        assert_eq!(
            bindings(&picks),
            [
                ("PgPool", "db"),
                ("CacheStore", "cache"),
                ("JobQueue", "jobs"),
                ("SessionCookie", "auth")
            ]
        );
        // mail uses the queue, so it writes after whichever provides it.
        assert!(after("mail", &["jobs", "jobs-memory", "mail"]).contains(&"jobs"));
        assert!(!after("mail", &["mail"]).contains(&"jobs"));
    }

    #[test]
    fn two_session_cookies_conflict() {
        let said = doubled(&["sessions", "auth"])[0].render();
        assert!(
            said.contains("`sessions` and `auth` each provide SessionCookie"),
            "{said}"
        );
    }

    #[test]
    fn removing_the_provider_names_who_requires_it() {
        let orphans = orphaned_by("db", &["jobs", "flags"]);
        let keys: Vec<&str> = orphans.iter().map(|(k, _, _)| *k).collect();
        assert_eq!(keys, ["jobs", "flags"]);
        assert!(orphaned_by("db", &["jobs", "jobs-memory"]).is_empty());
    }

    // -- the default provider (docs/object-storage.md §3) ------------------

    #[test]
    fn a_default_is_one_per_kind_deployable_and_among_several() {
        let mut seen: Vec<&str> = Vec::new();
        for key in available() {
            let m = about(key);
            for kind in &m.defaults {
                assert!(
                    m.gives(kind.name),
                    "`{key}` defaults {kind} it does not provide"
                );
                assert!(m.development_only.is_none(), "`{key}` is development-only");
                assert_eq!(kind.cardinality, Cardinality::One, "{kind}");
                assert!(providers(kind.name).len() > 1, "{kind} has one provider");
                assert!(!seen.contains(&kind.name), "two defaults for {kind}");
                seen.push(kind.name);
            }
        }
        assert_eq!(default_provider("ObjectStore"), Some("storage-fs"));
        assert_eq!(default_provider("CacheStore"), None);
    }

    #[test]
    fn storage_brings_the_default_store_and_says_so() {
        let (picks, added) = resolve(&["storage"], Door::Create).expect("resolves");
        assert_eq!(picks, ["storage", "storage-fs"]);
        assert_eq!(
            added[0].line(),
            "added storage-fs: storage requires ObjectStore (an object store)"
        );
        // A chosen backend meets the requirement: nothing is added.
        let (picks, added) = resolve(&["storage", "storage-s3"], Door::Create).expect("resolves");
        assert!(added.is_empty());
        assert_eq!(bindings(&picks), [("ObjectStore", "storage-s3")]);
        // `storage-db` brings `db`, once, with `jobs` beside it.
        let (picks, _) =
            resolve(&["storage", "storage-db", "jobs"], Door::Create).expect("resolves");
        assert_eq!(picks.iter().filter(|p| **p == "db").count(), 1);
        assert!(!picks.contains(&"storage-fs"));
    }

    #[test]
    fn two_object_stores_conflict_under_one_parent() {
        let said = doubled(&["storage", "storage-fs", "storage-s3"])[0].render();
        assert!(
            said.contains("`storage-fs` and `storage-s3` each provide ObjectStore"),
            "{said}"
        );
        // A child that refines a parent which provides the Kind still does
        // not conflict.
        assert!(doubled(&["cache", "cache-redis"]).is_empty());
    }

    #[test]
    fn adding_a_backend_replaces_the_one_recorded_and_nothing_else() {
        let (picks, replaced) = replace(&["storage", "storage-fs"], &["storage-s3"]);
        assert_eq!(picks, ["storage"]);
        assert_eq!(
            replaced[0].line(),
            "replaced storage-fs: storage-s3 provides ObjectStore (an object store)"
        );
        let (picks, _) = replace(&["storage", "storage-s3"], &["storage-gcs"]);
        assert_eq!(picks, ["storage"]);
        // A Kind with no default is not a backend choice: two cookies stay
        // two cookies, and `doubled` refuses them.
        let (picks, replaced) = replace(&["auth"], &["sessions"]);
        assert_eq!(picks, ["auth"]);
        assert!(replaced.is_empty());
    }

    #[test]
    fn removing_a_backend_brings_the_default_back() {
        assert!(orphaned_by("storage-s3", &["storage"]).is_empty());
        let (picks, added) = resolve(&["storage"], Door::Add).expect("resolves");
        assert!(picks.contains(&"storage-fs"));
        assert_eq!(added[0].key, "storage-fs");
        // The default itself cannot leave while `storage` stays.
        let orphans = orphaned_by("storage-fs", &["storage"]);
        assert_eq!(orphans[0].0, "storage");
    }

    #[test]
    fn a_record_from_before_the_kind_keeps_its_backend() {
        let mut with_db = vec!["db", "storage"];
        assert_eq!(upgrade_record(&mut with_db), Some("storage-db"));
        assert!(with_db.contains(&"storage-db"));
        let mut alone = vec!["storage"];
        assert_eq!(upgrade_record(&mut alone), Some("storage-fs"));
        let mut current = vec!["storage", "storage-s3"];
        assert_eq!(upgrade_record(&mut current), None);
        let mut none = vec!["db"];
        assert_eq!(upgrade_record(&mut none), None);
    }
}
