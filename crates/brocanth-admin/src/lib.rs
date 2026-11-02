#![doc = include_str!("../README.md")]
//!
//! # Design notes
//!
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

/// The shipped screens as source, for `brocanth admin eject`.
///
/// Behind the same off-by-default feature as [`scaffold`]: it embeds the
/// template text, which no application needs at run time.
#[cfg(feature = "scaffold")]
pub mod eject;

/// The shipped screens.
///
/// Compiled into the binary by rsx at macro expansion: nothing is read from
/// disk at runtime, and an application that replaces every view still pays for
/// these, which is the one cost of shipping defaults at all.
mod templates {
    rsx::load_dir!("src/templates");
}

pub use admin::{Admin, AdminError, Response, Route};

/// The one inline script of the admin layout that is not dp-swap's own
/// partial script. A Content-Security-Policy allows it by its hash:
/// `Csp::default().inline_script(brocanth_admin::INLINE_SCRIPT)`.
pub const INLINE_SCRIPT: &str = assets::SCRIPT;

/// The shipped stylesheet, which the layout inlines in a `<style>`.
///
/// Public for an ejected layout, which inlines it the same way. Its
/// content changes with every release; the custom properties listed in
/// `docs/admin-customization.md` are the stable part.
pub const STYLESHEET: &str = assets::STYLE;

/// dp-swap's mutating verbs, the layout's second script.
///
/// Re-exported from `brocanth_middleware::dp::SCRIPT` so an ejected layout
/// needs no dependency of its own on the middleware crate.
pub const MUTATE_SCRIPT: &str = brocanth_middleware::dp::SCRIPT;

/// The class strings the shipped screens are written in.
///
/// Public so an ejected screen compiles in the application with the same
/// vocabulary. **Stable:** the function names. **Not stable:** the class
/// strings they return, which follow the stylesheet.
pub mod parts {
    pub use crate::templates::parts::*;
}
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
