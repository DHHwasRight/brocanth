//! Kinds: the typed capabilities one module gives and another uses.
//!
//! A module names what it needs by the thing, not by the module that gives
//! it: `requires::<PgPool>`, not `needs("db")`. The CLI resolves each Kind to
//! one provider at generation time, writes the binding as a comment over the
//! plugin chain and in `brocanth.toml`, and hands it to every module as
//! [`Project::provider`](crate::Project::provider). Nothing is discovered at
//! run time, so ADR 1 holds: the chain stays written out.
//!
//! A Kind is a marker type, so a typo does not compile. It names its runtime
//! type as a path string for prose and comments; the generated code names
//! the real type, and the compiler checks that code.
//!
//! `docs/module-capabilities.md` has the design. Keep the set small: a new
//! Kind needs two providers or two consumers that exist (§11 there).

use std::fmt;

/// How many providers a Kind takes in one project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cardinality {
    /// One provider. A requirement with none chosen is resolved by the CLI;
    /// two chosen providers are a conflict.
    One,
    /// Any number of contributors, collected by one reader. None is not an
    /// error.
    Many,
}

/// A typed capability. Implement it with [`kind!`](crate::kind!), on a unit
/// struct in this module: modules outside the workspace use the core Kinds
/// and define none (owner decision 5 in `docs/module-capabilities.md`).
pub trait Kind: 'static {
    /// The name in diagnostics, comments and `brocanth.toml`: `PgPool`.
    const NAME: &'static str;
    /// The domain that owns it, for `explain` and the docs: `db`.
    const GROUP: &'static str;
    /// The runtime type the generated code reads, as a path.
    const RUNTIME: &'static str;
    /// What it is, in a few words, for a diagnostic: `a Postgres pool`.
    const GIST: &'static str;
    /// One provider, or many contributors.
    const CARDINALITY: Cardinality;
}

/// A Kind as a value, so a [`Manifest`](crate::Manifest) can hold it.
///
/// Two refs are the same Kind when their names are the same: the name is
/// unique across the set, and the set is closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KindRef {
    /// [`Kind::NAME`].
    pub name: &'static str,
    /// [`Kind::GROUP`].
    pub group: &'static str,
    /// [`Kind::RUNTIME`].
    pub runtime: &'static str,
    /// [`Kind::GIST`].
    pub gist: &'static str,
    /// [`Kind::CARDINALITY`].
    pub cardinality: Cardinality,
}

impl KindRef {
    /// The value of the Kind `K`.
    pub fn of<K: Kind>() -> Self {
        Self {
            name: K::NAME,
            group: K::GROUP,
            runtime: K::RUNTIME,
            gist: K::GIST,
            cardinality: K::CARDINALITY,
        }
    }
}

impl fmt::Display for KindRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name)
    }
}

/// Declare a Kind: a unit struct and its [`Kind`] impl.
#[macro_export]
macro_rules! kind {
    ($(#[$doc:meta])* $name:ident, group = $group:literal, runtime = $runtime:literal, gist = $gist:literal, $card:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy)]
        pub struct $name;
        impl $crate::kind::Kind for $name {
            const NAME: &'static str = stringify!($name);
            const GROUP: &'static str = $group;
            const RUNTIME: &'static str = $runtime;
            const GIST: &'static str = $gist;
            const CARDINALITY: $crate::kind::Cardinality = $crate::kind::Cardinality::$card;
        }
    };
}

kind!(
    /// The project's one shared Postgres pool. `db` provides it, and no
    /// other module opens a pool of its own: one set of connections, sized
    /// by `[db] max_connections`, one URL, one place for TLS.
    PgPool,
    group = "db",
    runtime = "brocanth_db::Db",
    gist = "a Postgres pool",
    One
);

kind!(
    /// A path where a module's routes answer, with a [`MountRole`]. Each
    /// [`Opt::mount`](crate::Opt::mount) of a chosen module contributes
    /// one. Readers ask [`Project::mounts_with`](crate::Project::mounts_with)
    /// for a role, and name no module: robots and analytics leave out every
    /// `Backoffice` mount, i18n keeps every mount that is not `Public`
    /// unprefixed.
    Mount,
    group = "http",
    runtime = "a path constant in the module file, with a role",
    gist = "a mounted path",
    Many
);

kind!(
    /// A tab in the backoffice. A module contributes one with
    /// [`Manifest::contributes`](crate::Manifest::contributes), and names its
    /// RBAC resource there; `admin` collects them. A tab reaches application
    /// data only through a handle that could become remote
    /// (`docs/module-capabilities.md` §5.1).
    AdminPanel,
    group = "admin",
    runtime = "brocanth_admin::panel::Panel",
    gist = "a backoffice tab",
    Many
);

/// Who a mounted path serves. A reader asks for a role, never for a
/// module key (`docs/module-capabilities.md` §5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MountRole {
    /// Pages for visitors. Robots may crawl them, analytics counts them.
    #[default]
    Public,
    /// The backoffice: staff only. Never crawled, never counted, never
    /// localised. It can move to another service later.
    Backoffice,
    /// Called by other programs: an API, an upload endpoint, an OAuth
    /// callback. Not a page a person reads.
    Machine,
}

