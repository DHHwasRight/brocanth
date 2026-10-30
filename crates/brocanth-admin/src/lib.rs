//! A Django-admin-shaped backoffice for brocanth, configured through rsx.
//!
//! Four screens, which is what an admin is: a list with pagination, search and
//! sortable columns; a detail view; one form that creates and edits; and a
//! delete behind a confirmation. Forms come from the schema, with a widget per
//! column type and a foreign-key picker that shows a person's name rather than
//! their row id.
//!
//! Two things make it worth having rather than writing by hand.
//!
//! **Every screen is replaceable with your own rsx component.** That is the
//! point of the crate, so the seam was designed first and the shipped screens
//! are its first consumer: [`Views`] holds one [`View`] per screen and
//! `Views::default()` is the templates in `src/templates`. An override is a
//! closure around a component and nothing else, which means the extension
//! path is the path the defaults take.
//!
//! **It ships locked.** The admin has no authentication scheme — PLAN §8
//! forbids assuming one — so it takes a [`Policy`] the application supplies,
//! and the default is [`DenyAll`]. An admin mounted without a policy answers
//! 403 to its own index. Shipping open and asking people to remember to lock
//! it is a security incident waiting for someone's first deploy.
//!
//! **Rows go through the entity's policy.** The [`Policy`] above decides
//! which screens a person may open. Which *rows* those screens show and
//! change is `brocanth-policy`'s typed predicate, registered per model with
//! [`AdminConfig::resource`]: the list and its count filter in the query,
//! a record is read with a guarded select, a write is guarded by the
//! condition it was checked against, and a form accepts only the columns the
//! field mask allows. A model with no resource answers 403. See [`rows`].
//!
//! **A tab can come from another crate.** Four screens about a table is not
//! a whole backoffice: a queue has a dashboard and a kill button, a feature
//! flag has a switch, and neither is a row. [`Panel`](panel::Panel) is a tab
//! that renders its own body and is given this chrome for free. It lives in
//! the crate that owns the data, because this one keeps its dependencies few
//! and intends to keep them so.
//!
//! # Modularity
//!
//! Five dependencies: `brocanth`, `rsx`, `async-trait`, `thiserror`, and
//! `brocanth-policy`, whose own only dependency is `thiserror`. No SQL
//! driver, no ORM, no serde, no async runtime, no TOML. The admin takes an
//! [`AdminData`] the application supplies and never learns what is behind it;
//! PLAN §8.4 says the ORM is recommended and absent, and that applies to a
//! plugin as much as to the core. An application that does not name this crate
//! in `main` pays nothing for it, not even a template.
//!
//! # Getting started
//!
//! ```
//! use std::sync::Arc;
//!
//! use brocanth_admin::policy::{policy_fn, Action, Principal};
//! use brocanth_admin::{Admin, AdminConfig, Discovery, Fixture};
//!
//! struct Staff;
//!
//! let data = Arc::new(Fixture::blog());
//! let admin = Admin::new(
//!     data,
//!     AdminConfig::new()
//!         .titled("Blog admin")
//!         .at("/admin")
//!         // The registry holds SeaORM's migration table too; this is what
//!         // keeps it out.
//!         .discovering(Discovery::new().under("blog::models"))
//!         .with_policy(policy_fn(|principal: &Principal<'_>, request| {
//!             principal.downcast::<Staff>().is_some() && request.action != Action::Delete
//!         }))
//!         // The in-memory fixture cannot run SQL, so it cannot take a row
//!         // policy. A real application registers one per model with
//!         // `.resource(..)` instead; see the `rows` module.
//!         .unscoped_rows_unsafe_for_development()
//!         .model("post", |m| {
//!             m.list_columns(["id", "title", "author_id", "published"])
//!                 .searchable(["title", "body"])
//!                 .foreign_key("author_id", "user", "id")
//!         })
//!         .model("user", |m| {
//!             m.display(|row| row.get("name").map(ToString::to_string).unwrap_or_default())
//!         }),
//! );
//!
//! assert_eq!(admin.model_keys(), ["post", "user"]);
//!
//! let staff = Staff;
//! let response = brocanth_admin::rt::block_on(admin.dispatch(
//!     Principal::new(&staff),
//!     "GET",
//!     "/admin/post",
//!     "page=1",
//!     &Default::default(),
//! ))
//! .unwrap();
//!
//! assert_eq!(response.status(), 200);
//! // The list shows the author's name, not their id.
//! assert!(response.html().unwrap().contains("Ada Lovelace"));
//! ```
//!
//! # Replacing a screen
//!
//! ```ignore
//! // src/views/admin.rsx — `Element` is implicit; never `use rsx::Element`.
//! use brocanth_admin::view::ListCtx;
//!
//! pub fn PostList(ctx: &ListCtx) -> Element {
//!     <h1>{&ctx.model.plural}</h1>
//!     <ol>
//!         for row in &ctx.rows {
//!             <li><a href={&row.href}>{&row.label}</a></li>
//!         }
//!     </ol>
//! }
//! ```
//!
//! ```ignore
//! use brocanth_admin::{AdminConfig, Views};
//! use brocanth_admin::view::ListCtx;
//! use rsx::Template;
//!
//! mod views { rsx::load_dir!("src/views"); }
//!
//! let config = AdminConfig::new().with_views(
//!     Views::default()
//!         .with_list(|ctx: &ListCtx, w: &mut rsx::Writer<'_>| {
//!             views::admin::PostList { ctx }.render(w)
//!         }),
//! );
//! ```
//!
//! # What was assumed, and what SeaORM cannot do
//!
//! The model layer is being written in parallel, so this crate depends on
//! neither it nor SeaORM: [`data::AdminData`] is the small trait it needs,
//! [`Fixture`] implements it in memory, and [`adapter`] is the one module to
//! fill in. Every assumption, and the four things SeaORM 2.0.4 does not report
//! that the admin wanted, are written down in [`adapter`].

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod assets;

