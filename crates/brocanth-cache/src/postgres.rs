//! The Postgres backend.
//!
//! One `UNLOGGED` table, a primary key, and no second index. The arguments for
//! each of those are in [`sql`], next to the statements they justify; the
//! argument for keeping a cache in Postgres at all is in [`crate`].

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sqlx::{PgPool, Row as _};

use crate::cache::{expires_at, Cache, DEFAULT_MAX_VALUE_BYTES};
use crate::clock::{from_millis, to_millis, Clock, SystemClock};
use crate::error::{CacheError, Result};
use crate::key::{Key, Namespace};

pub(crate) mod sql;

/// A cache kept in Postgres.
///
/// Cheap to clone — it is a pool handle, an `Arc` around a clock and a number
/// — so it goes in the context, in a handler, in a hook, anywhere.
///
/// ```no_run
/// # use std::time::Duration;
/// # use brocanth_cache::{Cache, Namespace, PgCache};
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let cache = PgCache::connect("postgres://localhost/app").await?;
/// cache.migrate().await?;
///
/// let views = Namespace::new("views")?;
/// cache
///     .set(&views.key("home")?, b"<h1>hello</h1>", Duration::from_secs(60))
///     .await?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct PgCache {
    pool: PgPool,
    clock: Arc<dyn Clock>,
    max_value_bytes: usize,
}

impl std::fmt::Debug for PgCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgCache")
            .field("max_value_bytes", &self.max_value_bytes)
            .finish_non_exhaustive()
    }
}

impl PgCache {
    /// Opens a cache from a database URL.
    ///
    /// A URL that is not `postgres://…` or `postgresql://…` is an error here
    /// rather than a connection failure later, and the message says which
    /// crate to look at — because a SQLite cache is a different crate, not a
    /// different URL.
    ///
    /// # Errors
    /// [`CacheError::Config`] for a URL this crate cannot use;
    /// [`CacheError::Backend`] when the connection could not be made.
    pub async fn connect(url: &str) -> Result<Self> {
        if !(url.starts_with("postgres://") || url.starts_with("postgresql://")) {
            return Err(CacheError::Config(format!(
                "`{}` is not a Postgres URL. brocanth-cache is Postgres only on \
                 purpose; a cache for a SQLite deployment is a separate crate \
                 implementing the same `Cache` trait.",
                url.split(':').next().unwrap_or(url)
            )));
        }
        Ok(Self::from_pool(PgPool::connect(url).await?))
    }

    /// Builds a cache on a pool the application already has.
    ///
    /// This is the path for an application that also uses `brocanth-model` or
    /// `brocanth-jobs`: SeaORM 2.0 hands out its underlying `sqlx` pool, so
    /// the models, the queue and the cache share one set of connections rather
    /// than opening three against the same database.
    ///
    /// Sharing one pool is also the honest version of the trade this crate
    /// makes. A cache read is a connection a request is not using for its real
    /// query, and that is visible in one pool and invisible in three.
    pub fn from_pool(pool: PgPool) -> Self {
        Self {
            pool,
            clock: Arc::new(SystemClock),
            max_value_bytes: DEFAULT_MAX_VALUE_BYTES,
        }
    }

    /// Replaces the clock. See [`crate::clock`] for why there is one.
    #[must_use]
    pub fn with_clock(mut self, clock: impl Clock) -> Self {
        self.clock = Arc::new(clock);
        self
    }

    /// Changes the cap on one value.
    ///
    /// See [`DEFAULT_MAX_VALUE_BYTES`] for what the default is and the three
    /// reasons a large entry costs more than it looks.
    #[must_use]
    pub fn with_max_value_bytes(mut self, bytes: usize) -> Self {
        self.max_value_bytes = bytes;
        self
    }

