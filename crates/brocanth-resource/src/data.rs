//! The seam between this crate and whatever stores the rows.
//!
//! `brocanth-admin` set the pattern and the reason is PLAN.md §8.4: *"the ORM
//! is recommended and absent"*. So this crate declares the smallest trait that
//! can serve five routes and takes no dependency on SeaORM, on `sqlx`, on a
//! driver or on a pool. `tests/common/mod.rs` implements it over a real SQLite database, because the
//! claim under test is that the policy filters in the query.
//!
//! # Six operations
//!
//! | | what it runs |
//! |---|---|
//! | [`ResourceData::count`] | `SELECT count(*) … WHERE <condition>` |
//! | [`ResourceData::list`] | `SELECT … WHERE <condition> ORDER BY … LIMIT … OFFSET …` |
//! | [`ResourceData::fetch`] | `SELECT … WHERE <pk> = ? AND <condition>` |
//! | [`ResourceData::admit`] | a statement this crate wrote; report the row count |
//! | [`ResourceData::write`] | a statement this crate wrote; report rows affected |
//! | [`ResourceData::insert`] | the one operation whose SQL this crate cannot write |
//!
//! The first three take a query object rather than loose arguments, because
//! the condition is the one thing an implementation must not forget and a
//! struct with no constructor that omits it is harder to forget than a
//! parameter. There is no `ListQuery` without a [`Condition`].
//!
//! # Why `insert` is the odd one
//!
//! Everything else is a statement this crate built and the implementation
//! executes. An insert is not: the column list, the defaults, the returning
//! clause and the generated key are the model layer's, and a framework that
//! wrote the `INSERT` would be an ORM. So [`ResourceData::insert`] takes the
//! values the policy has already admitted and hands back the key and the row.
//! The authorisation happened before it, in [`crate::sql::create_admission`].
//!
//! # Synchronous
//!
//! `brocanth`'s `Handler` is a synchronous `Fn`, so a handler must block
//! somewhere. This trait is synchronous so that the blocking is the
//! application's call rather than this crate's: an application with an async
//! pool writes `rt::block_on(self.pool.fetch_all(..))` inside its impl, or
//! `tokio::task::block_in_place`, or uses a synchronous driver and blocks
//! nowhere. [`crate::rt::block_on`] is here for the first case.

use brocanth_policy::{Column, Condition, Value};
use thiserror::Error;

use crate::resource::Sort;
use crate::sql::Statement;

/// What a data source can fail with.
///
/// Two kinds, and they answer differently: [`DataError::Invalid`] is the model
/// layer's verdict on the body and reaches the client as `400`, while
/// [`DataError::Backend`] is a `500` and reaches the application's error
/// hooks.
#[derive(Debug, Error)]
pub enum DataError {
    /// The record is not valid. The message reaches the client.
    #[error("{0}")]
    Invalid(String),
    /// Anything the storage layer raised.
    #[error("{0}")]
    Backend(String),
}

impl DataError {
    /// A backend failure from any message.
    pub fn backend(message: impl Into<String>) -> Self {
        DataError::Backend(message.into())
    }

    /// A validation failure from any message.
    pub fn invalid(message: impl Into<String>) -> Self {
        DataError::Invalid(message.into())
    }
}

/// One page of a listing, with the policy's condition in it.
///
/// Built only by [`crate::flow::list`], and only from a compiled policy, so
/// there is no way to ask a data source for a page that is not filtered. An
/// implementation that drops [`ListQuery::condition`] on the floor is a bug
/// this crate cannot catch; an implementation that never knew about it is one
/// it can.
#[derive(Clone, Debug)]
pub struct ListQuery<'a, C: Column> {
    condition: &'a Condition,
    offset: u64,
    limit: u64,
    sort: Sort<C>,
}

impl<'a, C: Column> ListQuery<'a, C> {
    pub(crate) fn new(condition: &'a Condition, offset: u64, limit: u64, sort: Sort<C>) -> Self {
        ListQuery {
            condition,
            offset,
            limit,
            sort,
        }
    }

