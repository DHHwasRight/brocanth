//! [`PgStore`]: the history in one table of the application's own
//! database.
//!
//! Plain SQL through the pool the `db` module opened. A flush is one
//! transaction. The columns an entry touched are three text arrays of one
//! length, so the crate needs no JSON library. Every value is a bound
//! parameter.

use async_trait::async_trait;
use brocanth_db::sqlx::{self, AssertSqlSafe};
use brocanth_db::Db;

use crate::entry::{Entry, Field, Kind};
use crate::store::{AuditStore, Query, StoreError};

/// The history in Postgres.
#[derive(Clone, Debug)]
pub struct PgStore {
    db: Db,
}

/// The advisory lock key of the migration and of the purge, so one replica
/// runs each at a time: a hash of the table's schema and name. An advisory
/// lock is database-wide, and two schemas each hold a table of their own.
const LOCK: &str = "hashtextextended(current_schema() || '.brocanth_audit', 0)";

/// Rows one purge statement deletes. A purge loops until a statement
/// deletes fewer, so no statement holds a long lock on the table.
const PURGE_BATCH: i64 = 10_000;

/// Each query shape, with a fixed `WHERE`, so a generic plan of a prepared
/// statement still uses the `(table_name, row_key, id)` index.
const SELECT: &str = "SELECT id, at, actor, source, table_name, row_key, label, kind, \
                      field_names, field_before, field_after FROM brocanth_audit";

impl PgStore {
    /// The table and its two indexes, idempotent.
    ///
    /// `at` is seconds since 1970, a `bigint`, so the crate needs no date
    /// library. `field_before` and `field_after` hold NULL for a value that
    /// is NULL or unknown.
    pub const SCHEMA: &'static str = "\
CREATE TABLE IF NOT EXISTS brocanth_audit (
    id           bigserial PRIMARY KEY,
    at           bigint    NOT NULL,
    actor        text,
    source       text      NOT NULL,
    table_name   text      NOT NULL,
    row_key      text      NOT NULL,
    label        text      NOT NULL DEFAULT '',
    kind         text      NOT NULL,
    field_names  text[]    NOT NULL DEFAULT '{}',
    field_before text[]    NOT NULL DEFAULT '{}',
    field_after  text[]    NOT NULL DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS brocanth_audit_row
    ON brocanth_audit (table_name, row_key, id);
CREATE INDEX IF NOT EXISTS brocanth_audit_at
    ON brocanth_audit (at);
";

    /// A store over `db`. Creates nothing: see [`PgStore::migrate`].
    pub fn new(db: Db) -> PgStore {
        PgStore { db }
    }

    /// Runs [`PgStore::SCHEMA`], in one transaction under an advisory
    /// lock: replicas that start together run it one after the other, and
    /// the second finds the table made. Without the lock two concurrent
    /// `CREATE TABLE IF NOT EXISTS` can both try to create the row type.
    ///
    /// # Errors
    /// The database refused a statement.
    pub async fn migrate(&self) -> Result<(), StoreError> {
        let mut tx = self.db.pool().begin().await.map_err(failed)?;
        sqlx::query(AssertSqlSafe(format!(
            "SELECT pg_advisory_xact_lock({LOCK})"
        )))
        .execute(&mut *tx)
        .await
        .map_err(failed)?;
        for statement in Self::SCHEMA
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            sqlx::query(AssertSqlSafe(statement.to_string()))
                .execute(&mut *tx)
                .await
                .map_err(failed)?;
        }
        tx.commit().await.map_err(failed)
    }
}

fn failed(e: sqlx::Error) -> StoreError {
    StoreError::new(format!("audit store: {e}"))
}

type Row = (
    i64,
    i64,
    Option<String>,
    String,
    String,
    String,
    String,
    String,
    Vec<String>,
    Vec<Option<String>>,
    Vec<Option<String>>,
);

