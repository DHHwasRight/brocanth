//! [`Exec`]: one set of query methods for a pool and a transaction.
//!
//! # Why `&mut self`
//!
//! A transaction is one connection, and sqlx runs a statement on a
//! connection through `&mut PgConnection`. A pool needs only `&PgPool`. A
//! trait that both implement must take the stricter of the two, so every
//! method takes `&mut self`. A pool's implementation ignores the `mut`.
//!
//! The payoff is one helper for both:
//!
//! ```no_run
//! use brocanth_db::{Db, DbError, Exec};
//! use brocanth_db::sea_query::{Expr, Query};
//!
//! async fn count_posts(db: &mut impl Exec) -> Result<i64, DbError> {
//!     let q = Query::select().expr(Expr::cust("count(*)")).from("posts").to_owned();
//!     let (n,): (i64,) = db.one(&q).await?;
//!     Ok(n)
//! }
//!
//! # async fn example(db: Db) -> Result<(), DbError> {
//! let mut db = db; // a `Db` is a cheap clone; `&mut db` costs nothing
//! count_posts(&mut db).await?;
//! db.transact(async |tx| count_posts(tx).await).await?;
//! # Ok(())
//! # }
//! ```
//!
//! `&mut Db` and `&Db` implement `Exec` as well, so a helper that takes
//! `impl Exec` by value accepts `&mut tx`, `&db` and a clone of `db`.
//!
//! # Every method builds before it awaits
//!
//! A method turns the statement into SQL text and values at the call, then
//! returns a future that owns them. So the future does not borrow the
//! statement, and it is `Send` whenever the executor is.

use std::future::Future;

use sea_query::{
    DeleteStatement, InsertStatement, PostgresQueryBuilder, QueryStatementWriter, UpdateStatement,
    Values,
};
use sea_query_sqlx::SqlxValues;
use sqlx::postgres::{PgExecutor, PgRow};
use sqlx::{AssertSqlSafe, FromRow};

use crate::error::DbError;

/// Something a statement runs on: a [`Db`](crate::Db) (a pooled connection
/// per statement) or a [`Tx`](crate::Tx) (one connection, one transaction).
pub trait Exec: Send {
    /// The sqlx executor underneath, for a statement this trait does not
    /// cover: `sqlx::query("…").execute(db.executor())`.
    fn executor(&mut self) -> impl PgExecutor<'_>;

    /// Every row, each decoded as `T`: a tuple, or a struct with
    /// `#[derive(sqlx::FromRow)]`.
    ///
    /// Takes any statement, so an `INSERT … RETURNING` works too.
    fn all<T>(
        &mut self,
        q: &impl QueryStatementWriter,
    ) -> impl Future<Output = Result<Vec<T>, DbError>> + Send
    where
        T: for<'r> FromRow<'r, PgRow> + Send + Unpin,
    {
        let (sql, values) = q.build(PostgresQueryBuilder);
        async move {
            sqlx::query_as_with::<_, T, _>(AssertSqlSafe(sql), SqlxValues(values))
                .fetch_all(self.executor())
                .await
                .map_err(DbError::query)
        }
    }

    /// The first row. No row is [`DbError::NotFound`].
    fn one<T>(
        &mut self,
        q: &impl QueryStatementWriter,
    ) -> impl Future<Output = Result<T, DbError>> + Send
    where
        T: for<'r> FromRow<'r, PgRow> + Send + Unpin,
    {
        let (sql, values) = q.build(PostgresQueryBuilder);
        async move {
            sqlx::query_as_with::<_, T, _>(AssertSqlSafe(sql), SqlxValues(values))
                .fetch_one(self.executor())
                .await
                .map_err(DbError::query)
        }
    }

    /// The first row, or `None`.
    fn optional<T>(
        &mut self,
        q: &impl QueryStatementWriter,
    ) -> impl Future<Output = Result<Option<T>, DbError>> + Send
    where
        T: for<'r> FromRow<'r, PgRow> + Send + Unpin,
    {
        let (sql, values) = q.build(PostgresQueryBuilder);
        async move {
            sqlx::query_as_with::<_, T, _>(AssertSqlSafe(sql), SqlxValues(values))
                .fetch_optional(self.executor())
                .await
                .map_err(DbError::query)
        }
    }

