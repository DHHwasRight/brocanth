//! Applying SeaORM migrations under a lock.
//!
//! SeaORM's `MigratorTrait::up` takes no lock. Two `migrate up` runs at one
//! moment both read the same pending list, and both run the same
//! `CREATE TABLE`: the second fails with `duplicate key value violates
//! unique constraint "pg_type_typname_nsp_index"`, or applies a migration
//! twice. [`locked`] runs the work in one transaction that first takes
//! `pg_advisory_xact_lock`, so the second run waits, then finds nothing
//! pending.
//!
//! The lock is a transaction lock: it ends at `COMMIT` or `ROLLBACK`, also
//! when the caller drops the future, so no connection goes back to the pool
//! still holding it. SeaORM runs a Postgres migration inside one transaction
//! already, so a nested `begin` in it is a savepoint, and a failure leaves
//! nothing behind.
//!
//! The transaction also drops the session limits an application pool can
//! set: it is read-write whatever `default_transaction_read_only` says, and
//! has no `statement_timeout`. It sets `lock_timeout`, so a migration that
//! waits on a busy table fails and does not queue every query behind it.

use std::time::Duration;

use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, DbErr, Statement,
    TransactionTrait,
};

/// The key of the lock: the eight ASCII bytes of `brocmodl`, read as a
/// big-endian `i64`, so the row in `pg_locks` reads back as text.
/// `brocanth-jobs`, `brocanth-storage` and `brocanth-cache` key their own
/// migration locks the same way, each on its own word.
pub const LOCK: i64 = 0x6272_6f63_6d6f_646c; // "brocmodl"

/// How long a statement of a migration waits for a table lock before it
/// fails, unless the caller passes another value: a generated `migrate`
/// passes `[db] lock_timeout`. The same as `brocanth-db`'s.
pub const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(10);

/// Runs `f` in a transaction that holds the migration lock.
///
/// `f` returning `Err` rolls the transaction back. `lock_timeout` is set
/// after the lock is taken, so a run that waits for another run to finish
/// waits as long as that takes. `None` sets no `lock_timeout`.
///
/// ```ignore
/// let before = brocanth_model::migrating::locked(&conn, Some(DEFAULT_LOCK_TIMEOUT), async |txn| {
///     let before = Migrator::get_migration_with_status(txn).await?;
///     Migrator::up(txn, None).await?;
///     Ok(before)
/// })
/// .await?;
/// ```
///
/// # Errors
/// The error of `f`, or a [`DbErr`] from `BEGIN`, the lock, a `SET` or
/// `COMMIT`.
pub async fn locked<T>(
    conn: &DatabaseConnection,
    lock_timeout: Option<Duration>,
    f: impl AsyncFnOnce(&DatabaseTransaction) -> Result<T, DbErr>,
) -> Result<T, DbErr> {
    let txn = conn.begin().await?;
    // First statement of the transaction: the only place it can change.
    txn.execute_unprepared("SET TRANSACTION READ WRITE").await?;
    // Before the lock: the wait for another run is no statement to cancel.
    txn.execute_unprepared("SET LOCAL statement_timeout = 0")
        .await?;
    txn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT pg_advisory_xact_lock($1)",
        [LOCK.into()],
    ))
    .await?;
    let lock_ms = lock_timeout.map_or(0, |t| t.as_millis().clamp(1, i32::MAX as u128));
    txn.execute_unprepared(&format!("SET LOCAL lock_timeout = {lock_ms}"))
        .await?;
    match f(&txn).await {
        Ok(value) => {
            txn.commit().await?;
            Ok(value)
        }
        Err(e) => {
            // The error of `f` says why. A failed rollback closes the
            // connection, and its transaction does not commit.
            let _ = txn.rollback().await;
            Err(with_lock_help(e))
        }
    }
}

/// A `lock_timeout` failure (SQLSTATE `55P03`), with a `help:` line that
/// names the setting. Any other error comes back unchanged.
fn with_lock_help(e: DbErr) -> DbErr {
    let timed_out = match &e {
        DbErr::Exec(sea_orm::RuntimeErr::SqlxError(sqlx))
        | DbErr::Query(sea_orm::RuntimeErr::SqlxError(sqlx)) => sqlx
            .as_database_error()
            .and_then(|db| db.code())
            .is_some_and(|code| code == "55P03"),
        _ => false,
    };
    if !timed_out {
        return e;
    }
    DbErr::Custom(format!(
        "{e}\nhelp: a migration statement waited longer than `lock_timeout` for a table \
         another session holds; run `migrate` again when that session ends, or raise \
         `[db] lock_timeout`"
    ))
}
