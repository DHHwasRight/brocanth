//! [`NotificationStore`] and [`Inbox`]: the table, and one account's view
//! of it.
//!
//! Plain SQL through the pool the `db` module opened. Every value is a bound
//! parameter, and every statement of an [`Inbox`] names its account.

use brocanth_db::sqlx::postgres::{PgArguments, PgRow};
use brocanth_db::sqlx::query::QueryAs;
use brocanth_db::sqlx::{self, AssertSqlSafe, FromRow, Postgres, Row as _};
use brocanth_db::Db;
use brocanth_policy::{Condition, Value};

use crate::{safe_url, Notice};

/// The table.
pub const TABLE: &str = "brocanth_notifications";

/// The table and its index, idempotent. Run by the plugin's first readiness
/// check with `migrate`, or put it in a migration.
///
/// One partial index serves the unread count and the unread page: the
/// count of a busy account reads its unread rows and no others.
pub const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS brocanth_notifications (
    id          bigserial   PRIMARY KEY,
    account     text        NOT NULL,
    kind        text        NOT NULL,
    title       text        NOT NULL,
    body        text        NOT NULL DEFAULT '',
    url         text,
    created_at  timestamptz NOT NULL DEFAULT now(),
    read_at     timestamptz
);
CREATE INDEX IF NOT EXISTS brocanth_notifications_account
    ON brocanth_notifications (account, id DESC);
CREATE INDEX IF NOT EXISTS brocanth_notifications_unread
    ON brocanth_notifications (account, id DESC) WHERE read_at IS NULL";

/// Why a notification was not stored or read.
#[derive(Debug, thiserror::Error)]
pub enum NotifyError {
    /// The link is not a path on this site or an http(s) URL.
    #[error("the link {0:?} is not a path on this site or an http(s) URL\nhelp: pass `/path` or `https://…` to `Notice::url`")]
    UnsafeUrl(String),
    /// The title is empty, so the inbox would show a blank line.
    #[error("the notice has no title\nhelp: pass a title to `Notice::new`")]
    NoTitle,
    /// The database refused the statement.
    #[error("[notifications] {0}")]
    Database(#[from] sqlx::Error),
}

/// One stored notification, as an inbox reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// Its id: what a mark-read form sends.
    pub id: i64,
    /// The kind.
    pub kind: String,
    /// The title.
    pub title: String,
    /// The body, empty for none.
    pub body: String,
    /// The link, already checked by [`safe_url`] when it was stored.
    pub url: Option<String>,
    /// When it was stored, in seconds since 1970-01-01 UTC.
    pub created_at: i64,
    /// Whether the account has read it.
    pub read: bool,
}

impl<'r> FromRow<'r, PgRow> for Item {
    fn from_row(row: &'r PgRow) -> Result<Self, sqlx::Error> {
        Ok(Item {
            id: row.try_get("id")?,
            kind: row.try_get("kind")?,
            title: row.try_get("title")?,
            body: row.try_get("body")?,
            url: row.try_get("url")?,
            created_at: row.try_get("created")?,
            read: row.try_get("read")?,
        })
    }
}

/// The notifications of every account. Cheap to clone: it holds the pool
/// and the settings.
#[derive(Clone, Debug)]
pub struct NotificationStore {
    db: Db,
    config: std::sync::Arc<crate::NotificationsConfig>,
}

impl NotificationStore {
    /// A store over `db`, with the default settings. Creates nothing: see
    /// [`NotificationStore::migrate`].
    pub fn new(db: Db) -> Self {
        Self::configured(db, crate::NotificationsConfig::default())
    }

    /// A store over `db` with these settings, as the plugin builds it.
    pub fn configured(db: Db, config: crate::NotificationsConfig) -> Self {
        Self {
            db,
            config: std::sync::Arc::new(config),
        }
    }

    /// The settings the page reads: `per_page`, `mail`.
    pub fn config(&self) -> &crate::NotificationsConfig {
        &self.config
    }

    /// Runs [`SCHEMA`] under a transaction-scoped advisory lock, so two
    /// replicas that boot together take turns.
    ///
    /// # Errors
    /// The database refused a statement.
    pub async fn migrate(&self) -> Result<(), NotifyError> {
        let mut tx = self.db.pool().begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('brocanth_notifications'))")
            .execute(&mut *tx)
            .await?;
        for statement in SCHEMA.split(';') {
            sqlx::query(AssertSqlSafe(statement.to_string()))
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Stores a notification for `account` and answers its id.
    ///
    /// # Errors
    /// [`NotifyError::UnsafeUrl`] for a link [`safe_url`] refuses,
    /// [`NotifyError::NoTitle`] for an empty title, and what the database
    /// raised.
    pub async fn notify(&self, account: &str, notice: &Notice) -> Result<i64, NotifyError> {
        if notice.title.trim().is_empty() {
            return Err(NotifyError::NoTitle);
        }
        if let Some(url) = notice.url.as_deref().filter(|url| !safe_url(url)) {
            return Err(NotifyError::UnsafeUrl(url.to_string()));
        }
        let id = sqlx::query_scalar(
            "INSERT INTO brocanth_notifications (account, kind, title, body, url) \
             VALUES ($1, $2, $3, $4, $5) RETURNING id",
        )
        .bind(account)
        .bind(&notice.kind)
        .bind(&notice.title)
        .bind(&notice.body)
        .bind(&notice.url)
        .fetch_one(self.db.pool())
        .await?;
        Ok(id)
    }

    /// The inbox of one account. Every read and write through it names that
    /// account.
    pub fn inbox<'s>(&'s self, account: &str) -> Inbox<'s> {
        Inbox {
            db: &self.db,
            account: account.to_string(),
        }
    }
}

