//! [`Db`], the pool handle, and [`Tx`], one transaction on one connection.

use sqlx::postgres::{PgExecutor, PgPool};
use sqlx::{Acquire, Postgres, Transaction};

use crate::config::DbConfig;
use crate::error::DbError;
use crate::exec::Exec;
use std::sync::{Arc, Mutex};

use crate::principal::{Assumed, DbIdentity};

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
    /// one. For a synchronous stage, which must not wait for I/O: check the
    /// pool later with [`Db::ping`], from async code.
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

    /// [`Db::transact`] as `identity`: the transaction assumes it first
    /// ([`Tx::assume`]). For a request under row-level security
    /// (`docs/rls.md`).
    ///
    /// ```no_run
    /// # use brocanth_db::{Db, DbError, DbIdentity, Exec};
    /// # use brocanth_db::sea_query::Query;
    /// # async fn example(db: Db) -> Result<(), DbError> {
    /// let me = DbIdentity::new().role("app_member").set("tenant_id", 7);
    /// let q = Query::select().column("title").from("posts").to_owned();
    /// let titles: Vec<(String,)> = db.transact_as(&me, async |tx| tx.all(&q).await).await?;
    /// # let _ = titles; Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    /// [`DbError::Config`] for a role, key or value [`DbIdentity`] refuses; the
    /// error of `f`; a [`DbError::Query`] from `BEGIN`, `SET ROLE` or
    /// `COMMIT`.
    pub async fn transact_as<T, F>(&self, identity: &DbIdentity, f: F) -> Result<T, DbError>
    where
        F: AsyncFnOnce(&mut Tx<'_>) -> Result<T, DbError>,
    {
        let mut tx = self.begin().await?;
        tx.assume(identity).await?;
        run(tx, f).await
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
            assumed: Arc::default(),
        })
    }

    /// Runs `SELECT 1` on a pooled connection. For a readiness probe, and
    /// for the startup gate of a pool opened with [`Db::connect_lazy`].
    ///
    /// Taking the connection is reported as [`Db::connect`] reports it, so a
    /// lazy pool with a wrong password fails here with the same message and
    /// `help:` line.
    ///
    /// # Errors
    /// [`DbError::Connect`] when no connection could be taken in
    /// `acquire_timeout`; [`DbError::Query`] when the server did not answer.
    pub async fn ping(&self) -> Result<(), DbError> {
        let mut conn = self.pool.acquire().await.map_err(DbError::connect)?;
        sqlx::query("SELECT 1")
            .execute(&mut *conn)
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
    /// What [`Tx::assume`] set in this transaction, shared with its
    /// savepoints, so a second call replaces the first.
    assumed: Arc<Mutex<Assumed>>,
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

    /// Runs the rest of this transaction as `identity`: `SET LOCAL ROLE`
    /// and `set_config('brocanth.<key>', $value, true)`. Both end at
    /// `COMMIT` or `ROLLBACK`, so the next user of the connection never sees
    /// them. On a savepoint, a rollback of the savepoint reverts them.
    ///
    /// A second call replaces the first; it does not merge with it. A key
    /// an earlier call set and this one does not is set to `''`, which a
    /// policy reads as `NULL`. With no role, the transaction returns to the
    /// login role (`SET LOCAL ROLE NONE`). Savepoints share this record.
    ///
    /// Each value is a bind parameter. The role is a quoted identifier,
    /// because `SET` takes no parameter.
    ///
    /// # Errors
    /// [`DbError::Config`] for a role, key or value [`DbIdentity`] refuses;
    /// [`DbError::Query`] when the login role is not a member of the role.
    pub async fn assume(&mut self, identity: &DbIdentity) -> Result<(), DbError> {
        let before = self.assumed.lock().map(|a| a.clone()).unwrap_or_default();
        let crate::principal::Statements { role, settings } = identity.statements_after(&before)?;
        if let Some(sql) = role {
            sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
                .execute(&mut *self.inner)
                .await
                .map_err(|e| role_error(e, identity))?;
        }
        if let Some((sql, binds)) = settings {
            let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
            for bind in binds {
                query = query.bind(bind);
            }
            query
                .execute(&mut *self.inner)
                .await
                .map_err(DbError::query)?;
        }
        if let Ok(mut assumed) = self.assumed.lock() {
            assumed.record(identity);
        }
        Ok(())
    }

    /// Starts a nested transaction: a `SAVEPOINT` inside this one.
    ///
    /// # Errors
    /// [`DbError::Query`] when `SAVEPOINT` failed.
    pub async fn begin(&mut self) -> Result<Tx<'_>, DbError> {
        Ok(Tx {
            inner: self.inner.begin().await.map_err(DbError::query)?,
            assumed: Arc::clone(&self.assumed),
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

/// A refused `SET ROLE`, with the grant that fixes it.
fn role_error(e: sqlx::Error, identity: &DbIdentity) -> DbError {
    let err = DbError::query(e);
    match (&err, identity.role_name()) {
        (DbError::Query { code: Some(code), message, .. }, Some(role))
            if code == "42501" || code == "22023" || code == "42704" =>
        {
            DbError::config(
                format!("SET ROLE {role}: {message}"),
                format!(
                    "create the role and grant it to the login: `Role::new(\"{role}\").member(<login>)` in brocanth-rls, or GRANT \"{role}\" TO <login>"
                ),
            )
        }
        _ => err,
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
