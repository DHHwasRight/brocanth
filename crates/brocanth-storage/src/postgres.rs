//! Objects in Postgres, in `bytea` columns, behind the `postgres` feature.
//!
//! The second backend, and it changes nothing above
//! [`Storage`]: the addresses are the same addresses,
//! `put` still refuses to rewrite the first uploader's metadata, `delete` is
//! still idempotent, and variants are still cached through the trait so the
//! cache follows the objects. What changes is where the bytes live, and the
//! answer is "in the row", which is the point — a deployment that already runs
//! Postgres for its queues and its cache gets blobs with no second piece of
//! infrastructure, no shared filesystem between two application instances, and
//! one thing to back up.
//!
//! # `bytea`, not a large object
//!
//! Postgres offers two ways to hold a blob and they are not close.
//!
//! **A large object** (`pg_largeobject`) streams, seeks, and goes past a
//! gigabyte. All three are worth nothing here. [`Storage`] takes `&[u8]` and
//! answers `Vec<u8>`, so there is no streaming to do and no offset to seek
//! to: the whole value is materialised in memory at both ends before it
//! reaches this crate and after it leaves. What a large object *costs* is
//! severe. It lives in one cluster-wide system table rather than in the
//! objects table, so row-level security, a per-table grant and a `pg_dump` of
//! one schema all stop applying to it. It is referenced by an OID, and
//! deleting the referencing row does **not** delete it: without `lo_unlink`
//! on exactly the right path, or the `lo` extension's trigger, every failed
//! delete leaks storage that only `vacuumlo` will ever find. "Orphaned large
//! objects" is a well-worn operational problem, and the thing this crate must
//! never do is leave a thumbnail of a deleted document behind.
//!
//! **A `bytea`** is an ordinary column. TOAST compresses it and moves it out
//! of line without being asked, it is transactional, `pg_dump` carries it, a
//! `GRANT` on the table covers it, and it dies with its row — which is what
//! makes the `ON DELETE CASCADE` on the variants table a complete answer
//! rather than half of one. Its ceiling is one gigabyte per value.
//!
//! So: `bytea`, and the ceiling is not the reason to pick the other one,
//! because the trait's own signature imposes a lower one.
//!
//! # The ceiling this backend imposes, and why it is not a gigabyte
//!
//! [`DEFAULT_MAX_OBJECT_BYTES`] is thirty-two mebibytes, and
//! [`Postgres::with_max_object_bytes`] refuses anything above
//! [`OBJECT_SIZE_CEILING`], which is two hundred and fifty-six.
//!
//! The hard limit is a gigabyte, and a gigabyte is unreachable in practice. A
//! `put` of *n* bytes holds the caller's slice, the driver's encoded parameter
//! buffer and the server's copy of the value at the same time; a `get` does
//! the same in reverse, and every concurrent request pays it again. At the
//! hard limit one upload is three gigabytes of transient memory and a protocol
//! message at the edge of what the wire format can describe. Thirty-two
//! mebibytes covers an avatar, a logo, a photograph and a contract, which is
//! what a `Vec<u8>`-shaped trait is for; two hundred and fifty-six is where
//! this backend stops pretending, and a store that needs more than that per
//! object wants an object store and a streaming trait, not a bigger number
//! here.
//!
//! The limit is checked before the statement runs, so an oversized object
//! never reaches the server and the refusal is a [`StorageError::TooLarge`]
//! the serving half can turn into a 413. The HTTP half must still enforce its
//! own limit on the request body; this one is the backstop, not the gate.
//!
//! # Concurrency
//!
//! Every write is `INSERT … ON CONFLICT … DO NOTHING`, which is both the
//! dedupe and the defence. Two transactions inserting one address do not race
//! and do not fail: the second waits on the first's speculative insertion
//! lock, sees the conflict when it commits, and does nothing. Because there is
//! no `DO UPDATE`, "does nothing" means the first uploader's media type and
//! filename are untouched — the no-rewrite property falls out of the same
//! clause that gives atomicity. See [`sql`] for the statement itself.
//!
//! # Schema
//!
//! Two tables, a `bytea` primary key and one foreign key with `ON DELETE
//! CASCADE`. The DDL and the argument for every column are in [`sql`].
//!
//! # Example
//!
//! ```no_run
//! use brocanth_storage::{HashKey, ObjectInfo, Postgres, Storage};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let store = Postgres::connect("postgres://localhost/brocanth")
//!     .await?
//!     .with_hash_key(HashKey::from_secret(b"deployment secret"));
//! store.migrate().await?;
//!
//! let key = store.put(b"\x89PNG\r\n\x1a\nbody", &ObjectInfo::new()).await?;
//! assert_eq!(store.meta(&key).await?.content_type.as_str(), "image/png");
//! # Ok(())
//! # }
//! ```