    /// Runs an `INSERT`, `UPDATE` or `DELETE`, and answers the number of rows
    /// it touched.
    ///
    /// Only those three statement types compile here. A `SELECT` passed to
    /// `execute` would throw its rows away, which is never what a caller
    /// meant.
    fn execute(
        &mut self,
        q: &impl WriteStatement,
    ) -> impl Future<Output = Result<u64, DbError>> + Send {
        let (sql, values) = q.build(PostgresQueryBuilder);
        async move {
            sqlx::query_with(AssertSqlSafe(sql), SqlxValues(values))
                .execute(self.executor())
                .await
                .map(|done| done.rows_affected())
                .map_err(DbError::query)
        }
    }

    /// Every row as a JSON object, keyed by column name.
    ///
    /// Postgres does the encoding. The statement runs as
    /// `WITH q AS (<statement>) SELECT row_to_json(q)::text FROM q`, and each
    /// text parses into a [`serde_json::Value`]. So every Postgres type
    /// becomes the JSON that Postgres would write for it: `numeric` stays
    /// exact as a JSON number, `timestamptz` becomes an ISO 8601 string, a
    /// `jsonb` column nests as JSON. A CTE rather than a subquery, so an
    /// `INSERT … RETURNING` works too, and the order of the rows stays the
    /// order of the statement.
    ///
    /// The keys of each object come out in the column order of the
    /// statement only when `serde_json` has its `preserve_order` feature on.
    fn json(
        &mut self,
        q: &impl QueryStatementWriter,
    ) -> impl Future<Output = Result<Vec<serde_json::Value>, DbError>> + Send {
        let (sql, values) = q.build(PostgresQueryBuilder);
        self.raw_json(sql, values)
    }

    /// [`Exec::json`] for SQL text built elsewhere: a saved query, a
    /// report definition, a template. Placeholders are `$1`, `$2`, … and
    /// `values` binds them in order.
    ///
    /// The text runs as written, inside the CTE [`Exec::json`] describes.
    /// Never build it from user input.
    fn raw_json(
        &mut self,
        sql: impl Into<String>,
        values: Values,
    ) -> impl Future<Output = Result<Vec<serde_json::Value>, DbError>> + Send {
        let sql = wrap_json(&sql.into());
        async move {
            let rows: Vec<String> = sqlx::query_scalar_with(AssertSqlSafe(sql), SqlxValues(values))
                .fetch_all(self.executor())
                .await
                .map_err(DbError::query)?;
            rows.iter()
                .map(|text| serde_json::from_str(text).map_err(DbError::Json))
                .collect()
        }
    }
}

/// The SQL [`Exec::json`] runs. A trailing `;` would end the CTE early, so it
/// goes.
fn wrap_json(sql: &str) -> String {
    let inner = sql.trim().trim_end_matches(';');
    format!("WITH q AS ({inner}) SELECT row_to_json(q)::text FROM q")
}

/// A statement that writes: [`InsertStatement`], [`UpdateStatement`] or
/// [`DeleteStatement`]. Sealed.
pub trait WriteStatement: QueryStatementWriter + sealed::Sealed {}

impl WriteStatement for InsertStatement {}
impl WriteStatement for UpdateStatement {}
impl WriteStatement for DeleteStatement {}

mod sealed {
    pub trait Sealed {}
    impl Sealed for sea_query::InsertStatement {}
    impl Sealed for sea_query::UpdateStatement {}
    impl Sealed for sea_query::DeleteStatement {}
}

impl<E: Exec> Exec for &mut E {
    fn executor(&mut self) -> impl PgExecutor<'_> {
        (**self).executor()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_wraps_in_a_cte_and_drops_a_semicolon() {
        assert_eq!(
            wrap_json(" SELECT 1 AS a; "),
            "WITH q AS (SELECT 1 AS a) SELECT row_to_json(q)::text FROM q"
        );
    }
}
