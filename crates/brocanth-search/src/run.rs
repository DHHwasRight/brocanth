//! Run a search over the pool the `db` module opens. Behind `postgres`.

use brocanth_db::sqlx::postgres::{PgArguments, PgRow};
use brocanth_db::sqlx::query::{QueryAs, QueryScalar};
use brocanth_db::sqlx::{self, AssertSqlSafe, FromRow, Postgres};
use brocanth_db::{Db, DbError};
use brocanth_pagination::Page;
use brocanth_policy::{Condition, Entity, Value};

use crate::index::Index;
use crate::SearchError;

/// One page of results: the rows, best first, and where the page sits.
#[derive(Clone, Debug, PartialEq)]
pub struct Hits<R> {
    /// The rows, in rank order.
    pub items: Vec<R>,
    /// The page, with the total of the same `WHERE`.
    pub page: Page,
}

impl<E: Entity> Index<E> {
    /// Searches for `term` inside `condition`, and reads page `number`.
    ///
    /// Counts first, then reads the page from the clamped [`Page`], so a page
    /// past the end is the last page. An empty term finds nothing and reads
    /// nothing.
    ///
    /// `R` reads the columns of `E` and `search_rank` (a `real`, `f32`).
    /// [`Hit`](crate::documents::Hit) does, for the site index.
    ///
    /// # Errors
    ///
    /// What [`Index::query`] refuses, and what the database raised.
    pub async fn search<R>(
        &self,
        db: &Db,
        condition: &Condition,
        term: &str,
        number: u64,
        per_page: u64,
    ) -> Result<Hits<R>, SearchError>
    where
        R: for<'r> FromRow<'r, PgRow> + Send + Unpin,
    {
        let term = term.trim();
        if term.is_empty() {
            return Ok(Hits {
                items: Vec::new(),
                page: Page::new(1, per_page, 0),
            });
        }
        let probe = self.query(condition, term, 0, per_page)?;
        let mut count: QueryScalar<'_, Postgres, i64, PgArguments> =
            sqlx::query_scalar(AssertSqlSafe(probe.count.sql.clone()));
        for value in &probe.count.binds {
            count = bind_scalar(count, value);
        }
        let total = count.fetch_one(db.pool()).await.map_err(DbError::from)?;
        let page = Page::new(number, per_page, u64::try_from(total).unwrap_or(0));
        if page.is_empty() {
            return Ok(Hits {
                items: Vec::new(),
                page,
            });
        }
        let statement = self
            .query(condition, term, page.offset(), page.limit())?
            .select;
        let mut select: QueryAs<'_, Postgres, R, PgArguments> =
            sqlx::query_as(AssertSqlSafe(statement.sql));
        for value in &statement.binds {
            select = bind_as(select, value);
        }
        let items = select.fetch_all(db.pool()).await.map_err(DbError::from)?;
        Ok(Hits { items, page })
    }
}

macro_rules! bind_value {
    ($query:expr, $value:expr) => {
        match $value {
            Value::Null => $query.bind(None::<String>),
            Value::Bool(b) => $query.bind(*b),
            Value::Int(n) => $query.bind(*n),
            Value::Float(f) => $query.bind(*f),
            Value::Text(t) => $query.bind(t.clone()),
            Value::Bytes(b) => $query.bind(b.clone()),
        }
    };
}

fn bind_scalar<'q>(
    query: QueryScalar<'q, Postgres, i64, PgArguments>,
    value: &Value,
) -> QueryScalar<'q, Postgres, i64, PgArguments> {
    bind_value!(query, value)
}

fn bind_as<'q, R>(
    query: QueryAs<'q, Postgres, R, PgArguments>,
    value: &Value,
) -> QueryAs<'q, Postgres, R, PgArguments> {
    bind_value!(query, value)
}
