//! `resource` -- listed on the `brocanth create` menu, and not available.
//!
//! This crate is the records API: typed routes over one table, with the
//! policy in the `WHERE` clause rather than in a filter that runs after the
//! page has been read. It is written, it is tested against a real engine, and
//! a generated project can now mount it -- see [`crate::pages`]. What it still
//! has no answer for is which table, which is why this module says so on the
//! menu, where the question is asked, rather than leaving the crate off it and
//! letting somebody ask again next month. `brocanth-realtime`'s two
//! transports are listed the same way, for the same reason.
//!
//! # Why it is blocked
//!
//! One reason now, and it is about the project rather than about the
//! framework. The first reason is answered.
//!
//! **1. The routes reach a generated project. Answered.**
//! [`pages::mount`](crate::pages::mount) writes the seven routes straight into
//! the `RouterBuilder` a generated `src/pages/mod.rs` builds its table from,
//! so they are in the one table `dispatch` consults and a path two sources
//! both claim is a build error. `ResourceRoutes` stays beside it for a host
//! that mounts a `RouteSource`; both drive [`flow`](crate::flow) and render
//! through [`render`](crate::render), so there is one authorisation path.
//!
//! **2. A resource is one entity, and a new project has none.**
//! [`Resource`](crate::Resource) names an
//! [`Entity`](brocanth_policy::Entity), a
//! [`Policy`](brocanth_policy::Policy) over that entity, a
//! [`ResourceData`](crate::ResourceData) that reads it, and three functions
//! that turn an id, a body and a row into and out of the application's own
//! types. None of those has a default and none of them can be guessed. The
//! admin solves the same problem by accepting an empty list --
//! `admin_source!` with no entities is a backoffice that says "nothing to
//! administer" and shows its contributed panels, which is accurate rather
//! than broken. That answer does not transfer: an admin is a registry and an
//! empty registry is still an admin, where a resource *is* its entity and
//! there is no inhabitant of `Resource` with no table behind it. Scaffolding
//! an invented `Post` would mean generating an entity, a policy, a data
//! source and a migration for a table nobody asked for -- the crate's own
//! fixture is six hundred lines for exactly that -- and then deleting all of
//! it is the first thing anybody would do.
//!
//! The second reason is also why `Manifest::needs("model", ..)` would be a
//! lie: the ORM does not help. `brocanth-policy`'s `Entity` is written by
//! hand, has no derive, and nothing in this workspace turns a SeaORM entity
//! into one -- `brocanth-policy`'s own dependency comment calls that adapter
//! "a later crate", and it does not exist yet. So a project with `model` on
//! is no closer to a resource than a project without it.
//!
//! # What would unblock it
//!
//! Something for `contribute` to write. Reason 2 is the whole of what is
//! left, so the question is no longer "can these routes be reached" but "over
//! which table", and a module answers it one of two ways:
//!
//! 1. It asks. A module that takes a table name, a primary key and a column
//!    list can write the entity, the policy, the data source and the four
//!    lines that call [`pages::mount`](crate::pages::mount) -- and a
//!    migration, which is `model`'s business and not this crate's.
//! 2. It reads. An adapter from a SeaORM entity to
//!    [`Entity`](brocanth_policy::Entity) would make `model` the source of
//!    the answer. `brocanth-policy`'s own dependency comment calls that
//!    adapter "a later crate", and it does not exist yet.
//!
//! Until one of those is settled, a module here would scaffold an invented
//! `Post`, and deleting it is the first thing anybody would do.
//!
//! Compiled only under the `scaffold` feature, which is off. See the
//! dependency comment in `Cargo.toml`.

use brocanth_scaffold::{Manifest, Module, Project, Scaffold};

/// The records API, listed and not available.
///
/// A unit struct, because a module is asked once per run and everything it
/// needs arrives in [`Project`].
/// Named `Records` and not `Resource`, which is what it scaffolds.
///
/// `brocanth_resource::Resource` is this crate's own trait. A second item
/// under that name in the same crate makes rustc print the fully qualified
/// path in every error that mentions it — which broke a UI test pinning
/// that message the moment this feature was enabled. PocketBase calls them
/// records; so does this.
pub struct Records;

impl Module for Records {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "resource",
            "typed CRUD routes over one table, with the policy in the WHERE clause",
        )
        .asked_in("doors", "")
        .blocked("a resource is one entity, and a new project has none to name")
    }

    fn contribute(&self, _project: &Project, s: &mut Scaffold) {
        // Never reached while the CLI honours `Manifest::blocked`. Here so
        // that a CLI which does not stops rather than writing a project whose
        // routes answer nothing.
        s.refuse(
            "resource cannot be scaffolded: it needs an entity, a policy and a data source, \
             and a new project has none of the three. The routes themselves reach a \
             generated project through `brocanth_resource::pages::mount`, which writes them \
             into the table `src/pages/mod.rs` builds",
        );
    }
}
