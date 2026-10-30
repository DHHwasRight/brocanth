//! `resource` -- what `brocanth create` writes when somebody asks for typed
//! CRUD routes.
//!
//! This crate is the records API: typed routes over one table, with the
//! policy in the `WHERE` clause rather than in a filter that runs after the
//! page has been read.
//!
//! # What the module writes, and what it does not
//!
//! It writes the seam and no entity. `src/plugins/resource/mod.rs` holds:
//!
//! * the three impls that make the project's own `Ctx`, `Response` and
//!   `Error` a page triple ([`PageCtx`](crate::pages::PageCtx),
//!   [`PageResponse`](crate::pages::PageResponse),
//!   [`PageError`](crate::pages::PageError)). They delegate, one line each,
//!   and the compiler checks them against the generated types;
//! * a `mount` with the shape `Slot::RouterMounts` expects, which mounts
//!   every resource the application lists. The list starts empty.
//!
//! `src/pages/mod.rs` calls that `mount` at `/api`, so a resource added to the
//! list is in the one table `dispatch` consults, and a path a page and a
//! resource both claim is a build error.
//!
//! # Why no example entity
//!
//! A [`Resource`](crate::Resource) names an
//! [`Entity`](brocanth_policy::Entity), a
//! [`Policy`](brocanth_policy::Policy), a [`ResourceData`](crate::ResourceData)
//! that runs SQL, and three functions that turn an id, a body and a row into
//! and out of the application's own types. None of those has a default and
//! none can be guessed. An invented `Post` would bring an entity, a policy, a
//! data source and a migration for a table nobody asked for, and deleting
//! them is the first thing anybody would do.
//!
//! So the module stops where guessing starts. The list is a registry, the
//! way the admin's model list is: empty is accurate, not broken. Until it
//! has an entry, the table holds no route under `/api`.
//!
//! Compiled only under the `scaffold` feature, which is off. See the
//! dependency comment in `Cargo.toml`.

use brocanth_scaffold::{Dep, Manifest, Module, Project, Scaffold, Slot};

/// The records API, as a choice on the `brocanth create` menu.
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
        .krate(
            Dep::brocanth("brocanth-resource")
                // The page-shaped surface and nothing else. `plugin` is the
                // `RouteSource`, which a generated table cannot mount.
                .no_default_features()
                .feature("pages"),
        )
        .absent(
            "brocanth-resource",
            "no records API; every CRUD route is a page somebody wrote by hand",
        )
    }

    fn contribute(&self, _project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/resource/mod.rs", RESOURCE_RS);
        s.at(Slot::PluginsMod).line("pub mod resource;");
        // The prefix is the application's decision, so it is written at the
        // call and not inside `mount`. No `PagesRoutes` line: the list of
        // resources is empty, so the table gains no route.
        s.at(Slot::RouterMounts)
            .line("    crate::plugins::resource::mount(routes, \"/api\")?;");
    }
}

/// `src/plugins/resource/mod.rs`.
const RESOURCE_RS: &str = r##"//! Typed CRUD routes over your tables, with the policy in the `WHERE` clause.
//!
//! Written once by `brocanth create`. This file is yours.
//!
//! [`mount`] is the list of resources this application serves, and it starts
//! empty: a resource is one table, and a new project has none. Add one line
//! per resource. A resource is a `brocanth_resource::Resource`: an entity, a
//! policy over its rows (there is no default, so a resource without one does
//! not compile), a data source and three conversions. That crate's own
//! documentation has the worked example.
//!
//! `src/pages/mod.rs` calls [`mount`] at `/api`, so every route lands in the
//! one table. A page and a resource that claim one path are a build error.
//!
//! The three impls below make this project's `Ctx`, `Response` and `Error`
//! the page triple `brocanth_resource::pages` is written against. A resource
//! also implements `PageSubject<Ctx>`, which reads who is asking off a
//! request.

use brocanth::routing::{BuildError, RouterBuilder};
use brocanth_resource::pages::{PageCtx, PageError, PageResponse};

use crate::pages::{Ctx, Error, Handler, Response};

/// Every resource, under `prefix`.
///
/// Seven routes each: the five REST routes, and `POST /{id}` and
/// `POST /{id}/delete` for an HTML form, which can send nothing but GET and
/// POST. `brocanth_resource::pages::mount_api` mounts the five alone.
///
/// ```ignore
/// brocanth_resource::pages::mount(routes, &format!("{prefix}/posts"), Arc::new(Posts::new()))?;
/// ```
pub fn mount(routes: &mut RouterBuilder<Handler>, prefix: &str) -> Result<(), BuildError> {
    let _ = (routes, prefix);
    Ok(())
}

impl PageCtx for Ctx {
    fn query(&self) -> &str {
        &self.query
    }

    fn body(&self) -> &[u8] {
        &self.body
    }

    fn param(&self, name: &str) -> Option<&str> {
        Ctx::param(self, name)
    }

    fn header(&self, name: &str) -> Option<&str> {
        Ctx::header(self, name)
    }
}

impl PageResponse for Response {
    fn page_response(status: u16, body: String, headers: Vec<(String, String)>) -> Self {
        Response {
            status,
            body,
            headers,
        }
    }
}

impl PageError for Error {
    fn page_error(status: u16, message: String) -> Self {
        Error::new(status, message)
    }
}
"##;
