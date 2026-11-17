//! The seam between this crate and whatever stores the rows.
//!
//! `brocanth-admin` set the pattern and the reason is PLAN.md §8.4: *"the ORM
//! is recommended and absent"*. So this crate declares the smallest trait that
//! can serve five routes and takes no dependency on SeaORM, on `sqlx`, on a
//! driver or on a pool. `tests/common/mod.rs` implements it over a real
//! Postgres database, because the claim under test is that the policy filters
//! in the query.
//!
//! # A session per operation, and six statements
//!
//! [`ResourceData::session`] opens a [`Session`] for one operation, as the
//! request's identity. The session runs the statements, and
//! [`Session::commit`] ends it:
//!
//! | | what it runs |
//! |---|---|
//! | [`Session::count`] | `SELECT count(*) … WHERE <condition>` |
//! | [`Session::list`] | `SELECT … WHERE <condition> ORDER BY … LIMIT … OFFSET …` |
//! | [`Session::fetch`] | `SELECT … WHERE <pk> = ? AND <condition>` |
//! | [`Session::admit`] | a statement this crate wrote; report the row count |
//! | [`Session::write`] | a statement this crate wrote; report rows affected |
//! | [`Session::insert`] | the one operation whose SQL this crate cannot write |
//!
//! # Why a session
//!
//! docs/policy-rls-bridge.md, decision 13. On a table under Postgres
//! row-level security, a statement outside the request's identity sees no
//! row. So each operation runs in one transaction that assumed the
//! identity: the admission read and the `Verified` write it admits are in
//! the same transaction, and the database adds its policy to the
//! application's `UPDATE … WHERE id = ? AND <condition>`.
//!
//! The identity is [`ResourceData::Identity`], a type the source names:
//! this crate links no driver, so it cannot name `brocanth_db::DbIdentity`.
//! A source with no identity takes `()` and returns itself as the session,
//! so it runs as before.
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
//! wrote the `INSERT` would be an ORM. So [`Session::insert`] takes the
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
///
/// The source itself runs no statement. It opens a [`Session`] for each
/// operation, as the request's [`ResourceData::Identity`], and the session
/// runs every statement of that operation. So the admission read and the
/// write it admits run in one unit of work: for a SQL source, one
/// transaction, as one database identity.
pub trait ResourceData<E: brocanth_policy::Entity>: Send + Sync + 'static {
    /// The application's row. This crate never looks inside one; it counts
    /// them, pages them, and asks the resource to encode them.
    type Row;

    /// Who a request's statements run as, in the database.
    ///
    /// This crate never looks inside one either. A source over a table
    /// under Postgres row-level security takes a `brocanth_db::DbIdentity`
    /// (docs/policy-rls-bridge.md, decision 13). A source with none takes
    /// `()`. The default value is what a call with no identity gets, so
    /// [`crate::flow::list`] and the other functions without `_as` run as
    /// it.
    type Identity: Default + Send + Sync + 'static;

    /// The statements of one operation, as one identity.
    type Session<'a>: Session<E, Row = Self::Row>
    where
        Self: 'a;

    /// Opens the session of one operation, as `identity`.
    ///
    /// A SQL source under row-level security begins a transaction and
    /// assumes the identity in it (`Db::transact_as`, `Tx::assume`). A
    /// source with no identity can return itself, by reference, and run
    /// each statement on its pool, as before.
    ///
    /// # Errors
    ///
    /// Whatever the storage layer raised: no connection, or an identity it
    /// refuses.
    fn session(&self, identity: &Self::Identity) -> Result<Self::Session<'_>, DataError>;
}

/// The statements of one operation, in one unit of work.
///
/// [`ResourceData::session`] opens it, the flow runs its statements, and
/// [`Session::commit`] ends it. A session dropped without `commit` keeps
/// nothing: the flow drops it when a check refuses, and a write that was
/// refused leaves no trace.
pub trait Session<E: brocanth_policy::Entity> {
    /// The row, as [`ResourceData::Row`].
    type Row;

    /// How many rows satisfy the condition.
    ///
    /// The same condition the matching [`Session::list`] call carries. A
    /// count over a different set is a total that does not match its own
    /// pages.
    ///
    /// # Errors
    ///
    /// Whatever the storage layer raised.
    fn count(&mut self, query: &ListQuery<'_, E::Column>) -> Result<u64, DataError>;

    /// One page of rows.
    ///
    /// # Errors
    ///
    /// Whatever the storage layer raised.
    fn list(&mut self, query: &ListQuery<'_, E::Column>) -> Result<Vec<Self::Row>, DataError>;

    /// One row, or `None` when the key matches nothing **or** the condition
    /// refuses it. The two are one case on purpose.
    ///
    /// # Errors
    ///
    /// Whatever the storage layer raised.
    fn fetch(&mut self, query: &FetchQuery<'_, E::Column>) -> Result<Option<Self::Row>, DataError>;

    /// Run an admission statement and report how many rows it returned.
    ///
    /// Zero or one. The count, rather than a boolean, because
    /// `brocanth_policy::Admission::resolve` takes a count — there is no
    /// `resolve(true)` to write without having counted something.
    ///
    /// # Errors
    ///
    /// Whatever the storage layer raised.
    fn admit(&mut self, statement: &Statement) -> Result<usize, DataError>;

    /// Run a write statement and report rows affected.
    ///
    /// **Do not treat zero as success.** The statement carries the policy's
    /// condition in its `WHERE` clause, and this crate turns zero into
    /// `Denied::RaceLost`. Under row-level security the database adds its
    /// own policy to the same statement: two checks, one statement.
    ///
    /// # Errors
    ///
    /// Whatever the storage layer raised.
    fn write(&mut self, statement: &Statement) -> Result<u64, DataError>;

    /// Insert a row the policy has already admitted, and return its key and
    /// the row as stored.
    ///
    /// The values are exactly what the request set, already checked against
    /// [`Policy::writable_on_create`](brocanth_policy::Policy::writable_on_create). Defaults,
    /// generated keys and timestamps are the model layer's business.
    ///
    /// # Errors
    ///
    /// [`DataError::Invalid`] when the model layer rejects the record, which
    /// reaches the client as a `400` with the message. Anything else is a
    /// `500`.
    fn insert(&mut self, values: &[(E::Column, Value)]) -> Result<(Value, Self::Row), DataError>;

    /// Keep what the session wrote: `COMMIT`. A session with no transaction
    /// returns `Ok(())`.
    ///
    /// # Errors
    ///
    /// Whatever the storage layer raised. The flow reports it as a `500`,
    /// and nothing was kept.
    fn commit(self) -> Result<(), DataError>;
}
