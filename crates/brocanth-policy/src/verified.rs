//! The witness, and the two obligations that make it worth having.
//!
//! PLAN.md §6 shipped a witness and §9 says why a witness alone is not enough.
//! §9 then records two things a witness must do that a marker type does not:
//!
//! > 2. **`Verified` must carry its condition**, not merely mark a type.
//! >    Check-then-write has a TOCTOU window; the write must be `UPDATE … WHERE
//! >    id = ? AND <condition>`, and zero rows affected is an authorization
//! >    failure rather than a no-op.
//! >
//! > 3. **`Verified` must be consumed, not produced.** `#[must_use]` is not
//! >    enough — you can construct one and drop it. The write API takes it.
//!
//! Both are mechanical here.
//!
//! # The path, end to end
//!
//! ```text
//! Pred            the policy's answer for this subject
//!   .admission()  SELECT 1 FROM t WHERE id = ? AND (<cond>) LIMIT 1
//!   .resolve(n)   the caller runs that query and reports the row count
//!   -> Verified   holds the id AND the condition
//!   .update(..)   UPDATE t SET .. WHERE id = ? AND (<cond>)   <- same condition
//!   .applied(n)   0 rows is Denied::RaceLost, not Ok(())
//! ```
//!
//! # Why `Verified` cannot be constructed outside this path
//!
//! It has no public constructor, no public field, and no `From`. The only
//! function in the crate that returns one is [`Admission::resolve`], and the
//! only way to hold an [`Admission`] is to have compiled a predicate against a
//! primary key. `tests/verified.rs` asserts the shape; the type system asserts
//! the rest.
//!
//! This crate opens no connection, so "checked" means the caller ran
//! [`Admission::query`] and passed the row count back. That is the seam a
//! crate with no driver can have, and it is drawn so the wrong call is
//! awkward: `resolve` takes the count, not a boolean, so there is no
//! `resolve(true)` to write without having counted something.
//!
//! # Why `Verified` is not `Clone`
//!
//! A witness is a proof about one row at one moment. Copying it would let the
//! same proof authorise two writes, which is the TOCTOU window reopening under
//! a different name.
//!
//! # Dropping one
//!
//! `#[must_use]` catches an ignored return value and nothing else — `let _ =
//! admission.resolve(1)?;` silences it. So [`Verified`] also carries its
//! payload in an `Option` that every consuming method takes, and its `Drop`
//! fires a `debug_assert!` when the payload is still there. In a debug build,
//! a witness you checked and then forgot to use fails the test that forgot it.
//! In release it costs a branch on a bool. It is not a linear type, because
//! Rust has none; it is the closest thing that does not need `unsafe`, and
//! this crate forbids `unsafe`.

use std::marker::PhantomData;

use crate::entity::{Column, Entity};
use crate::errors::{CompileError, Denied};
use crate::pred::Pred;
use crate::sql::{write_admission, write_delete, write_update, Condition, Dialect, Query};
use crate::value::Value;

impl<C: Column> Pred<C> {
    /// The one-row admission test, for a row the caller is about to touch.
    ///
    /// The default interpretation of PLAN.md §9 obligation 1, and the same
    /// statement PocketBase's `CanAccessRecord` runs.
    pub fn admission<E: Entity<Column = C>>(
        &self,
        dialect: Dialect,
        id: impl Into<Value>,
    ) -> Result<Admission<E>, CompileError> {
        let id = id.into();
        let condition = self.condition::<E>(dialect)?;
        let query = write_admission::<E, C>(dialect, &id, &condition)?;
        Ok(Admission {
            id,
            condition,
            query,
            entity: PhantomData,
        })
    }
}

/// A question asked, before it has an answer.
///
/// Hold the query, run it, hand back how many rows it returned.
pub struct Admission<E: Entity> {
    id: Value,
    condition: Condition,
    query: Query,
    entity: PhantomData<fn() -> E>,
}

impl<E: Entity> Admission<E> {
    /// The statement to run. One row or none.
    pub fn query(&self) -> &Query {
        &self.query
    }

    /// The primary key being asked about.
    pub fn id(&self) -> &Value {
        &self.id
    }

    /// The condition, which the witness will carry into the write.
    pub fn condition(&self) -> &Condition {
        &self.condition
    }

