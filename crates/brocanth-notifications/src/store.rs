//! [`NotificationStore`] and [`Inbox`]: the table, and one account's view
//! of it.
//!
//! Plain SQL through the pool the `db` module opened. Every value is a bound
//! parameter, and every statement of an [`Inbox`] names its account.
//!
//! Each call runs in one transaction. A store made with
//! [`NotificationStore::as_identity`] assumes that identity first
//! (`SET LOCAL ROLE`, the `brocanth.*` settings), so a row-level security
//! policy on the table checks the same account one floor down.

use brocanth_db::sqlx::postgres::{PgArguments, PgRow};
use brocanth_db::sqlx::query::QueryAs;
use brocanth_db::sqlx::{self, AssertSqlSafe, FromRow, Postgres, Row as _};
use brocanth_db::{Db, DbError, DbIdentity, Tx};
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
    /// The transaction did not begin, take the identity or commit.
    #[error("[notifications] {0}")]
    Transaction(#[from] DbError),
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
    /// Who each statement runs as: `None` is the login role.
    identity: Option<std::sync::Arc<DbIdentity>>,
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
            identity: None,
        }
    }

    /// This store, with every statement run as `identity`: the role and the
    /// settings a row-level security policy reads. Cheap: it shares the
    /// pool and the settings.
    ///
    /// The `rls` module's generated inbox runs as the reader, and `notify`
    /// as the `notifier` role: `docs/modules/notifications.md`.
    #[must_use]
    pub fn as_identity(&self, identity: DbIdentity) -> Self {
        Self {
            identity: Some(std::sync::Arc::new(identity)),
            ..self.clone()
        }
    }

    /// A transaction, as this store's identity when it has one.
    async fn begin(&self) -> Result<Tx<'static>, NotifyError> {
        let mut tx = self.db.begin().await?;
        if let Some(identity) = &self.identity {
            tx.assume(identity).await?;
        }
        Ok(tx)
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
        let mut tx = self.begin().await?;
        // No `RETURNING`: under row-level security, Postgres checks a
        // returned row against the `SELECT` policy, and the writer may not
        // read another account's row. `lastval()` reads the id the insert
        // drew, on this connection, with `USAGE` on the sequence.
        sqlx::query(
            "INSERT INTO brocanth_notifications (account, kind, title, body, url) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(account)
        .bind(&notice.kind)
        .bind(&notice.title)
        .bind(&notice.body)
        .bind(&notice.url)
        .execute(tx.connection())
        .await?;
        let id = sqlx::query_scalar("SELECT lastval()")
            .fetch_one(tx.connection())
            .await?;
        tx.commit().await?;
        Ok(id)
    }

    /// The inbox of one account. Every read and write through it names that
    /// account.
    pub fn inbox<'s>(&'s self, account: &str) -> Inbox<'s> {
        Inbox {
            store: self,
            account: account.to_string(),
        }
    }
}

/// One account's notifications. Made by [`NotificationStore::inbox`].
#[derive(Debug)]
pub struct Inbox<'s> {
    store: &'s NotificationStore,
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
        let mut tx = self.store.begin().await?;
        let count = sqlx::query_scalar(
            "SELECT count(*) FROM brocanth_notifications WHERE account = $1 AND read_at IS NULL",
        )
        .bind(&self.account)
        .fetch_one(tx.connection())
        .await?;
        tx.commit().await?;
        Ok(count)
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
        let mut tx = self.store.begin().await?;
        let items = query.fetch_all(tx.connection()).await?;
        tx.commit().await?;
        Ok(items)
    }

    /// Marks one notification read. `false` when the account has no
    /// notification with that id, which is also the answer for another
    /// account's id.
    ///
    /// # Errors
    /// What the database raised.
    pub async fn mark_read(&self, id: i64) -> Result<bool, NotifyError> {
        let mut tx = self.store.begin().await?;
        let done = sqlx::query(
            "UPDATE brocanth_notifications SET read_at = now() \
             WHERE id = $1 AND account = $2 AND read_at IS NULL",
        )
        .bind(id)
        .bind(&self.account)
        .execute(tx.connection())
        .await?;
        // Already read is still "the account has it".
        let held = done.rows_affected() == 1
            || sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM brocanth_notifications WHERE id = $1 AND account = $2)",
            )
            .bind(id)
            .bind(&self.account)
            .fetch_one(tx.connection())
            .await?;
        tx.commit().await?;
        Ok(held)
    }

    /// Marks every unread notification of the account read, and answers how
    /// many it marked.
    ///
    /// # Errors
    /// What the database raised.
    pub async fn mark_all_read(&self) -> Result<u64, NotifyError> {
        let mut tx = self.store.begin().await?;
        let done = sqlx::query(
            "UPDATE brocanth_notifications SET read_at = now() \
             WHERE account = $1 AND read_at IS NULL",
        )
        .bind(&self.account)
        .execute(tx.connection())
        .await?;
        tx.commit().await?;
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
