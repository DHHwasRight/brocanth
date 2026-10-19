//! `resource` -- listed on the `brocanth create` menu, and not available.
//!
//! This crate is the records API: five typed routes over one table, with the
//! policy in the `WHERE` clause rather than in a filter that runs after the
//! page has been read. It is written, it is tested against a real engine, and
//! a generated project cannot reach it. This module says so on the menu,
//! where the question is asked, rather than leaving the crate off it and
//! letting somebody ask again next month. `brocanth-realtime`'s two
//! transports are listed the same way, for the same reason.
//!
//! # Why it is blocked
//!
//! Two reasons, and only the first is about the framework rather than about
//! the project.
//!
//! **1. A generated application dispatches `src/pages` and nothing else.**
//! `ResourceRoutes` is a
//! `RouteSource`, which is the core's
//! bundle seam: the application writes
//! `app.mount_routes::<ResourceRoutes<Posts>>("/api/collections/posts/records")?`
//! and the five routes appear under that prefix. Nothing a module may write
//! can put that line in a generated project, and nothing in a generated
//! project reads `App::route_plan` afterwards: `src/app.rs` builds its table
//! from `pages::router()` and `dispatch` consults that table alone. A module
//! that wrote the plugin anyway would produce five routes that answer
//! nothing, which is worse than the row you are reading. The fix is in the
//! generator and in the application template, not here.
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
//! Two lines in the generated `src/app.rs`, and then this module can be
//! written in an afternoon:
//!
//! 1. `build()` mounts the bundles the application chose, after the plugins
//!    are registered and before the stack is assembled.
//! 2. `dispatch` consults `app.route_plan()` when `pages::router()` has no
//!    answer -- or `pages::router()` is seeded from it, which keeps one
//!    table and one `Dispatch`.
//!
//! Neither is this crate's to write. Both are in `brocanth-cli`'s
//! `app_rs`, which is the file a developer owns.
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
            "five typed CRUD routes over one table, with the policy in the WHERE clause",
        )
        .blocked("a generated app dispatches src/pages; these routes arrive as a RouteSource")
    }

    fn contribute(&self, _project: &Project, s: &mut Scaffold) {
        // Never reached while the CLI honours `Manifest::blocked`. Here so
        // that a CLI which does not stops rather than writing a project whose
        // routes answer nothing.
        s.refuse(
            "resource cannot be scaffolded: its five routes are a RouteSource, which an \
             application mounts with `App::mount_routes` and dispatches from \
             `App::route_plan`, and a generated `src/app.rs` builds its table from \
             `pages::router()` and consults that alone. A resource also needs an entity, a \
             policy and a data source, and a new project has no entities",
        );
    }
}
