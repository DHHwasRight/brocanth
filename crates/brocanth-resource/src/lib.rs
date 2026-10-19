//! Typed CRUD routes, from an entity and a policy.
//!
//! PLAN.md §12 item 5. The crate answers the records API of
//! `docs/pocketbase-surface.md` §2 with Rust types, and it answers §3's rules
//! with `brocanth-policy`.
//!
//! # The assertion this crate exists to satisfy
//!
//! PLAN.md §12:
//!
//! > **And one more assertion in the same spirit as the dependency test: a
//! > build of `brocanth-resource` with no policy registered must not
//! > compile.** Authorization claimed in a README rots. Authorization asserted
//! > by the type checker does not.
//!
//! [`Resource`] has an associated type `Policy`, and that type has no default.
//! A resource is the only way into every route this crate builds, so a
//! `Resource` impl without a policy is `error[E0046]: not all trait items
//! implemented, missing: `Policy``, and a `Resource` impl that names a type
//! which is not a [`Policy`](brocanth_policy::Policy) is an unsatisfied trait
//! bound. `tests/ui/` holds both cases and `tests/trybuild.rs` runs them
//! against the compiler's own output.
//!
//! The weakest policy a person can write still has to say who may list the
//! rows, because [`Policy::list`](brocanth_policy::Policy::list) is the one
//! method with no default. Every other method defaults to
//! [`Pred::Never`](brocanth_policy::Pred::Never) and
//! [`FieldMask::deny_all`](brocanth_policy::FieldMask::deny_all).
//!
//! # The central argument, and where it lives in the code
//!
//! PLAN.md §9:
//!
//! > **List authorization is not a per-object property. It is a set
//! > comprehension.** Any design that ignores this ships a paginator that
//! > silently lies.
//!
//! So this crate never filters rows it has read. [`ListQuery`] carries a
//! [`Condition`](brocanth_policy::Condition) that the data source has to put
//! in its `WHERE` clause, there is no constructor for a [`ListQuery`] without
//! one, and the same condition goes to [`ResourceData::count`]. The page and
//! the total are therefore windows over one set, so page two cannot overlap
//! page one and the total cannot disagree with the pages.
//!
//! The write paths go through [`Verified`](brocanth_policy::Verified), which
//! carries its condition into `UPDATE … WHERE id = ? AND <condition>`. Zero
//! rows affected is [`Denied::RaceLost`](brocanth_policy::Denied::RaceLost)
//! rather than a silent no-op; this crate reports it as `409`.
//!
//! # Two layers, and the lower one has no HTTP in it
//!
//! * [`flow`] is five functions over [`ResourceData`]. They take a subject,
//!   return an outcome or a [`Failure`], and know nothing about requests,
//!   responses or status codes. Every authorisation test in `tests/` drives
//!   these.
//! * [`routes`] turns those outcomes into responses and is where the status
//!   table lives. It needs the core, so it is behind the default `plugin`
//!   feature.
//!
//! The split is not tidiness. The status codes of
//! `docs/pocketbase-surface.md` §3 are information-hiding decisions, and
//! keeping them in one small module with [`status`] beside it is what lets a
//! reader check them all at once.
//!
//! # Synchronous, and why
//!
//! `brocanth`'s `Handler` is a synchronous `Fn`. [`ResourceData`] is therefore
//! synchronous as well: an async trait would oblige this crate to block the
//! calling thread on the application's behalf, inside a handler, with no way
//! to know whether the application has `block_in_place`, a blocking pool or a
//! synchronous driver. The application knows. It implements a synchronous
//! trait and decides for itself where the blocking happens — [`rt::block_on`]
//! is here for the common case of an async pool behind a synchronous impl.
//! `brocanth-admin` made the opposite choice and pays for it with
//! `async_trait` plus its own executor; the difference is that the admin holds
//! one `Arc<dyn AdminData>` and cannot be generic, where this crate is generic
//! over [`Resource::Data`] already.
//!
//! The core has since grown `AsyncHandler` beside `Handler`, and this crate
//! still registers the synchronous one. Nothing here has to change for that,
//! and the upgrade, when an application wants it, is additive: an
//! `AsyncResourceData` with the same six operations, a second set of handlers
//! registered through `RouteBundle::described_async`, and [`flow`] shared
//! between them unchanged, because [`flow`] is where the authorisation is and
//! none of it waits for anything.
//!
//! # Out of scope, by PLAN.md
//!
//! Dynamic collections (§12 declines them outright), a string filter language
//! (§9 is the argument against it), batch, `expand`, `fields`, and file
//! upload — `brocanth-storage` owns blobs, and a resource that references one
//! stores whatever identifier that crate hands it, as an ordinary column.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod data;
pub mod error;
pub mod flow;
pub mod json;
pub mod page;
pub mod resource;
pub mod rt;
pub mod sql;
pub mod status;

#[cfg(feature = "plugin")]
pub mod routes;

/// Why this crate is listed on the `brocanth create` menu and not available.
///
/// Behind an off-by-default feature. The one program that reads it is the
/// CLI.
#[cfg(feature = "scaffold")]
pub mod scaffold;

pub use data::{DataError, FetchQuery, ListQuery, ResourceData};
pub use error::Failure;
pub use page::{Listing, PageRequest, Totals};
pub use resource::{ColumnOf, CreateCheck, Direction, Resource, RowOf, Sort};
pub use sql::Statement;
pub use status::{Action, Outcome};

#[cfg(feature = "plugin")]
pub use routes::{ResourceRoutes, SubjectSource};

// Re-exported so that an application, and the compile-fail cases under
// `tests/ui`, name one crate rather than three. A resource is written against
// all of these by definition.
pub use brocanth_events as events;
pub use brocanth_policy as policy;
