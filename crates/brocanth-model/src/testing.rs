//! A transactional test harness: begin, run, never commit.
//!
//! [`Harness::begin`] hands back a [`TestTransaction`] that has no `commit`
//! method at all, so the "I forgot to roll back and the next test saw my rows"
//! failure cannot be written. The test's own code can still open transactions
//! inside it, because SeaORM nests through real `SAVEPOINT`s.
//!
//! # Two backends, two shapes of isolation
//!
//! **SQLite** gets a private database per test: `sqlite::memory:` is a new,
//! empty database for every connection, so [`Harness::sqlite`] needs no
//! cleanup and no coordination. The transaction is there to keep a test's own
//! writes out of the test's own later assertions, not to keep tests apart.
//!
//! **Postgres** is a server, and one server serves every test at once. So the
//! schema is created once and shared, and the transaction is what keeps the
//! tests apart. [`Harness::postgres_migrated`] does the once part with a
//! transaction-scoped advisory lock, which is safe across parallel tests *and*
//! across parallel `cargo test` processes, and it is atomic because Postgres
//! rolls back DDL like anything else. SQLite cannot do the same thing, which
//! is the first place the base-backend decision shows up in this crate.
//!
//! Rows written through [`Harness::begin`] are discarded on both. Rows written
//! through [`Harness::conn`] are discarded on SQLite only, because there the
//! whole database goes away. On Postgres they stay. Use [`Harness::begin`].
//!
//! # Running the Postgres tests
//!
//! ```sh
//! BROCANTH_MODEL_TEST_POSTGRES=postgres://localhost/brocanth_model_test \
//!   cargo test -p brocanth-model
//! ```
//!
//! `BROCANTH_TEST_DATABASE_URL` is read as well, so one variable can point
//! every brocanth crate at the same server.

use std::ops::Deref;

use sea_orm::{
    ConnectOptions, Database, DatabaseConnection, DatabaseTransaction, DbErr, TransactionTrait,
};
use sea_orm_migration::MigratorTrait;

use crate::Probe;

/// Announce that a test did not run, where the announcement cannot be
/// swallowed.
///
/// `cargo test` captures `println!` and `eprintln!` and shows them only for a
/// test that fails, so a skip announced that way is invisible — and a test
/// that silently passes by running nothing is worse than one that is honestly
/// skipped. This writes to file descriptor 2 directly, which the capture does
/// not intercept, and names the test through its thread name.
///
/// ```ignore
/// let Some(url) = Harness::postgres_url() else {
///     testing::skipped("no server: set BROCANTH_MODEL_TEST_POSTGRES");
///     return;
/// };
/// ```
pub fn skipped(reason: &str) {
    use std::io::Write;

    let thread = std::thread::current();
    let name = thread.name().unwrap_or("a test");
    let line = format!("!! SKIPPED {name}: {reason}\n");

    if let Ok(mut fd2) = std::fs::OpenOptions::new().write(true).open("/dev/stderr") {
        if fd2.write_all(line.as_bytes()).is_ok() {
            let _ = fd2.flush();
            return;
        }
    }
    eprint!("{line}");
}

/// A database for one test, with the probe already attached.
#[derive(Debug)]
pub struct Harness {
    conn: DatabaseConnection,
    probe: Probe,
}

#[cfg(feature = "sqlite")]
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
}

#[cfg(feature = "postgres")]
impl Harness {
    /// The environment variables that name the test server, in the order they
    /// are read.
    pub const POSTGRES_URL_VARS: [&'static str; 2] =
        ["BROCANTH_MODEL_TEST_POSTGRES", "BROCANTH_TEST_DATABASE_URL"];

    /// The test server's URL, or `None` when no variable names one.
    ///
    /// A test that gets `None` must say so on stderr and return. A Postgres
    /// test that quietly passes because there was no Postgres is worse than no
    /// test at all.
    pub fn postgres_url() -> Option<String> {
        Self::POSTGRES_URL_VARS
            .iter()
            .find_map(|name| std::env::var(name).ok())
            .filter(|url| !url.is_empty())
    }

    /// A connection to `url`.
    ///
    /// A small pool, because a test holds one connection for the length of its
    /// transaction and the suite is parallel.
    pub async fn postgres(url: &str) -> Result<Self, DbErr> {
        Self::postgres_with(url, Probe::default()).await
    }

    /// The same, with a probe the caller configured.
    pub async fn postgres_with(url: &str, probe: Probe) -> Result<Self, DbErr> {
        let mut options = ConnectOptions::new(url.to_owned());
        options
            .max_connections(2)
            .min_connections(1)
            .sqlx_logging(false);
        let mut conn = Database::connect(options).await?;
        probe.install(&mut conn);
        Ok(Self { conn, probe })
    }

    /// A connection to `url` with `M` applied, once, whoever gets there first.
    ///
    /// The migration runs inside a transaction that first takes
    /// `pg_advisory_xact_lock`, so two tests, two threads or two `cargo test`
    /// processes racing to create the same tables serialise instead of
    /// colliding, and a migration that fails halfway leaves nothing behind.
    /// `M::up` is already idempotent through `seaql_migrations`, so the second
    /// caller through the lock does nothing.
    pub async fn postgres_migrated<M: MigratorTrait>(url: &str) -> Result<Self, DbErr> {
        Self::postgres_migrated_with::<M>(url, Probe::default()).await
    }

    /// The same, with a probe the caller configured.
    pub async fn postgres_migrated_with<M: MigratorTrait>(
        url: &str,
        probe: Probe,
    ) -> Result<Self, DbErr> {
        let harness = Self::postgres_with(url, probe).await?;
        harness.migrate_once::<M>().await?;
        Ok(harness)
    }

    /// [`Harness::postgres_migrated`] against [`Harness::postgres_url`], or
    /// `None` when no variable names a server.
    pub async fn postgres_migrated_from_env<M: MigratorTrait>() -> Result<Option<Self>, DbErr> {
        match Self::postgres_url() {
            Some(url) => Self::postgres_migrated::<M>(&url).await.map(Some),
            None => Ok(None),
        }
    }

    /// Applies `M` under an advisory lock. See [`Harness::postgres_migrated`].
    ///
    /// Use this and not [`Harness::migrate`] on Postgres. [`Harness::migrate`]
    /// is a bare `M::up`, and two tests running it at the same moment against
    /// one server fail with `duplicate key value violates unique constraint
    /// "pg_type_typname_nsp_index"` — two `CREATE TABLE`s for the same name,
    /// racing in the catalogue. That failure is the reason this method exists.
    pub async fn migrate_once<M: MigratorTrait>(&self) -> Result<(), DbErr> {
        use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

        // An arbitrary constant, fixed so that every brocanth-model test
        // process picks the same lock.
        const LOCK: i64 = 0x6272_6f6d_6f64; // "bromod"

        let txn = self.conn.begin().await?;
        txn.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT pg_advisory_xact_lock($1)",
            [LOCK.into()],
        ))
        .await?;
        M::up(&txn, None).await?;
        txn.commit().await
    }
}

impl Harness {
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
