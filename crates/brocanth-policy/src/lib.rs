//! Authorisation as a predicate and a witness, not one or the other.
//!
//! PLAN.md §9, in its own words:
//!
//! > A rule is a predicate; a witness is a proof that a predicate was
//! > evaluated. PocketBase ships only the first, §6 shipped only the second.
//! > `brocanth-policy` ships both: a typed predicate over the entity's column
//! > enum, with two interpretations — `condition()` into SQL for lists, and
//! > `eval()` for a row you hold.
//!
//! # The bug this exists to delete
//!
//! PocketBase validates a rule against an empty request object. An
//! unresolvable `@request.*` path logs a debug line and becomes `NULL`, and
//! its `!=` compiles to `IS NOT` over `COALESCE`:
//!
//! ```text
//! @request.auth.rolee != "banned"              <- a typo
//! -> COALESCE(NULL,'') IS NOT COALESCE('banned','')
//! -> '' IS NOT 'banned'
//! -> TRUE                                       <- access granted
//! ```
//!
//! **A misspelled field in a negated rule grants access, silently, forever.**
//! It is structural rather than a bug. Two things here make it impossible:
//!
//! 1. **The field name is not a string.** A comparison takes a [`Column`],
//!    which is the entity's own enum, so `PostColumn::Rolee` does not compile.
//!    There is no string-to-filter parser in this crate and there is not one
//!    anywhere else in the workspace either; §9 is the argument for why there
//!    should not be.
//! 2. **An absent value is a decision the author writes down.** An `Option`
//!    does not convert into a [`Bound`]. The author writes
//!    [`Bound::or_deny`], [`Bound::or_allow`] or [`Bound::or_null`], and the
//!    word is in the diff. See [`value`].
//!
//! And where the grant still happens because the author asked for it,
//! [`Pred::is_always`] says so and a test can assert it.
//!
//! # Two interpretations, and the one that is the default
//!
//! | | what it is | when |
//! |---|---|---|
//! | [`Pred::condition`] | a `WHERE` fragment and an ordered bind list | always |
//! | [`Pred::admission`] | `SELECT 1 … WHERE id = ? AND (<cond>) LIMIT 1` | before a write |
//! | [`Pred::eval`] | three-valued logic over a row you hold | opt-in |
//!
//! §9 obligation 1 says the two interpretations can disagree and PocketBase
//! cannot have that bug because it has only one. So `condition()` is the
//! default everywhere, [`Pred::admission`] is literally PocketBase's
//! `CanAccessRecord`, and [`Pred::eval`] is something a caller opts into
//! together with the obligation to prove the two agree. [`agreement`] is the
//! harness for that proof, and its module documentation says exactly how far
//! tier one got with it.
//!
//! # Three-valued logic
//!
//! [`Truth`] has three values because SQL does. `NOT Unknown` is `Unknown`,
//! and the collapse to a yes or a no happens once, at the top, in
//! [`Truth::admits`], towards denial — which is what a `WHERE` clause does and
//! therefore the only collapse under which the two interpretations can agree.
//! See [`eval`].
//!
//! # The witness
//!
//! [`Verified`] carries the condition it was checked against, so the write is
//! `UPDATE … WHERE id = ? AND <condition>` and the TOCTOU window closes;
//! [`Write::applied`] turns zero affected rows into [`Denied::RaceLost`]
//! rather than into a no-op; and every method that does anything with a
//! [`Verified`] takes it by value, because `#[must_use]` is not enough. See
//! [`verified`].
//!
//! # No ORM, no driver, no core
//!
//! Compiled for [`Dialect::Postgres`] by default, because that is where
//! brocanth keeps its queues, cache and blobs; [`Dialect::Sqlite`] is the
//! smaller-setup target and is compiled with the same meaning rather than the
//! same text. The dialect is an argument at every entry point so a call site
//! says which one it meant.
//!
//! This crate emits SQL text and a `Vec<Value>`. It does not depend on
//! SeaORM, on `sqlx`, on a driver, or on `brocanth` itself — PLAN.md §8 says
//! the arrow points from plugin to core, always, and the core has three
//! dependencies and a test that fails the build when a fourth kind arrives.
//! The adapter that turns a [`Condition`] into a `sea_orm::Condition` is a
//! later, separate crate. [`sql`] carries the argument and the dialect table.
//!
//! # Realtime
//!
//! Every variant of [`Pred`] is public, because PLAN.md §10 says *"a predicate
//! you can inspect can be inverted into an audience; a string you can only
//! execute"*. [`Pred::audience`] does the inversion for the shapes §10 names,
//! and says in words why it could not for the rest. See [`audience`].
//!
//! # Not here, on purpose
//!
//! The `policy!` macro. PLAN.md §12 puts it at item 10, *"deliberately late,
//! after a real application has used the algebra, so the syntax is informed
//! rather than imitated"*. This is item 2. Policies are plain Rust values, and
//! what they teach us is the input to that macro.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod agreement;
pub mod audience;
pub mod entity;
pub mod errors;
pub mod eval;
pub mod mask;
pub mod policy;
pub mod pred;
pub mod sql;
pub mod value;

pub use audience::{Audience, NotInvertible};
pub use entity::{Column, Entity, Ident};
pub use errors::{CompileError, Denied, EvalError};
pub use eval::{MapRow, RowView, Truth};
pub use mask::{FieldMask, MaskViolation};
pub use policy::Policy;
pub use pred::{EntityPred, Membership, Op, Opaque, Pred, Shape};
pub use sql::{Condition, Dialect, Query};
pub use value::{Bound, Missing, Value};
pub use verified::{Admission, Verified, Write};

mod verified;