pub mod sql;

use async_trait::async_trait;
use sqlx::{PgPool, Row as _};

use crate::error::StorageError;
use crate::key::{HashKey, ObjectKey};
use crate::meta::{ContentType, Filename, ObjectInfo, ObjectMeta};
use crate::spec::VariantSpec;
use crate::storage::Storage;

/// The largest object a new [`Postgres`] stores: thirty-two mebibytes.
///
/// See the module documentation for why this is not a gigabyte.
pub const DEFAULT_MAX_OBJECT_BYTES: u64 = 32 * 1024 * 1024;

/// The largest value [`Postgres::with_max_object_bytes`] accepts: two hundred
/// and fifty-six mebibytes.
///
/// Postgres would take four times this in one `bytea`. This backend will not,
/// because the trait materialises every object whole at both ends and a
/// gigabyte-shaped `Vec<u8>` is an out-of-memory waiting for a second
/// concurrent request. See the module documentation.
pub const OBJECT_SIZE_CEILING: u64 = 256 * 1024 * 1024;

/// SQLSTATE `23503`, `foreign_key_violation`: a variant whose parent object is
/// not there.
const FOREIGN_KEY_VIOLATION: &str = "23503";

/// The advisory lock [`Postgres::migrate`] holds while it applies the schema.
///
/// `CREATE TABLE IF NOT EXISTS` is **not** safe to run concurrently in
/// Postgres: `IF NOT EXISTS` checks the catalogue and then creates, and two
/// sessions that check at the same moment both proceed and the loser fails on
/// `pg_type_typname_nsp_index` with a duplicate key. Two application instances
/// booting together is the ordinary case, so the lock is not an optimisation.
/// It is held for the transaction and released by the commit.
///
/// The number is the ASCII of `brocstor`, so it is derivable rather than
/// magic. A lock key is cluster-wide and this crate does not own the space;
/// another user of this exact number would serialise with this migration and
/// nothing worse.
const MIGRATION_LOCK: i64 = 0x6272_6f63_7374_6f72;

/// Objects in a Postgres database.
///
/// Cheap to clone in the sense that matters — it holds a [`PgPool`], which is
/// itself a handle — but the expected way to hold one is `Arc<dyn Storage>`,
/// exactly as for [`LocalDisk`](crate::local::LocalDisk).
#[derive(Clone, Debug)]
pub struct Postgres {
    pool: PgPool,
    hash_key: Option<HashKey>,
    max_object_bytes: u64,
}

impl Postgres {
    /// Opens a pool on `url` and wraps it.
    ///
    /// Does **not** create the tables; call [`Postgres::migrate`] for that, in
    /// the one place an application decides to run its migrations.
    ///
    /// Addresses are plain SHA-256 content addresses. For user uploads prefer
    /// [`Postgres::with_hash_key`], which closes the existence oracle
    /// described in [`crate::key`].
    ///
    /// # Errors
    /// [`StorageError::Backend`] when the database cannot be reached.
    pub async fn connect(url: &str) -> Result<Self, StorageError> {
        let pool = PgPool::connect(url)
            .await
            .map_err(|e| StorageError::backend(format!("connect to Postgres: {e}")))?;
        Ok(Self::with_pool(pool))
    }

    /// Wraps a pool the application already has.
    ///
    /// This is the usual constructor: an application running
    /// `brocanth-jobs` and a model layer already holds one pool, and a second
    /// one for blobs is a second set of connections against the same
    /// `max_connections`.
    pub fn with_pool(pool: PgPool) -> Self {
        Self {
            pool,
            hash_key: None,
            max_object_bytes: DEFAULT_MAX_OBJECT_BYTES,
        }
    }

    /// Addresses objects with HMAC-SHA256 under `key` instead of a plain hash.
    ///
    /// Dedupe is unchanged. What changes is that an attacker holding a copy of
    /// a file can no longer compute the address it is stored at, so neither a
    /// guessed URL nor [`Storage::exists`] tells them whether somebody
    /// uploaded it.
    #[must_use]
    pub fn with_hash_key(mut self, key: HashKey) -> Self {
        self.hash_key = Some(key);
        self
    }