/// One account's notifications. Made by [`NotificationStore::inbox`].
#[derive(Debug)]
pub struct Inbox<'s> {
    db: &'s Db,
    account: String,
}

impl Inbox<'_> {
    /// The account this inbox reads.
    pub fn account(&self) -> &str {
        &self.account
    }

    /// How many notifications the account has not read.
    ///
    /// # Errors
    /// What the database raised.
    pub async fn unread(&self) -> Result<i64, NotifyError> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM brocanth_notifications WHERE account = $1 AND read_at IS NULL",
        )
        .bind(&self.account)
        .fetch_one(self.db.pool())
        .await?)
    }

    /// Up to `limit` notifications, newest first, with an id below `before`
    /// when it is given: the next page starts below the last id of this one.
    ///
    /// `rule` is a policy condition over
    /// [`NotificationColumn`](crate::NotificationColumn), ANDed into the same `WHERE` as the
    /// account. It can narrow the list; it cannot widen it to another
    /// account.
    ///
    /// # Errors
    /// What the database raised.
    pub async fn list(
        &self,
        before: Option<i64>,
        limit: u32,
        rule: Option<&Condition>,
    ) -> Result<Vec<Item>, NotifyError> {
        let mut binds: Vec<Value> = rule.map(|c| c.binds().to_vec()).unwrap_or_default();
        let narrowed = rule.map_or_else(|| "TRUE".to_string(), |c| c.sql().to_string());
        binds.push(Value::Text(self.account.clone()));
        let account = binds.len();
        binds.push(Value::Int(before.unwrap_or(i64::MAX)));
        let below = binds.len();
        let sql = format!(
            "SELECT id, kind, title, body, url, \
             EXTRACT(EPOCH FROM created_at)::bigint AS created, read_at IS NOT NULL AS read \
             FROM brocanth_notifications \
             WHERE ({narrowed}) AND account = ${account} AND id < ${below} \
             ORDER BY id DESC LIMIT {}",
            limit.clamp(1, 500)
        );
        let mut query: QueryAs<'_, Postgres, Item, PgArguments> =
            sqlx::query_as(AssertSqlSafe(sql));
        for value in &binds {
            query = bind(query, value);
        }
        Ok(query.fetch_all(self.db.pool()).await?)
    }

    /// Marks one notification read. `false` when the account has no
    /// notification with that id, which is also the answer for another
    /// account's id.
    ///
    /// # Errors
    /// What the database raised.
    pub async fn mark_read(&self, id: i64) -> Result<bool, NotifyError> {
        let done = sqlx::query(
            "UPDATE brocanth_notifications SET read_at = now() \
             WHERE id = $1 AND account = $2 AND read_at IS NULL",
        )
        .bind(id)
        .bind(&self.account)
        .execute(self.db.pool())
        .await?;
        if done.rows_affected() == 1 {
            return Ok(true);
        }
        // Already read is still "the account has it".
        let held: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM brocanth_notifications WHERE id = $1 AND account = $2)",
        )
        .bind(id)
        .bind(&self.account)
        .fetch_one(self.db.pool())
        .await?;
        Ok(held)
    }

    /// Marks every unread notification of the account read, and answers how
    /// many it marked.
    ///
    /// # Errors
    /// What the database raised.
    pub async fn mark_all_read(&self) -> Result<u64, NotifyError> {
        let done = sqlx::query(
            "UPDATE brocanth_notifications SET read_at = now() \
             WHERE account = $1 AND read_at IS NULL",
        )
        .bind(&self.account)
        .execute(self.db.pool())
        .await?;
        Ok(done.rows_affected())
    }
}

fn bind<'q>(
    query: QueryAs<'q, Postgres, Item, PgArguments>,
    value: &Value,
) -> QueryAs<'q, Postgres, Item, PgArguments> {
    match value {
        Value::Null => query.bind(None::<String>),
        Value::Bool(b) => query.bind(*b),
        Value::Int(n) => query.bind(*n),
        Value::Float(f) => query.bind(*f),
        Value::Text(t) => query.bind(t.clone()),
        Value::Bytes(b) => query.bind(b.clone()),
        Value::Uuid(b) => query.bind(sqlx::types::Uuid::from_bytes(*b)),
        // Outside chrono's range (only a hand-built `Value` is), the
        // timestamp binds as NULL.
        Value::Timestamptz(micros) => query.bind(sqlx::types::chrono::DateTime::<
            sqlx::types::chrono::Utc,
        >::from_timestamp_micros(*micros)),
    }
}