    /// The policy's condition. `AND` it into the `WHERE` clause.
    ///
    /// PLAN.md §9: this is what makes the paginator honest. Filtering the rows
    /// after reading them gives a page two that overlaps page one and a total
    /// that is a lie.
    pub fn condition(&self) -> &'a Condition {
        self.condition
    }

    /// Rows to skip. Already clamped.
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// Rows to return. Already clamped to
    /// [`Resource::MAX_PER_PAGE`](crate::Resource::MAX_PER_PAGE).
    pub fn limit(&self) -> u64 {
        self.limit
    }

    /// The order. Never absent: a paginator with no `ORDER BY` returns
    /// whatever the engine felt like, which is a different kind of lying page.
    pub fn sort(&self) -> Sort<C> {
        self.sort
    }
}

/// One row by primary key, with the policy's condition in it.
///
/// PocketBase's by-id guard: `WHERE id = ? AND <rule>`, one statement, so
/// there is no window between deciding and reading.
#[derive(Clone, Debug)]
pub struct FetchQuery<'a, C: Column> {
    condition: &'a Condition,
    id: &'a Value,
    marker: std::marker::PhantomData<fn() -> C>,
}

impl<'a, C: Column> FetchQuery<'a, C> {
    pub(crate) fn new(condition: &'a Condition, id: &'a Value) -> Self {
        FetchQuery {
            condition,
            id,
            marker: std::marker::PhantomData,
        }
    }

    /// The policy's condition. `AND` it into the `WHERE` clause.
    pub fn condition(&self) -> &'a Condition {
        self.condition
    }

    /// The primary key.
    pub fn id(&self) -> &'a Value {
        self.id
    }
}

/// Everything this crate needs from the layer that stores rows.
///
/// `E` is the entity, so one implementation serves one table and the column
/// type is checked. See the module documentation for why it is synchronous.
pub trait ResourceData<E: brocanth_policy::Entity>: Send + Sync + 'static {
    /// The application's row. This crate never looks inside one; it counts
    /// them, pages them, and asks the resource to encode them.
    type Row;

    /// How many rows satisfy the condition.
    ///
    /// The same condition the matching [`ResourceData::list`] call carries. A
    /// count over a different set is a total that does not match its own
    /// pages.
    ///
    /// # Errors
    ///
    /// Whatever the storage layer raised.
    fn count(&self, query: &ListQuery<'_, E::Column>) -> Result<u64, DataError>;

    /// One page of rows.
    ///
    /// # Errors
    ///
    /// Whatever the storage layer raised.
    fn list(&self, query: &ListQuery<'_, E::Column>) -> Result<Vec<Self::Row>, DataError>;

    /// One row, or `None` when the key matches nothing **or** the condition
    /// refuses it. The two are one case on purpose.
    ///
    /// # Errors
    ///
    /// Whatever the storage layer raised.
    fn fetch(&self, query: &FetchQuery<'_, E::Column>) -> Result<Option<Self::Row>, DataError>;

    /// Run an admission statement and report how many rows it returned.
    ///
    /// Zero or one. The count, rather than a boolean, because
    /// `brocanth_policy::Admission::resolve` takes a count — there is no
    /// `resolve(true)` to write without having counted something.
    ///
    /// # Errors
    ///
    /// Whatever the storage layer raised.
    fn admit(&self, statement: &Statement) -> Result<usize, DataError>;

    /// Run a write statement and report rows affected.
    ///
    /// **Do not treat zero as success.** The statement carries the policy's
    /// condition in its `WHERE` clause, and this crate turns zero into
    /// `Denied::RaceLost`.
    ///
    /// # Errors
    ///
    /// Whatever the storage layer raised.
    fn write(&self, statement: &Statement) -> Result<u64, DataError>;

    /// Insert a row the policy has already admitted, and return its key and
    /// the row as stored.
    ///
    /// The values are exactly what the request set, already checked against
    /// [`Policy::writable`](brocanth_policy::Policy::writable). Defaults,
    /// generated keys and timestamps are the model layer's business.
    ///
    /// # Errors
    ///
    /// [`DataError::Invalid`] when the model layer rejects the record, which
    /// reaches the client as a `400` with the message. Anything else is a
    /// `500`.
    fn insert(&self, values: &[(E::Column, Value)]) -> Result<(Value, Self::Row), DataError>;
}