    /// Sets the largest object this backend will store.
    ///
    /// Returns a `Result` rather than clamping, because a limit that was
    /// quietly reduced is a 413 nobody configured and cannot find.
    ///
    /// # Errors
    /// [`StorageError::Backend`] when `bytes` is zero or above
    /// [`OBJECT_SIZE_CEILING`].
    pub fn with_max_object_bytes(mut self, bytes: u64) -> Result<Self, StorageError> {
        if bytes == 0 {
            return Err(StorageError::backend(
                "an object size limit of zero stores nothing",
            ));
        }
        if bytes > OBJECT_SIZE_CEILING {
            return Err(StorageError::backend(format!(
                "an object size limit of {bytes} is past this backend's ceiling of \
                 {OBJECT_SIZE_CEILING}; a store that needs more wants an object store \
                 and a streaming trait"
            )));
        }
        self.max_object_bytes = bytes;
        Ok(self)
    }

    /// The pool, for an application that wants to run its own statement
    /// against the same connections.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// The largest object this backend stores.
    pub fn max_object_bytes(&self) -> u64 {
        self.max_object_bytes
    }

    /// Creates the two tables if they are not there.
    ///
    /// Safe to run on every boot and from several instances at once. `IF NOT
    /// EXISTS` is what makes the second run a no-op; the advisory lock is what
    /// makes two *simultaneous* first runs safe, because `IF NOT EXISTS`
    /// checks and then creates and the gap between them is a real race. See
    /// the module source for the lock key, and [`sql::SCHEMA`] for the DDL.
    ///
    /// # Errors
    /// [`StorageError::Backend`] when a statement fails.
    pub async fn migrate(&self) -> Result<(), StorageError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| backend("begin the migration", e))?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(MIGRATION_LOCK)
            .execute(&mut *tx)
            .await
            .map_err(|e| backend("take the migration lock", e))?;
        for statement in sql::SCHEMA {
            sqlx::query(*statement)
                .execute(&mut *tx)
                .await
                .map_err(|e| backend("create the storage tables", e))?;
        }
        // Postgres DDL is transactional, so either both tables and the foreign
        // key between them exist or neither does. The lock goes with the
        // commit.
        tx.commit()
            .await
            .map_err(|e| backend("commit the migration", e))
    }

    /// The address these bytes get in this store, without storing them.
    pub fn address_of(&self, content: &[u8]) -> ObjectKey {
        match &self.hash_key {
            Some(key) => ObjectKey::keyed(key, content),
            None => ObjectKey::of(content),
        }
    }

    /// Refuses an object the backend will not hold, before anything is sent.
    fn check_size(&self, content: &[u8]) -> Result<(), StorageError> {
        let size = content.len() as u64;
        if size > self.max_object_bytes {
            return Err(StorageError::TooLarge {
                size,
                limit: self.max_object_bytes,
            });
        }
        Ok(())
    }
}

#[async_trait]
impl Storage for Postgres {
    async fn put(&self, content: &[u8], about: &ObjectInfo) -> Result<ObjectKey, StorageError> {
        let key = self.address_of(content);
        self.put_at(&key, content, about).await?;
        Ok(key)
    }

    async fn put_at(
        &self,
        key: &ObjectKey,
        content: &[u8],
        about: &ObjectInfo,
    ) -> Result<(), StorageError> {
        self.check_size(content)?;
        let content_type = resolve_type(content, about);
        // No `SELECT` first. The conflict clause is the existence check, it is
        // atomic where a check-then-insert is not, and it leaves the first
        // uploader's row alone — which is the whole of the no-rewrite
        // property. See `sql::INSERT_OBJECT`.
        sqlx::query(sql::INSERT_OBJECT)
            .bind(&key.as_bytes()[..])
            .bind(content)
            .bind(content.len() as i64)
            .bind(content_type.as_str())
            .bind(about.filename.as_ref().map(Filename::as_str))
            .execute(&self.pool)
            .await
            .map_err(|e| backend("store an object", e))?;
        // Zero rows affected means it was already there, which is a successful
        // deduplicated put rather than a failure.
        Ok(())
    }

