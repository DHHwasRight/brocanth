//! A transactional test harness: begin, run, never commit.
//!
//! [`Harness::begin`] hands back a [`TestTransaction`] that has no `commit`
//! method at all, so the "I forgot to roll back and the next test saw my rows"
//! failure cannot be written. The test's own code can still open transactions
//! inside it, because SeaORM nests through real `SAVEPOINT`s.
//!
//! # Two shapes of isolation
//!
//! Brocanth supports Postgres only. One server serves every test at once, so
//! a test needs isolation from the others. There are two shapes:
//!
//! **A shared schema and a transaction.** [`Harness::postgres_migrated`]
//! creates the schema once and shares it, and the transaction from
//! [`Harness::begin`] keeps the tests apart. The once part uses a
//! transaction-scoped advisory lock, which is safe across parallel tests *and*
//! across parallel `cargo test` processes, and it is atomic because Postgres
//! rolls back DDL like anything else. Rows written through [`Harness::conn`]
//! stay. Use [`Harness::begin`].
//!
//! **A schema of its own.** [`Harness::isolated`] creates a new, empty schema
//! and points every connection's `search_path` at it, so the test sees an
//! empty database. Use it when a test needs the database to itself: it
//! migrates up and down, counts every table, or writes through
//! [`Harness::conn`]. The schema is dropped when the harness drops.
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
    /// Set for [`Harness::isolated`]: the schema to drop at the end.
    isolation: Option<Isolation>,
}

/// The server and the schema of an isolated harness.
#[derive(Debug)]
struct Isolation {
    url: String,
    schema: String,
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
        Self::POSTGRES_URL_VARS.iter().find_map(|name| {
            std::env::var(name)
                .ok()
                .filter(|url| !url.trim().is_empty())
        })
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
        Ok(Self {
            conn,
            probe,
            isolation: None,
        })
    }

    /// A new, empty schema on the server that [`Harness::postgres_url`]
    /// names, or `None` when no variable names one.
    ///
    /// Every connection of the harness has its `search_path` set to the
    /// schema, so unqualified names resolve there and the test sees an empty
    /// database. The schema is dropped when the harness drops. See the module
    /// documentation for when to use this and when to use
    /// [`Harness::postgres_migrated`].
    pub async fn isolated() -> Result<Option<Self>, DbErr> {
        Self::isolated_with(Probe::default()).await
    }

    /// The same, with a probe the caller configured.
    pub async fn isolated_with(probe: Probe) -> Result<Option<Self>, DbErr> {
        let Some(url) = Self::postgres_url() else {
            return Ok(None);
        };
        Self::postgres_isolated_with(&url, probe).await.map(Some)
    }

    /// [`Harness::isolated`] with `M` applied.
    pub async fn isolated_migrated<M: MigratorTrait>() -> Result<Option<Self>, DbErr> {
        let Some(harness) = Self::isolated().await? else {
            return Ok(None);
        };
        harness.migrate::<M>().await?;
        Ok(Some(harness))
    }

    /// A new, empty schema on `url`. See [`Harness::isolated`].
    pub async fn postgres_isolated(url: &str) -> Result<Self, DbErr> {
        Self::postgres_isolated_with(url, Probe::default()).await
    }

    /// The same, with a probe the caller configured.
    pub async fn postgres_isolated_with(url: &str, probe: Probe) -> Result<Self, DbErr> {
        use sea_orm::ConnectionTrait;

        let schema = unique_schema();
        {
            let admin = Database::connect(admin_options(url)).await?;
            admin
                .execute_unprepared(&format!("CREATE SCHEMA \"{schema}\""))
                .await?;
            admin.close().await?;
        }

        let mut options = ConnectOptions::new(url.to_owned());
        let name = schema.clone();
        options
            .max_connections(2)
            .min_connections(1)
            .sqlx_logging(false)
            // The application name lets the drop find this harness's
            // sessions, and end them, before it drops the schema.
            .map_sqlx_postgres_opts(move |opts| {
                opts.application_name(&name)
                    .options([("search_path", name.as_str())])
            });
        let mut conn = Database::connect(options).await?;
        probe.install(&mut conn);
        Ok(Self {
            conn,
            probe,
            isolation: Some(Isolation {
                url: url.to_owned(),
                schema,
            }),
        })
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

        // The eight ASCII bytes of `brocmodl`, read as a big-endian `i64`, so
        // the row in `pg_locks` can be read back as text and identified.
        // `brocanth-jobs`, `brocanth-storage` and `brocanth-cache` key their
        // own migration locks the same way, each on its own word.
        const LOCK: i64 = 0x6272_6f63_6d6f_646c; // "brocmodl"

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

#[cfg(feature = "postgres")]
impl Drop for Harness {
    /// Drops the schema of an isolated harness.
    ///
    /// `Drop` cannot await, and the test's runtime may be the thread that
    /// runs this. So the work runs on a thread of its own with a runtime of
    /// its own. It first ends this harness's sessions: a transaction that a
    /// test dropped can still hold a lock on a table, and `DROP SCHEMA` would
    /// wait for it forever. A failure leaves the schema behind and prints a
    /// line; it never fails the test.
    fn drop(&mut self) {
        let Some(Isolation { url, schema }) = self.isolation.take() else {
            return;
        };
        let cleanup = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?;
            runtime.block_on(async {
                use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

                let admin = Database::connect(admin_options(&url))
                    .await
                    .map_err(|e| e.to_string())?;
                admin
                    .execute_raw(Statement::from_sql_and_values(
                        DatabaseBackend::Postgres,
                        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
                         WHERE application_name = $1 AND pid <> pg_backend_pid()",
                        [schema.clone().into()],
                    ))
                    .await
                    .map_err(|e| e.to_string())?;
                admin
                    .execute_unprepared(&format!("DROP SCHEMA IF EXISTS \"{schema}\" CASCADE"))
                    .await
                    .map_err(|e| e.to_string())?;
                admin.close().await.map_err(|e| e.to_string())
            })
        });
        match cleanup.join() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => skipped(&format!("the test schema was not dropped: {e}")),
            Err(_) => skipped("the test schema was not dropped: the cleanup panicked"),
        }
    }
}

/// One connection, for the statements around an isolated harness.
#[cfg(feature = "postgres")]
fn admin_options(url: &str) -> ConnectOptions {
    let mut options = ConnectOptions::new(url.to_owned());
    options
        .max_connections(1)
        .min_connections(0)
        .sqlx_logging(false);
    options
}

/// A schema name no other test in any process has: the process id, a counter
/// and the clock.
#[cfg(feature = "postgres")]
fn unique_schema() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("brocanth_test_{}_{n}_{nanos}", std::process::id())
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