    /// Turn the row count [`Admission::query`] returned into a decision.
    ///
    /// Zero rows is [`Denied::NotAuthorised`], and this crate does not say
    /// whether the row was missing or forbidden: the distinction is itself a
    /// disclosure.
    pub fn resolve(self, rows: usize) -> Result<Verified<E>, Denied> {
        if rows == 0 {
            return Err(Denied::NotAuthorised);
        }
        Ok(Verified {
            inner: Some(VerifiedInner {
                id: self.id,
                condition: self.condition,
                entity: PhantomData,
            }),
        })
    }
}

struct VerifiedInner<E: Entity> {
    id: Value,
    condition: Condition,
    entity: PhantomData<fn() -> E>,
}

/// Proof that one row satisfied one condition, carrying the condition.
///
/// Not `Clone`, not constructible from outside, and consumed by every method
/// that does anything with it. See the module documentation.
///
/// There is no public constructor, so the struct literal does not compile:
///
/// ```compile_fail
/// # use brocanth_policy::{Entity, Ident, Verified};
/// # #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)] struct Col;
/// # impl brocanth_policy::Column for Col {
/// #     const ALL: &'static [Self] = &[Col];
/// #     fn name(&self) -> &'static str { "id" }
/// # }
/// # struct Post;
/// # impl Entity for Post {
/// #     type Column = Col;
/// #     const TABLE: Ident = Ident::must("posts");
/// #     const PRIMARY_KEY: Self::Column = Col;
/// # }
/// let _: Verified<Post> = Verified { inner: None };
/// ```
///
/// And it is not `Clone`, because one proof must not authorise two writes:
///
/// ```compile_fail
/// # use brocanth_policy::{Dialect, Entity, Ident, Pred, Verified};
/// # #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)] struct Col;
/// # impl brocanth_policy::Column for Col {
/// #     const ALL: &'static [Self] = &[Col];
/// #     fn name(&self) -> &'static str { "id" }
/// # }
/// # struct Post;
/// # impl Entity for Post {
/// #     type Column = Col;
/// #     const TABLE: Ident = Ident::must("posts");
/// #     const PRIMARY_KEY: Self::Column = Col;
/// # }
/// let verified: Verified<Post> = Pred::eq(Col, 1i64)
///     .admission::<Post>(Dialect::Sqlite, 1i64)
///     .unwrap()
///     .resolve(1)
///     .unwrap();
/// let _second = verified.clone();
/// ```
#[must_use = "a Verified that is not consumed is an authorisation check that changed nothing"]
pub struct Verified<E: Entity> {
    inner: Option<VerifiedInner<E>>,
}

impl<E: Entity> Verified<E> {
    /// The primary key this witness is about.
    pub fn id(&self) -> &Value {
        &self.expect_live().id
    }

    /// The condition this witness carries, and will re-apply.
    pub fn condition(&self) -> &Condition {
        &self.expect_live().condition
    }

    /// `UPDATE <table> SET … WHERE <pk> = ? AND (<condition>)`.
    ///
    /// The condition is the one the admission test passed, re-applied inside
    /// the write. That is PLAN.md §9 obligation 2: between the check and the
    /// write another transaction may have changed the row out from under the
    /// policy, and the only statement that cannot lose that race is one that
    /// re-asks the question while it writes.
    ///
    /// Takes `self`. The witness is gone afterwards and a second write needs a
    /// second check.
    pub fn update(
        mut self,
        dialect: Dialect,
        assignments: impl IntoIterator<Item = (E::Column, Value)>,
    ) -> Result<Write<E>, CompileError> {
        let inner = self.take();
        let assignments: Vec<(E::Column, Value)> = assignments.into_iter().collect();
        let query = write_update::<E, E::Column>(
            dialect,
            &inner.id,
            &assignments,
            &recompiled(&inner.condition, dialect)?,
        )?;
        Ok(Write {
            query,
            entity: PhantomData,
        })
    }

    /// `DELETE FROM <table> WHERE <pk> = ? AND (<condition>)`, with the same
    /// re-application and the same consumption.
    pub fn delete(mut self, dialect: Dialect) -> Result<Write<E>, CompileError> {
        let inner = self.take();
        let query = write_delete::<E, E::Column>(
            dialect,
            &inner.id,
            &recompiled(&inner.condition, dialect)?,
        )?;
        Ok(Write {
            query,
            entity: PhantomData,
        })
    }

