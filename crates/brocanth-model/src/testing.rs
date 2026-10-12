//! A transactional test harness: begin, run, never commit.
//!
//! [`Harness::begin`] hands back a [`TestTransaction`] that has no `commit`
//! method at all, so the "I forgot to roll back and the next test saw my rows"
//! failure cannot be written. The test's own code can still open transactions
//! inside it, because SeaORM nests through real `SAVEPOINT`s.

use std::ops::Deref;

use sea_orm::{
    ConnectOptions, Database, DatabaseConnection, DatabaseTransaction, DbErr, TransactionTrait,
};
use sea_orm_migration::MigratorTrait;

use crate::Probe;

/// A database for one test, with the probe already attached.
#[derive(Debug)]
pub struct Harness {
    conn: DatabaseConnection,
    probe: Probe,
}

impl Harness {
    /// An empty in-memory SQLite database.
    ///
    /// One connection, because every connection to `sqlite::memory:` is its own
    /// database and a pool of two would be two databases.
    pub async fn sqlite() -> Result<Self, DbErr> {
        Self::sqlite_with(Probe::default()).await
    }

    /// The same, with a probe the caller configured.
    pub async fn sqlite_with(probe: Probe) -> Result<Self, DbErr> {
        let mut options = ConnectOptions::new("sqlite::memory:");
        options
            .max_connections(1)
            .min_connections(1)
            .sqlx_logging(false);
        let mut conn = Database::connect(options).await?;
        probe.install(&mut conn);
        Ok(Self { conn, probe })
    }

    /// An in-memory SQLite database with `M` applied.
    pub async fn migrated<M: MigratorTrait>() -> Result<Self, DbErr> {
        let harness = Self::sqlite().await?;
        M::up(&harness.conn, None).await?;
        Ok(harness)
    }

    /// The connection. Writes through it are not rolled back; prefer
    /// [`Harness::begin`].
    pub fn conn(&self) -> &DatabaseConnection {
        &self.conn
    }

    /// The probe attached to this connection.
    pub fn probe(&self) -> &Probe {
        &self.probe
    }

    /// Apply a migrator to this database.
    pub async fn migrate<M: MigratorTrait>(&self) -> Result<(), DbErr> {
        M::up(&self.conn, None).await
    }

    /// Roll a migrator back, `steps` at a time, or all the way with `None`.
    pub async fn rollback<M: MigratorTrait>(&self, steps: Option<u32>) -> Result<(), DbErr> {
        M::down(&self.conn, steps).await
    }

    /// Open the test's transaction.
    ///
    /// The returned guard cannot commit. It rolls back when it is dropped, or
    /// earlier if [`TestTransaction::rollback`] is called.
    pub async fn begin(&self) -> Result<TestTransaction, DbErr> {
        Ok(TestTransaction {
            txn: self.conn.begin().await?,
        })
    }
}

/// A transaction that cannot be committed.
///
/// Derefs to the underlying [`DatabaseTransaction`], so it is a
/// `ConnectionTrait` and a `TransactionTrait`: a test can call `begin()` on it
/// and get a real `SAVEPOINT`, commit *that* one, and still have everything
/// discarded when the harness guard drops.
#[derive(Debug)]
pub struct TestTransaction {
    txn: DatabaseTransaction,
}

impl TestTransaction {
    /// Discard everything now instead of at the end of the scope.
    pub async fn rollback(self) -> Result<(), DbErr> {
        self.txn.rollback().await
    }
}

impl Deref for TestTransaction {
    type Target = DatabaseTransaction;
    fn deref(&self) -> &DatabaseTransaction {
        &self.txn
    }
}