pub mod adapter;
pub mod admin;
pub mod config;
pub mod data;
pub mod discovery;
pub mod fixture;
pub mod form;
pub mod model;
pub mod paging;
pub mod panel;
#[cfg(feature = "plugin")]
pub mod plugin;
pub mod policy;
pub mod query;
pub mod rows;
pub mod rt;
pub mod view;

/// What `brocanth create` writes for `admin`, and why `admin-model` is listed
/// and not available.
///
/// Behind an off-by-default feature. The one program that reads it is the
/// CLI.
#[cfg(feature = "scaffold")]
pub mod scaffold;

/// The shipped screens.
///
/// Compiled into the binary by rsx at macro expansion: nothing is read from
/// disk at runtime, and an application that replaces every view still pays for
/// these, which is the one cost of shipping defaults at all.
mod templates {
    rsx::load_dir!("src/templates");
}

pub use admin::{Admin, AdminError, Response, Route};
pub use config::{AdminConfig, ModelAdmin};
pub use data::{AdminData, DataError, FieldErrors, ListRequest, Scope, Search, SharedData};
pub use rows::{AdminResource, RowAction};

/// Re-exported so an [`AdminResource`] and its policy name one crate.
pub use brocanth_policy as row_policy;
pub use discovery::{discover, Discovery, RegistryEntry};
pub use fixture::Fixture;
pub use form::{widget_for, Choice, Field, Form, Submission, Widget};
pub use model::{ColumnDescriptor, ColumnKind, ModelDescriptor, PrimaryKey, Relation, Row, Value};
pub use paging::Page;
#[cfg(feature = "plugin")]
pub use plugin::{AdminPlugin, AdminSetup};
pub use policy::{policy_fn, Action, DenyAll, Policy, Principal};
pub use query::{Direction, ListQuery, Sort};
pub use view::{View, Views};

/// Re-exported so a template file and an override need one dependency, and so
/// the rsx this crate was built against is the rsx an override compiles with.
pub use rsx;
