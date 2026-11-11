//! [`PgStore`]: the rules in one table of the application's own database.
//!
//! Plain SQL through the pool the `db` module opened. Every value is a bound
//! parameter.

use std::collections::BTreeMap;

use async_trait::async_trait;
use brocanth_db::sqlx::{self, AssertSqlSafe};
use brocanth_db::Db;

use crate::rule::Rule;
use crate::store::{FlagStore, StoreError};

/// The rules in Postgres.
#[derive(Clone, Debug)]
pub struct PgStore {
    db: Db,
}

impl PgStore {
    /// The table, idempotent. One row per flag that somebody set; a flag
    /// with no row is off.
    pub const SCHEMA: &'static str = "\
CREATE TABLE IF NOT EXISTS brocanth_flags (
    key         text        PRIMARY KEY,
    on_for_all  boolean     NOT NULL DEFAULT false,
    percent     smallint    NOT NULL DEFAULT 0 CHECK (percent BETWEEN 0 AND 100),
    user_ids    text[]      NOT NULL DEFAULT '{}',
    group_names text[]      NOT NULL DEFAULT '{}',
    updated_at  timestamptz NOT NULL DEFAULT now()
)";

    /// A store over `db`. Creates nothing: see [`PgStore::migrate`].
    pub fn new(db: Db) -> PgStore {
        PgStore { db }
    }

    /// Runs [`PgStore::SCHEMA`].
    ///
    /// # Errors
    /// The database refused the statement.
    pub async fn migrate(&self) -> Result<(), StoreError> {
        sqlx::query(AssertSqlSafe(Self::SCHEMA.to_string()))
            .execute(self.db.pool())
            .await
            .map(drop)
            .map_err(failed)
    }
}

fn failed(e: sqlx::Error) -> StoreError {
    StoreError::new(format!("flags store: {e}"))
}

type Row = (String, bool, i16, Vec<String>, Vec<String>);

#[async_trait]
impl FlagStore for PgStore {
    async fn load(&self) -> Result<BTreeMap<String, Rule>, StoreError> {
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT key, on_for_all, percent, user_ids, group_names FROM brocanth_flags",
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(failed)?;
        Ok(rows
            .into_iter()
            .map(|(key, on, percent, actors, groups)| {
                let rule = Rule {
                    on,
                    percent: u8::try_from(percent.clamp(0, 100)).unwrap_or(0),
                    actors,
                    groups,
                };
                (key, rule)
            })
            .collect())
    }

    async fn save(&self, key: &str, rule: &Rule) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO brocanth_flags (key, on_for_all, percent, user_ids, group_names, updated_at) \
             VALUES ($1, $2, $3, $4, $5, now()) \
             ON CONFLICT (key) DO UPDATE SET on_for_all = EXCLUDED.on_for_all, \
             percent = EXCLUDED.percent, user_ids = EXCLUDED.user_ids, \
             group_names = EXCLUDED.group_names, updated_at = now()",
        )
        .bind(key)
        .bind(rule.on)
        .bind(i16::from(rule.percent.min(100)))
        .bind(&rule.actors)
        .bind(&rule.groups)
        .execute(self.db.pool())
        .await
        .map(drop)
        .map_err(failed)
    }

    async fn remove(&self, key: &str) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM brocanth_flags WHERE key = $1")
            .bind(key)
            .execute(self.db.pool())
            .await
            .map(drop)
            .map_err(failed)
    }
}
