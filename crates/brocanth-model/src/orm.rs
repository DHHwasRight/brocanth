//! [`Orm`]: SeaORM over the pool [`brocanth_db::Db`] opened.
//!
//! Behind the `db` feature. ADR 2 sets the order: the pool comes first, and
//! the ORM is registered after it because it needs it. So this type does not
//! open a pool. It wraps the one `brocanth-db` holds, and a sea-query
//! statement through `Db` and an entity query through `Orm` use the same
//! connections, the same session settings and the same limits.
//!
//! ```no_run
//! use brocanth_db::{Db, DbConfig};
//! use brocanth_model::sea_orm::TransactionTrait;
//! use brocanth_model::Orm;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let db = Db::connect(&DbConfig::new("postgres://localhost/app")).await?;
//! let orm = Orm::from_db(&db);
//! // `Orm` derefs to `DatabaseConnection`, so every SeaORM call takes
//! // `&*orm` or `orm.connection()`, and a transaction is a method on it.
//! let tx = orm.begin().await?;
//! tx.commit().await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Why a newtype and not an alias
//!
//! A `type Orm = DatabaseConnection` cannot be a key. The plugin puts the
//! handle into the application's services and onto the request context, and
//! both are keyed by type. `ContextItem` is the core's trait and
//! `DatabaseConnection` is SeaORM's type, so no crate may implement one for
//! the other. A local type can. It also says which connection it is: the
//! one over the application's shared pool, and not some other connection a
//! plugin opened.
//!
//! The newtype costs nothing at the call site. It derefs to
//! `DatabaseConnection`, and [`Orm::connection`] and
//! `From<Orm> for DatabaseConnection` hand the connection to an API that
//! names the SeaORM type, such as `brocanth_admin_model::admin_source!`.

use std::ops::Deref;

use brocanth_db::{Db, DbIdentity};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, DbErr,
    SqlxPostgresConnector, Statement, TransactionTrait,
};

/// SeaORM's connection over the pool a [`Db`] holds.
///
/// A clone is cheap: it shares the pool.
#[derive(Clone, Debug)]
pub struct Orm(DatabaseConnection);

impl Orm {
    /// The ORM over `db`'s pool. Opens no connection.
    pub fn from_db(db: &Db) -> Orm {
        Orm(SqlxPostgresConnector::from_sqlx_postgres_pool(
            db.pool().clone(),
        ))
    }

    /// The same ORM with `probe` counting every statement it issues: what
    /// the `debug-toolbar` module installs in development.
    pub fn probed(mut self, probe: &crate::Probe) -> Orm {
        probe.install(&mut self.0);
        self
    }

    /// The SeaORM connection, for a call that takes `&C: ConnectionTrait`.
    pub fn connection(&self) -> &DatabaseConnection {
        &self.0
    }

    /// The SeaORM connection, by value.
    pub fn into_connection(self) -> DatabaseConnection {
        self.0
    }

    /// Runs `f` in a transaction that holds the migration lock, with
    /// [`crate::migrating::DEFAULT_LOCK_TIMEOUT`]. A generated
    /// `migrate up` and `migrate down` call this, so two runs at one moment
    /// do not apply one migration twice. See [`crate::migrating::locked`].
    ///
    /// ```ignore
    /// orm.migrate_locked(async |txn| Migrator::up(txn, None).await).await?;
    /// ```
    ///
    /// # Errors
    /// The error of `f`, or a [`DbErr`] from `BEGIN`, the lock or `COMMIT`.
    pub async fn migrate_locked<T>(
        &self,
        f: impl AsyncFnOnce(&DatabaseTransaction) -> Result<T, DbErr>,
    ) -> Result<T, DbErr> {
        crate::migrating::locked(&self.0, Some(crate::migrating::DEFAULT_LOCK_TIMEOUT), f).await
    }

    /// A SeaORM transaction as `identity`, for a table under Postgres
    /// row-level security (`docs/rls.md`). The transaction runs
    /// `SET LOCAL ROLE` and `set_config('brocanth.<key>', $value, true)`
    /// first, the statements [`Db::transact_as`] runs, built by
    /// [`DbIdentity::statements`]. Both end at `COMMIT` or `ROLLBACK`, so
    /// the next user of the connection never sees them. `f` returning
    /// `Err` rolls back.
    ///
    /// ```no_run
    /// use brocanth_db::DbIdentity;
    /// use brocanth_model::sea_orm::{ConnectionTrait, DbErr};
    /// use brocanth_model::Orm;
    ///
    /// # async fn example(orm: Orm) -> Result<(), DbErr> {
    /// let me = DbIdentity::new().role("app_member").set("tenant_id", 7);
    /// orm.transact_as(&me, async |txn| {
    ///     txn.execute_unprepared("SELECT 1").await?;
    ///     Ok(())
    /// })
    /// .await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    /// [`DbErr::Custom`] with a `help:` line for a role, key or value
    /// [`DbIdentity`] refuses; the error of `f`; a [`DbErr`] from `BEGIN`,
    /// `SET ROLE` or `COMMIT`.
    pub async fn transact_as<T, F>(&self, identity: &DbIdentity, f: F) -> Result<T, DbErr>
    where
        F: AsyncFnOnce(&DatabaseTransaction) -> Result<T, DbErr>,
    {
        // Checked before `BEGIN`, so a bad identity takes no connection.
        identity
            .statements()
            .map_err(|e| DbErr::Custom(e.to_string()))?;
        let txn = self.0.begin().await?;
        assume(&txn, identity).await?;
        match f(&txn).await {
            Ok(value) => {
                txn.commit().await?;
                Ok(value)
            }
            Err(e) => {
                // The error of `f` says why; a failed rollback closes the
                // connection, and its transaction does not commit.
                let _ = txn.rollback().await;
                Err(e)
            }
        }
    }
}

/// Runs the rest of the transaction `txn` as `identity`: `SET LOCAL ROLE`
/// and `set_config('brocanth.<key>', $value, true)`, the statements
/// [`DbIdentity::statements`] builds. For a SeaORM transaction this crate
/// did not open, such as the admin's data source.
///
/// Call it first in the transaction: it does not clear what an earlier call
/// set, as `Tx::assume` does.
///
/// # Errors
/// [`DbErr::Custom`] with a `help:` line for a role, key or value
/// [`DbIdentity`] refuses; a [`DbErr`] from `SET ROLE` or `set_config`.
pub async fn assume<C: ConnectionTrait>(txn: &C, identity: &DbIdentity) -> Result<(), DbErr> {
    let statements = identity
        .statements()
        .map_err(|e| DbErr::Custom(e.to_string()))?;
    if let Some(role) = statements.role {
        txn.execute_unprepared(&role).await?;
    }
    if let Some((sql, binds)) = statements.settings {
        let values = binds.into_iter().map(sea_orm::Value::from);
        txn.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .await?;
    }
    Ok(())
}

impl Deref for Orm {
    type Target = DatabaseConnection;

    fn deref(&self) -> &DatabaseConnection {
        &self.0
    }
}

impl From<&Db> for Orm {
    fn from(db: &Db) -> Orm {
        Orm::from_db(db)
    }
}

impl From<Orm> for DatabaseConnection {
    fn from(orm: Orm) -> DatabaseConnection {
        orm.0
    }
}

/// The plugin publishes the handle on the request context, so a handler
/// reads it with `ctx.provided::<Orm, _>()`.
#[cfg(feature = "plugin")]
impl brocanth::plugin_context::ContextItem for Orm {}