impl MountRole {
    /// The role's name, for prose and diagnostics.
    pub fn name(self) -> &'static str {
        match self {
            MountRole::Public => "Public",
            MountRole::Backoffice => "Backoffice",
            MountRole::Machine => "Machine",
        }
    }
}

/// One contribution to a Kind of cardinality [`Cardinality::Many`], as a
/// manifest holds it. Build a tab with [`Tab::new`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Contribution {
    /// The contribution's name. For an [`AdminPanel`] it is the tab's RBAC
    /// resource: its `Panel::key`, and the first part of the permissions
    /// `<resource>.read`, `<resource>.edit` and `<resource>.owner`.
    pub name: &'static str,
    /// What the project gets, for the module file and `explain`.
    pub what: &'static str,
}

/// An [`AdminPanel`] contribution: a tab and its RBAC resource.
pub type Tab = Contribution;

impl Contribution {
    /// A contribution named `name`. For a tab, `name` is the RBAC resource:
    /// lower case letters, digits, `-` and `_`.
    pub const fn new(name: &'static str, what: &'static str) -> Self {
        Self { name, what }
    }

    /// Whether `name` is a valid resource: lower case ASCII letters,
    /// digits, `-` and `_`, starting with a letter. A permission string is
    /// `<resource>.<level>`, so a dot or a slash would make it ambiguous.
    pub fn valid_name(name: &str) -> bool {
        name.starts_with(|c: char| c.is_ascii_lowercase())
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'))
    }
}

/// A requirement on a Kind, as a manifest holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    /// What is required.
    pub kind: KindRef,
    /// What breaks without it, printed when the CLI adds or asks for the
    /// provider.
    pub why: &'static str,
    /// Keys whose choice lifts the requirement: the module's own children
    /// that replace the backend that needs it. Empty for most.
    pub unless: &'static [&'static str],
}

impl Requirement {
    /// Whether the requirement holds in a project of `picks`.
    pub fn active(&self, picks: &[&str]) -> bool {
        !self.unless.iter().any(|key| picks.contains(key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_is_named_after_its_type() {
        let pool = KindRef::of::<PgPool>();
        assert_eq!(pool.name, "PgPool");
        assert_eq!(pool.group, "db");
        assert_eq!(pool.cardinality, Cardinality::One);
        assert_eq!(pool.to_string(), "PgPool");
    }

    #[test]
    fn a_resource_is_one_lower_case_word() {
        assert!(Contribution::valid_name("cache"));
        assert!(Contribution::valid_name("oauth-connections"));
        assert!(!Contribution::valid_name(""));
        assert!(!Contribution::valid_name("Cache"));
        assert!(!Contribution::valid_name("cache.edit"));
        assert!(!Contribution::valid_name("a/b"));
        assert_eq!(KindRef::of::<AdminPanel>().cardinality, Cardinality::Many);
        assert_eq!(KindRef::of::<Mount>().cardinality, Cardinality::Many);
    }

    #[test]
    fn a_requirement_is_lifted_by_a_named_child() {
        let r = Requirement {
            kind: KindRef::of::<PgPool>(),
            why: "the jobs are rows",
            unless: &["jobs-memory"],
        };
        assert!(r.active(&["jobs"]));
        assert!(!r.active(&["jobs", "jobs-memory"]));
    }
}