    /// The pool underneath, for an application that wants to run its own
    /// statement against the cache table.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// The current time, from this cache's clock.
    pub fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.clock.now()
    }

    /// The DDL [`Cache::migrate`] would run, as statements.
    ///
    /// For an application whose rule is that the schema lives in files a human
    /// reviewed — which is §4.3 of the plan, and the right rule — paste these
    /// into a migration and never call `migrate`. **Keep the `UNLOGGED`**: a
    /// migration that quietly drops it produces a cache that works and costs
    /// what a cache in Postgres is supposed to cost.
    /// [`PgCache::is_unlogged`] is how a deployment check catches that.
    pub fn schema_sql() -> &'static [&'static str] {
        sql::SCHEMA
    }

    /// Whether the cache table really is unlogged.
    ///
    /// `None` when the table is not there at all. Assert this in a deployment
    /// check: a logged cache table works perfectly and writes every entry to
    /// the WAL and out to every replica, which is the cost the whole design
    /// exists to avoid, and nothing at run time will tell you.
    ///
    /// Fixing it is `ALTER TABLE brocanth_cache SET UNLOGGED`, which rewrites
    /// the table and takes an exclusive lock, so this crate will not do it for
    /// you.
    ///
    /// # Errors
    /// [`CacheError::Backend`] when the catalogue could not be read.
    pub async fn is_unlogged(&self) -> Result<Option<bool>> {
        let row = sqlx::query(sql::IS_UNLOGGED)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|r| r.try_get::<bool, _>(0))
            .transpose()
            .map_err(Into::into)
    }

    /// The size of the cache and the state of its garbage.
    ///
    /// # Errors
    /// [`CacheError::Backend`] when the figures could not be read.
    pub async fn stats(&self) -> Result<CacheStats> {
        let now = to_millis(self.clock.now());
        let row = sqlx::query(sql::STATS)
            .bind(now)
            .fetch_one(&self.pool)
            .await?;
        Ok(CacheStats {
            entries: row.try_get::<i64, _>("entries")?.max(0) as u64,
            expired: row.try_get::<i64, _>("expired")?.max(0) as u64,
            bytes: row.try_get::<i64, _>("bytes")?.max(0) as u64,
            dead_tuples: row.try_get::<i64, _>("dead_tuples")?.max(0) as u64,
            last_vacuum: row
                .try_get::<Option<i64>, _>("last_vacuum")?
                .map(from_millis),
        })
    }

    /// Rejects a value that is over the cap, before it reaches the wire.
    fn check_size(&self, key: &Key, value: &[u8]) -> Result<()> {
        if value.len() > self.max_value_bytes {
            return Err(CacheError::TooLarge {
                namespace: key.namespace().as_str().to_owned(),
                key: key.as_str().to_owned(),
                bytes: value.len(),
                limit: self.max_value_bytes,
            });
        }
        Ok(())
    }
}

/// How big the cache is, and whether autovacuum is keeping up with it.
///
/// `dead_tuples` against `last_vacuum` is the pair to watch. This table makes
/// garbage faster than anything else in the database — every overwrite of a
/// hot key leaves a dead row version — and a cache that is quietly losing the
/// vacuum race is a cache that is slowing the rest of your queries down. See
/// the crate documentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheStats {
    /// Rows in the table, expired ones included.
    pub entries: u64,
    /// Rows past their expiry that nothing has swept yet. Not servable; see
    /// [`crate::sweep`].
    pub expired: u64,
    /// Total size on disk, including indexes and TOAST.
    pub bytes: u64,
    /// Dead row versions awaiting vacuum.
    pub dead_tuples: u64,
    /// When the table was last vacuumed, by hand or by autovacuum.
    pub last_vacuum: Option<chrono::DateTime<chrono::Utc>>,
}

#[async_trait]
impl Cache for PgCache {
    fn max_value_bytes(&self) -> usize {
        self.max_value_bytes
    }