    /// Give up the witness without writing, for a caller whose work is not a
    /// statement this crate can build — a stored procedure, a file, a queue.
    ///
    /// The returned condition is the one that must still guard whatever they
    /// do instead. Named so it reads as a decision rather than an accident:
    /// dropping a witness is a bug, releasing one is a choice.
    pub fn release(mut self) -> (Value, Condition) {
        let inner = self.take();
        (inner.id, inner.condition)
    }

    fn expect_live(&self) -> &VerifiedInner<E> {
        self.inner
            .as_ref()
            .expect("a Verified is only empty between `take` and `forget`")
    }

    /// Take the payload, which disarms the drop check: the husk this leaves
    /// behind drops quietly, and no `unsafe` or `ManuallyDrop` is involved.
    fn take(&mut self) -> VerifiedInner<E> {
        self.inner
            .take()
            .expect("a Verified is consumed exactly once; every consuming method takes self")
    }
}

impl<E: Entity> Drop for Verified<E> {
    fn drop(&mut self) {
        if self.inner.is_some() && !std::thread::panicking() {
            debug_assert!(
                false,
                "a Verified<{}> was dropped without being used: the authorisation \
                 check ran and nothing applied its condition. Call `update`, \
                 `delete`, or `release` if the write is built elsewhere.",
                std::any::type_name::<E>()
            );
        }
    }
}

/// A statement that carries a policy condition in its `WHERE` clause, and the
/// obligation to check what it changed.
#[must_use = "a Write is a statement to run and a row count to check"]
pub struct Write<E: Entity> {
    query: Query,
    entity: PhantomData<fn() -> E>,
}

impl<E: Entity> Write<E> {
    /// The statement.
    pub fn query(&self) -> &Query {
        &self.query
    }

    /// The statement, consumed.
    pub fn into_query(self) -> Query {
        self.query
    }

    /// Turn the affected row count into a decision.
    ///
    /// **Zero is [`Denied::RaceLost`], not `Ok`.** PLAN.md §9 obligation 2
    /// again: the statement re-applies the condition, so a write that changed
    /// nothing means the row stopped matching between the check and the write.
    /// An ORM reports that as `rows_affected: 0` and almost every application
    /// treats it as "nothing to do". Here it is the TOCTOU window closing, and
    /// it is an error.
    pub fn applied(self, rows_affected: u64) -> Result<u64, Denied> {
        if rows_affected == 0 {
            return Err(Denied::RaceLost);
        }
        Ok(rows_affected)
    }
}

// `Clone` and `Debug` by hand rather than by derive. A derive would require
// `E: Clone` and `E: Debug` on the entity, and an entity is a marker type
// nobody constructs — the `PhantomData<fn() -> E>` holds no `E` at all. A
// derive would make every entity carry derives it has no use for.

impl<E: Entity> Clone for Admission<E> {
    fn clone(&self) -> Self {
        Admission {
            id: self.id.clone(),
            condition: self.condition.clone(),
            query: self.query.clone(),
            entity: PhantomData,
        }
    }
}

impl<E: Entity> std::fmt::Debug for Admission<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Admission")
            .field("entity", &std::any::type_name::<E>())
            .field("id", &self.id)
            .field("query", &self.query)
            .finish()
    }
}

impl<E: Entity> std::fmt::Debug for Verified<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut out = f.debug_struct("Verified");
        out.field("entity", &std::any::type_name::<E>());
        match &self.inner {
            Some(inner) => out
                .field("id", &inner.id)
                .field("condition", &inner.condition)
                .finish(),
            None => out.field("consumed", &true).finish(),
        }
    }
}

impl<E: Entity> Clone for Write<E> {
    fn clone(&self) -> Self {
        Write {
            query: self.query.clone(),
            entity: PhantomData,
        }
    }
}

impl<E: Entity> std::fmt::Debug for Write<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Write")
            .field("entity", &std::any::type_name::<E>())
            .field("query", &self.query)
            .finish()
    }
}

/// A condition compiled for one dialect, re-targeted at another.
///
/// Mixing dialects is a mistake worth catching rather than a conversion worth
/// doing, so this refuses instead: a witness checked against SQLite cannot
/// quietly write PostgreSQL.
fn recompiled(condition: &Condition, dialect: Dialect) -> Result<Condition, CompileError> {
    if condition.dialect() == dialect {
        return Ok(condition.clone());
    }
    Err(CompileError::DialectMismatch {
        checked: condition.dialect(),
        writing: dialect,
    })
}
