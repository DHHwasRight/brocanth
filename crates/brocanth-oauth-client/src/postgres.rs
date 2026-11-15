//! [`PgTokenStore`]: connections in the database the `db` module opened.
//!
//! One table, keyed by `(account, provider)`. A `put` is one
//! `INSERT … ON CONFLICT DO UPDATE`, so two writers of one key leave one
//! whole row. The token columns are `bytea` and hold sealed bytes only.

use async_trait::async_trait;
use brocanth_db::sqlx::{self, Row};
use brocanth_db::Db;

use crate::store::{Connection, StoreError, StoredToken, TokenStore};

/// Connections in Postgres.
#[derive(Clone, Debug)]
pub struct PgTokenStore {
    db: Db,
}

impl PgTokenStore {
    /// The table, idempotent.
    pub const SCHEMA: &'static str = "\
CREATE TABLE IF NOT EXISTS brocanth_oauth_tokens (
    account      text   NOT NULL,
    provider     text   NOT NULL,
    access       bytea  NOT NULL,
    refresh      bytea,
    token_type   text   NOT NULL,
    scopes       text   NOT NULL,
    expires_at   bigint,
    connected_at bigint NOT NULL,
    refreshed_at bigint NOT NULL,
    PRIMARY KEY (account, provider)
)";

    /// A store over `db`. Creates nothing: see [`PgTokenStore::migrate`].
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Runs [`PgTokenStore::SCHEMA`], under a transaction-level advisory
    /// lock.
    ///
    /// `CREATE TABLE IF NOT EXISTS` is not atomic: two replicas that start
    /// together on a fresh database can both pass the check, and the second
    /// fails on the system catalogue's unique index. The lock makes the
    /// second wait, as `brocanth-auth`'s `PgStore::migrate` does.
    ///
    /// # Errors
    /// The database refused the statement.
    pub async fn migrate(&self) -> Result<(), StoreError> {
        let mut tx = self.db.pool().begin().await.map_err(failed)?;
        sqlx::query(MIGRATION_LOCK)
            .execute(&mut *tx)
            .await
            .map_err(failed)?;
        sqlx::query(Self::SCHEMA)
            .execute(&mut *tx)
            .await
            .map_err(failed)?;
        tx.commit().await.map_err(failed)
    }
}

/// One key for this crate's tables, apart from `brocanth-auth`'s.
const MIGRATION_LOCK: &str = "SELECT pg_advisory_xact_lock(7093854835689485417)";

fn failed(e: sqlx::Error) -> StoreError {
    StoreError(e.to_string())
}

const COLUMNS: &str =
    "account, provider, access, refresh, token_type, scopes, expires_at, connected_at, refreshed_at";

const LISTED: &str =
    "account, provider, refresh IS NOT NULL AS has_refresh, scopes, expires_at, connected_at, refreshed_at";

fn scopes(text: &str) -> Vec<String> {
    text.split(' ')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn connection(row: &sqlx::postgres::PgRow) -> Result<Connection, sqlx::Error> {
    Ok(Connection {
        account: row.try_get("account")?,
        provider: row.try_get("provider")?,
        scopes: scopes(row.try_get::<&str, _>("scopes")?),
        expires_at: row.try_get("expires_at")?,
        has_refresh: row.try_get("has_refresh")?,
        connected_at: row.try_get("connected_at")?,
        refreshed_at: row.try_get("refreshed_at")?,
    })
}

#[async_trait]
impl TokenStore for PgTokenStore {
    async fn get(&self, account: &str, provider: &str) -> Result<Option<StoredToken>, StoreError> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM brocanth_oauth_tokens WHERE account = $1 AND provider = $2"
        )))
        .bind(account)
        .bind(provider)
        .fetch_optional(self.db.pool())
        .await
        .map_err(failed)?;
        let Some(row) = row else { return Ok(None) };
        let token = (|| -> Result<StoredToken, sqlx::Error> {
            Ok(StoredToken {
                account: row.try_get("account")?,
                provider: row.try_get("provider")?,
                access: row.try_get("access")?,
                refresh: row.try_get("refresh")?,
                token_type: row.try_get("token_type")?,
                scopes: scopes(row.try_get::<&str, _>("scopes")?),
                expires_at: row.try_get("expires_at")?,
                connected_at: row.try_get("connected_at")?,
                refreshed_at: row.try_get("refreshed_at")?,
            })
        })()
        .map_err(failed)?;
        Ok(Some(token))
    }

    async fn put(&self, token: StoredToken) -> Result<(), StoreError> {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO brocanth_oauth_tokens ({COLUMNS}) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
             ON CONFLICT (account, provider) DO UPDATE SET \
             access = EXCLUDED.access, refresh = EXCLUDED.refresh, \
             token_type = EXCLUDED.token_type, scopes = EXCLUDED.scopes, \
             expires_at = EXCLUDED.expires_at, connected_at = EXCLUDED.connected_at, \
             refreshed_at = EXCLUDED.refreshed_at"
        )))
        .bind(&token.account)
        .bind(&token.provider)
        .bind(&token.access)
        .bind(&token.refresh)
        .bind(&token.token_type)
        .bind(token.scopes.join(" "))
        .bind(token.expires_at)
        .bind(token.connected_at)
        .bind(token.refreshed_at)
        .execute(self.db.pool())
        .await
        .map(drop)
        .map_err(failed)
    }

    async fn delete(&self, account: &str, provider: &str) -> Result<bool, StoreError> {
        let done =
            sqlx::query("DELETE FROM brocanth_oauth_tokens WHERE account = $1 AND provider = $2")
                .bind(account)
                .bind(provider)
                .execute(self.db.pool())
                .await
                .map_err(failed)?;
        Ok(done.rows_affected() > 0)
    }

    async fn for_account(&self, account: &str) -> Result<Vec<Connection>, StoreError> {
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {LISTED} FROM brocanth_oauth_tokens WHERE account = $1 ORDER BY provider"
        )))
        .bind(account)
        .fetch_all(self.db.pool())
        .await
        .map_err(failed)?;
        rows.iter()
            .map(connection)
            .collect::<Result<_, _>>()
            .map_err(failed)
    }

    async fn list(&self, limit: u32, offset: u32) -> Result<Vec<Connection>, StoreError> {
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {LISTED} FROM brocanth_oauth_tokens ORDER BY account, provider LIMIT $1 OFFSET $2"
        )))
        .bind(i64::from(limit))
        .bind(i64::from(offset))
        .fetch_all(self.db.pool())
        .await
        .map_err(failed)?;
        rows.iter()
            .map(connection)
            .collect::<Result<_, _>>()
            .map_err(failed)
    }

    async fn count(&self) -> Result<u64, StoreError> {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM brocanth_oauth_tokens")
            .fetch_one(self.db.pool())
            .await
            .map_err(failed)?;
        Ok(u64::try_from(n).unwrap_or(0))
    }
}