    async fn migrate(&self) -> Result<()> {
        // One transaction, behind an advisory lock, because
        // `CREATE TABLE IF NOT EXISTS` is not atomic and a rolling deploy runs
        // this from several processes at once. See `sql::MIGRATION_LOCK`.
        let mut tx = self.pool.begin().await?;
        sqlx::query(sql::MIGRATION_LOCK).execute(&mut *tx).await?;
        for statement in sql::SCHEMA {
            let statement: &'static str = statement;
            sqlx::query(statement).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn get(&self, key: &Key) -> Result<Option<Vec<u8>>> {
        let now = to_millis(self.clock.now());
        let row = sqlx::query(sql::GET)
            .bind(key.namespace().as_str())
            .bind(key.as_str())
            .bind(now)
            .fetch_optional(&self.pool)
            .await?;
        // Two layers of absence collapse into one miss: no row, or a row whose
        // value is NULL because somebody holds a lease on the key.
        match row {
            Some(row) => Ok(row.try_get::<Option<Vec<u8>>, _>(0)?),
            None => Ok(None),
        }
    }

    async fn get_many(&self, keys: &[Key]) -> Result<Vec<Option<Vec<u8>>>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let now = to_millis(self.clock.now());
        let namespaces: Vec<&str> = keys.iter().map(|k| k.namespace().as_str()).collect();
        let names: Vec<&str> = keys.iter().map(|k| k.as_str()).collect();

        let rows = sqlx::query(sql::GET_MANY)
            .bind(&namespaces)
            .bind(&names)
            .bind(now)
            .fetch_all(&self.pool)
            .await?;

        let mut out = vec![None; keys.len()];
        for row in rows {
            // `WITH ORDINALITY` counts from one.
            let at = row.try_get::<i64, _>("i")?;
            let value = row.try_get::<Option<Vec<u8>>, _>("value")?;
            if let Some(slot) = usize::try_from(at - 1).ok().and_then(|i| out.get_mut(i)) {
                *slot = value;
            }
        }
        Ok(out)
    }

    async fn set(&self, key: &Key, value: &[u8], ttl: Duration) -> Result<()> {
        self.check_size(key, value)?;
        let now = to_millis(self.clock.now());
        let until = expires_at(now, ttl)?;
        sqlx::query(sql::SET)
            .bind(key.namespace().as_str())
            .bind(key.as_str())
            .bind(value)
            .bind(until)
            .bind(now)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn add(&self, key: &Key, value: &[u8], ttl: Duration) -> Result<bool> {
        self.check_size(key, value)?;
        let now = to_millis(self.clock.now());
        let until = expires_at(now, ttl)?;
        let stored = sqlx::query(sql::ADD)
            .bind(key.namespace().as_str())
            .bind(key.as_str())
            .bind(value)
            .bind(until)
            .bind(now)
            .fetch_optional(&self.pool)
            .await?;
        Ok(stored.is_some())
    }

    async fn delete(&self, key: &Key) -> Result<bool> {
        let gone = sqlx::query(sql::DELETE)
            .bind(key.namespace().as_str())
            .bind(key.as_str())
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(gone > 0)
    }

    async fn clear(&self, namespace: &Namespace) -> Result<u64> {
        Ok(sqlx::query(sql::CLEAR)
            .bind(namespace.as_str())
            .execute(&self.pool)
            .await?
            .rows_affected())
    }

    async fn lease(&self, key: &Key, ttl: Duration) -> Result<bool> {
        let now = to_millis(self.clock.now());
        let until = expires_at(now, ttl)?;
        let took = sqlx::query(sql::LEASE)
            .bind(key.namespace().as_str())
            .bind(key.as_str())
            .bind(until)
            .bind(now)
            .fetch_optional(&self.pool)
            .await?;
        Ok(took.is_some())
    }

    async fn release_lease(&self, key: &Key) -> Result<()> {
        sqlx::query(sql::RELEASE_LEASE)
            .bind(key.namespace().as_str())
            .bind(key.as_str())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn purge_expired(&self, limit: u64) -> Result<u64> {
        let now = to_millis(self.clock.now());
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        Ok(sqlx::query(sql::PURGE_EXPIRED)
            .bind(now)
            .bind(limit)
            .execute(&self.pool)
            .await?
            .rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_url_for_another_database_says_which_crate_to_look_at() {
        let error = PgCache::connect("sqlite://cache.db").await.unwrap_err();
        let message = error.to_string();
        assert!(message.contains("Postgres only"), "{message}");
        assert!(message.contains("separate crate"), "{message}");
    }
}