#[async_trait]
impl AuditStore for PgStore {
    async fn append(&self, entries: &[Entry]) -> Result<(), StoreError> {
        if entries.is_empty() {
            return Ok(());
        }
        // One statement per entry, in one transaction: the field arrays
        // are per entry, and UNNEST cannot unpack an array of arrays of
        // different lengths.
        let mut tx = self.db.pool().begin().await.map_err(failed)?;
        for e in entries {
            let names: Vec<&str> = e.fields.iter().map(|f| f.name.as_str()).collect();
            let before: Vec<Option<&str>> = e.fields.iter().map(|f| f.before.as_deref()).collect();
            let after: Vec<Option<&str>> = e.fields.iter().map(|f| f.after.as_deref()).collect();
            sqlx::query(
                "INSERT INTO brocanth_audit (at, actor, source, table_name, row_key, label, \
                 kind, field_names, field_before, field_after) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
            )
            .bind(e.at)
            .bind(e.actor.as_deref())
            .bind(&e.source)
            .bind(&e.table)
            .bind(&e.row)
            .bind(&e.label)
            .bind(e.kind.as_str())
            .bind(&names)
            .bind(&before)
            .bind(&after)
            .execute(&mut *tx)
            .await
            .map_err(failed)?;
        }
        tx.commit().await.map_err(failed)
    }

    async fn find(&self, query: &Query) -> Result<Vec<Entry>, StoreError> {
        // `id < before` with no cursor is `id < i64::MAX`: every row, and
        // the same statement text for the first page and the next.
        let before = query.before.unwrap_or(i64::MAX);
        let limit = i64::try_from(query.capped()).unwrap_or(50);
        let sql = match (&query.table, &query.row) {
            (Some(_), Some(_)) => format!(
                "{SELECT} WHERE table_name = $1 AND row_key = $2 AND id < $3 \
                 ORDER BY id DESC LIMIT $4"
            ),
            (Some(_), None) => {
                format!("{SELECT} WHERE table_name = $1 AND id < $2 ORDER BY id DESC LIMIT $3")
            }
            (None, _) => format!("{SELECT} WHERE id < $1 ORDER BY id DESC LIMIT $2"),
        };
        let mut select = sqlx::query_as::<_, Row>(AssertSqlSafe(sql));
        if let Some(table) = &query.table {
            select = select.bind(table);
            if let Some(row) = &query.row {
                select = select.bind(row);
            }
        }
        let rows = select
            .bind(before)
            .bind(limit)
            .fetch_all(self.db.pool())
            .await
            .map_err(failed)?;
        Ok(rows.into_iter().map(entry).collect())
    }

    /// Deletes in batches of [`PURGE_BATCH`], under an advisory lock that
    /// one replica holds: another replica that purges at the same time
    /// skips, and deletes nothing.
    async fn purge(&self, before: i64) -> Result<u64, StoreError> {
        let mut conn = self.db.pool().acquire().await.map_err(failed)?;
        let (locked,): (bool,) = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT pg_try_advisory_lock({LOCK})"
        )))
        .fetch_one(&mut *conn)
        .await
        .map_err(failed)?;
        if !locked {
            return Ok(0);
        }
        let mut removed = 0;
        let outcome = loop {
            let done = sqlx::query(
                "DELETE FROM brocanth_audit WHERE id IN \
                 (SELECT id FROM brocanth_audit WHERE at < $1 ORDER BY id LIMIT $2)",
            )
            .bind(before)
            .bind(PURGE_BATCH)
            .execute(&mut *conn)
            .await;
            match done {
                Ok(done) => {
                    removed += done.rows_affected();
                    if done.rows_affected() < PURGE_BATCH as u64 {
                        break Ok(removed);
                    }
                }
                Err(e) => break Err(failed(e)),
            }
        };
        let unlocked = sqlx::query(AssertSqlSafe(format!("SELECT pg_advisory_unlock({LOCK})")))
            .execute(&mut *conn)
            .await;
        if unlocked.is_err() {
            // The lock is held by this session: close it rather than give
            // a locked connection back to the pool.
            conn.detach();
        }
        outcome
    }
}

fn entry(row: Row) -> Entry {
    let (id, at, actor, source, table, row_key, label, kind, names, before, after) = row;
    let fields = names
        .into_iter()
        .enumerate()
        .map(|(i, name)| Field {
            name,
            before: before.get(i).cloned().flatten(),
            after: after.get(i).cloned().flatten(),
        })
        .collect();
    Entry {
        id,
        at,
        actor,
        source,
        table,
        row: row_key,
        label,
        kind: Kind::parse(&kind),
        fields,
    }
}
