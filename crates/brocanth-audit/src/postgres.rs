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

    /// Runs [`PgStore::SCHEMA`].
    ///
    /// # Errors
    /// The database refused a statement.
    pub async fn migrate(&self) -> Result<(), StoreError> {
        for statement in Self::SCHEMA
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            sqlx::query(AssertSqlSafe(statement.to_string()))
                .execute(self.db.pool())
                .await
                .map_err(failed)?;
        }
        Ok(())
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
        let row = query.table.as_ref().and(query.row.as_deref());
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT id, at, actor, source, table_name, row_key, label, kind, \
             field_names, field_before, field_after FROM brocanth_audit \
             WHERE ($1::text IS NULL OR table_name = $1) \
             AND ($2::text IS NULL OR row_key = $2) \
             AND ($3::bigint IS NULL OR id < $3) \
             ORDER BY id DESC LIMIT $4",
        )
        .bind(query.table.as_deref())
        .bind(row)
        .bind(query.before)
        .bind(i64::try_from(query.capped()).unwrap_or(50))
        .fetch_all(self.db.pool())
        .await
        .map_err(failed)?;
        Ok(rows.into_iter().map(entry).collect())
    }

    async fn purge(&self, before: i64) -> Result<u64, StoreError> {
        sqlx::query("DELETE FROM brocanth_audit WHERE at < $1")
            .bind(before)
            .execute(self.db.pool())
            .await
            .map(|done| done.rows_affected())
            .map_err(failed)
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