    async fn get(&self, key: &ObjectKey) -> Result<Vec<u8>, StorageError> {
        let row = sqlx::query(sql::SELECT_CONTENT)
            .bind(&key.as_bytes()[..])
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| backend("read an object", e))?
            .ok_or(StorageError::NotFound)?;
        row.try_get::<Vec<u8>, _>(0)
            .map_err(|e| backend("decode an object", e))
    }

    async fn meta(&self, key: &ObjectKey) -> Result<ObjectMeta, StorageError> {
        let row = sqlx::query(sql::SELECT_META)
            .bind(&key.as_bytes()[..])
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| backend("read an object's metadata", e))?
            .ok_or(StorageError::NotFound)?;
        let size: i64 = row
            .try_get(0)
            .map_err(|e| backend("decode an object's size", e))?;
        let content_type: String = row
            .try_get(1)
            .map_err(|e| backend("decode an object's media type", e))?;
        let filename: Option<String> = row
            .try_get(2)
            .map_err(|e| backend("decode an object's filename", e))?;
        Ok(ObjectMeta {
            key: *key,
            // The column is a `BIGINT` because Postgres has no unsigned
            // integer, and a negative size is a row this crate did not write.
            // `application/octet-stream` and a zero are always safe answers;
            // refusing to serve the bytes over a bad metadata column is not.
            size: size.max(0) as u64,
            content_type: ContentType::parse(&content_type)
                .unwrap_or_else(ContentType::octet_stream),
            filename: filename.as_deref().and_then(Filename::sanitise),
        })
    }

    async fn exists(&self, key: &ObjectKey) -> Result<bool, StorageError> {
        let row = sqlx::query(sql::SELECT_EXISTS)
            .bind(&key.as_bytes()[..])
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| backend("look for an object", e))?;
        Ok(row.is_some())
    }

    async fn delete(&self, key: &ObjectKey) -> Result<(), StorageError> {
        // One statement: the foreign key's cascade removes the variants, in
        // this transaction, so there is no moment at which a thumbnail of a
        // deleted object is readable. Deleting what is not there affects no
        // rows and succeeds.
        sqlx::query(sql::DELETE_OBJECT)
            .bind(&key.as_bytes()[..])
            .execute(&self.pool)
            .await
            .map_err(|e| backend("remove an object", e))?;
        Ok(())
    }

    async fn get_variant(
        &self,
        key: &ObjectKey,
        spec: VariantSpec,
    ) -> Result<Option<Vec<u8>>, StorageError> {
        let row = sqlx::query(sql::SELECT_VARIANT)
            .bind(&key.as_bytes()[..])
            .bind(spec.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| backend("read a variant", e))?;
        match row {
            // A miss is ordinary control flow, not an error.
            None => Ok(None),
            Some(row) => Ok(Some(
                row.try_get::<Vec<u8>, _>(0)
                    .map_err(|e| backend("decode a variant", e))?,
            )),
        }
    }

    async fn put_variant(
        &self,
        key: &ObjectKey,
        spec: VariantSpec,
        content: &[u8],
    ) -> Result<(), StorageError> {
        self.check_size(content)?;
        let result = sqlx::query(sql::INSERT_VARIANT)
            .bind(&key.as_bytes()[..])
            .bind(spec.to_string())
            .bind(content)
            .execute(&self.pool)
            .await;
        match result {
            Ok(_) => Ok(()),
            // The foreign key refused a variant of an object that is not
            // stored. `LocalDisk` would write the orphan and keep it until
            // somebody noticed; here the database will not have one, so the
            // honest answer is that the parent is missing. The message never
            // carries the address: a Postgres constraint violation names the
            // value in its `DETAIL`, which is a line nobody wants in a log
            // aggregator.
            Err(e) if is_missing_parent(&e) => Err(StorageError::NotFound),
            Err(e) => Err(backend("store a variant", e)),
        }
    }
}

/// The media type to store: what the bytes are, then what the client said,
/// then nothing. The same order [`LocalDisk`](crate::local::LocalDisk) uses,
/// and for the same reason — see
/// [`Storage::put`](crate::storage::Storage::put).
fn resolve_type(content: &[u8], about: &ObjectInfo) -> ContentType {
    ContentType::sniff(content)
        .or_else(|| about.declared_type.clone())
        .unwrap_or_else(ContentType::octet_stream)
}

/// Names the operation, never the address. A key in an error message is a key
/// in a log aggregator, and under plain content addressing that is a file
/// somebody uploaded.
fn backend(context: &'static str, e: sqlx::Error) -> StorageError {
    StorageError::Backend(format!("{context}: {e}"))
}

fn is_missing_parent(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.code().as_deref() == Some(FOREIGN_KEY_VIOLATION))
}
