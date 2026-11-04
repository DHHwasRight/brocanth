//! [`Db`], the pool handle, and [`Tx`], one transaction on one connection.

use sqlx::postgres::{PgExecutor, PgPool};
use sqlx::{Acquire, Postgres, Transaction};

use crate::config::DbConfig;
use crate::error::DbError;
use crate::exec::Exec;

/// A Postgres pool. Clone it freely: a clone is an `Arc` bump, and every
/// clone shares the same connections.
///
/// Each statement run through [`Exec`] on a `Db` takes a connection from the
/// pool and gives it back when the statement ends. A request never holds a
/// connection between statements. So a slow handler that awaits an HTTP call
/// between two queries holds no connection while it waits, and the pool
/// size bounds concurrent *statements*, not concurrent *requests*. A
/// transaction is the one exception, and it is explicit: [`Db::transact`] or
/// [`Db::begin`].
#[derive(Clone, Debug)]
pub struct Db {
    pool: PgPool,
}

impl Db {
    /// Opens the pool and one connection, so a wrong URL, password or
    /// certificate fails here and not on the first request.
    ///
    /// # Errors
    /// [`DbError::Config`] for a config this crate cannot use;
    /// [`DbError::Connect`] when the first connection failed.
    pub async fn connect(cfg: &DbConfig) -> Result<Db, DbError> {
        let options = cfg.connect_options()?;
        let pool = cfg
            .pool_options()?
            .connect_with(options)
            .await
            .map_err(DbError::connect)?;
        Ok(Db { pool })
    }

    /// Builds the pool and opens no connection. The first statement opens
    /// one. For a process that must start while the database is down.
    ///
    /// With `min_connections` above 0, this spawns a task that opens them,
    /// so it must run inside a Tokio runtime.
    ///
    /// # Errors
    /// [`DbError::Config`] for a config this crate cannot use.
    pub fn connect_lazy(cfg: &DbConfig) -> Result<Db, DbError> {
        let options = cfg.connect_options()?;
        Ok(Db {
            pool: cfg.pool_options()?.connect_lazy_with(options),
        })
    }

    /// Wraps a pool the application already opened.
    ///
    /// The session settings of [`DbConfig`] are not applied: they belong to
    /// the pool's connect options.
    pub fn from_pool(pool: PgPool) -> Db {
        Db { pool }
    }

    /// The sqlx pool underneath.
    ///
    /// `brocanth-model` wraps the same pool for SeaORM with
    /// `SqlxPostgresConnector::from_sqlx_postgres_pool(db.pool().clone())`,
    /// so models and sea-query statements share one set of connections.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Runs `f` in a transaction. `Ok` commits. `Err` rolls back and returns
    /// the error. A panic in `f` drops the transaction, and the drop rolls
    /// back.
    ///
    /// ```no_run
    /// # use brocanth_db::{Db, DbError, Exec};
    /// # use brocanth_db::sea_query::Query;
    /// # async fn example(db: Db) -> Result<(), DbError> {
    /// let moved = db
    ///     .transact(async |tx| {
    ///         let debit = Query::update().table("accounts").value("balance", 0).to_owned();
    ///         tx.execute(&debit).await
    ///     })
    ///     .await?;
    /// # let _ = moved; Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    /// The error of `f`, or a [`DbError::Query`] from `BEGIN` or `COMMIT`.
    pub async fn transact<T, F>(&self, f: F) -> Result<T, DbError>
    where
        F: AsyncFnOnce(&mut Tx<'_>) -> Result<T, DbError>,
    {
        run(self.begin().await?, f).await
    }

    /// Starts a transaction and hands back its guard. Call [`Tx::commit`] to
    /// keep the writes. Dropping the guard rolls back.
    ///
    /// Prefer [`Db::transact`]: it cannot forget the commit.
    ///
    /// # Errors
    /// [`DbError::Query`] when no connection was free or `BEGIN` failed.
    pub async fn begin(&self) -> Result<Tx<'static>, DbError> {
        Ok(Tx {
            inner: self.pool.begin().await.map_err(DbError::query)?,
        })
    }

    /// Runs `SELECT 1` on a pooled connection. For a readiness probe.
    ///
    /// # Errors
    /// [`DbError::Query`] when no connection was free in `acquire_timeout`
    /// or the server did not answer.
    pub async fn ping(&self) -> Result<(), DbError> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map(drop)
            .map_err(DbError::query)
    }

    /// Waits for the connections in use to come back, then closes all of
    /// them. Every clone of this `Db` is closed too. For shutdown.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

impl Exec for Db {
    fn executor(&mut self) -> impl PgExecutor<'_> {
        &self.pool
    }
}

impl Exec for &Db {
    fn executor(&mut self) -> impl PgExecutor<'_> {
        &self.pool
    }
}

/// A transaction on one connection.
///
/// Dropping it without [`Tx::commit`] rolls back. Run statements through
/// [`Exec`]. A `Tx` from [`Db::begin`] is `Tx<'static>`: it owns its
/// connection. A `Tx` from [`Tx::begin`] is a savepoint and borrows its
/// parent.
#[derive(Debug)]
pub struct Tx<'c> {
    inner: Transaction<'c, Postgres>,
}

impl<'c> Tx<'c> {
    /// Keeps the writes: `COMMIT`, or `RELEASE SAVEPOINT` for a nested `Tx`.
    ///
    /// # Errors
    /// [`DbError::Query`] when the server refused the commit, for example a
    /// deferred constraint or a serialization failure.
    pub async fn commit(self) -> Result<(), DbError> {
        self.inner.commit().await.map_err(DbError::query)
    }

    /// Discards the writes now, rather than at drop.
    ///
    /// # Errors
    /// [`DbError::Query`] when the connection failed during the rollback.
    /// The server discards the writes in that case too.
    pub async fn rollback(self) -> Result<(), DbError> {
        self.inner.rollback().await.map_err(DbError::query)
    }

    /// Starts a nested transaction: a `SAVEPOINT` inside this one.
    ///
    /// # Errors
    /// [`DbError::Query`] when `SAVEPOINT` failed.
    pub async fn begin(&mut self) -> Result<Tx<'_>, DbError> {
        Ok(Tx {
            inner: self.inner.begin().await.map_err(DbError::query)?,
        })
    }

    /// [`Db::transact`] inside this transaction, on a savepoint. `Err` rolls
    /// back to the savepoint and leaves the outer transaction usable.
    ///
    /// # Errors
    /// The error of `f`, or a [`DbError::Query`] from the savepoint.
    pub async fn transact<T, F>(&mut self, f: F) -> Result<T, DbError>
    where
        F: AsyncFnOnce(&mut Tx<'_>) -> Result<T, DbError>,
    {
        run(self.begin().await?, f).await
    }
}

impl Exec for Tx<'_> {
    fn executor(&mut self) -> impl PgExecutor<'_> {
        &mut *self.inner
    }
}

/// Commit on `Ok`, roll back on `Err`.
///
/// When the rollback itself fails, the error of `f` still wins: it is the
/// one that says why. The server discards the writes either way, because a
/// connection that failed mid-rollback is closed, and a closed connection's
/// transaction does not commit.
async fn run<'c, T, F>(mut tx: Tx<'c>, f: F) -> Result<T, DbError>
where
    F: AsyncFnOnce(&mut Tx<'_>) -> Result<T, DbError>,
{
    match f(&mut tx).await {
        Ok(value) => {
            tx.commit().await?;
            Ok(value)
        }
        Err(e) => {
            let _ = tx.rollback().await;
            Err(e)
        }
    }
}
