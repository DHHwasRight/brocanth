//! Where a queued message waits until a worker sends it.
//!
//! A message can hold a secret: a password reset link carries its token. A
//! job payload is a poor place for it. The payload sits in the jobs table
//! while the job waits and retries, an operator reads it in the jobs tab,
//! and a backup keeps it. So the payload carries only an id, and the message
//! waits in a [`Pending`] store:
//!
//! 1. The request calls [`Pending::put`] and enqueues the id.
//! 2. The worker calls [`Pending::get`] and sends the message. A retry reads
//!    the same row again, so a refused send is retried as before.
//! 3. After the send, or after the last attempt, the worker calls
//!    [`Pending::delete`].
//!
//! Every row has a time to live. [`Pending::get`] never returns an expired
//! row, and [`Pending::put`] deletes the expired rows. So a row whose job was
//! lost stays no longer than the TTL. [`DEFAULT_TTL`] is one hour: the five
//! attempts of a job with the default backoff take less than three minutes.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;

use crate::{MailError, Message};

/// How long a row waits before it expires: one hour.
pub const DEFAULT_TTL: Duration = Duration::from_secs(3600);

/// A store of messages that wait for a worker.
#[async_trait]
pub trait Pending: Send + Sync + 'static {
    /// Keeps `message` until [`Pending::delete`] or its TTL, and returns its
    /// id. The id is random and holds nothing of the message.
    async fn put(&self, message: &Message) -> Result<String, MailError>;

    /// The message with this id. `None` when it was deleted or it expired.
    async fn get(&self, id: &str) -> Result<Option<Message>, MailError>;

    /// Deletes the message with this id. An id that is not there is not an
    /// error.
    async fn delete(&self, id: &str) -> Result<(), MailError>;
}

/// A random id: 32 hex characters.
fn new_id() -> Result<String, MailError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| {
        MailError::transport(format!(
            "the OS gave no randomness for a pending mail id: {e}\n\
             help: check the host's random source"
        ))
    })?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn encode(message: &Message) -> Result<String, MailError> {
    serde_json::to_string(message)
        .map_err(|e| MailError::transport(format!("the message did not serialise: {e}")))
}

/// Reads a row back. The `Deserialize` impls check every header again, so a
/// row edited in the table cannot carry a newline into a header.
fn decode(text: &str) -> Result<Message, MailError> {
    serde_json::from_str(text).map_err(|e| {
        MailError::transport(format!(
            "a pending mail row did not read back: {e}\n\
             help: the row was edited or written by another version; delete it"
        ))
    })
}

/// Pending messages in this process. For a project whose queue is in
/// memory too: a restart loses both.
#[derive(Debug)]
pub struct MemoryPending {
    ttl: Duration,
    rows: Mutex<HashMap<String, (Instant, String)>>,
}

impl MemoryPending {
    /// A store whose rows expire after `ttl`.
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            rows: Mutex::new(HashMap::new()),
        }
    }

    /// How many rows the store holds, expired ones included.
    pub fn len(&self) -> usize {
        self.rows
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Whether the store holds no row.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for MemoryPending {
    fn default() -> Self {
        Self::new(DEFAULT_TTL)
    }
}

#[async_trait]
impl Pending for MemoryPending {
    async fn put(&self, message: &Message) -> Result<String, MailError> {
        let id = new_id()?;
        let text = encode(message)?;
        let now = Instant::now();
        let mut rows = self.rows.lock().unwrap_or_else(PoisonError::into_inner);
        rows.retain(|_, (expires, _)| *expires > now);
        rows.insert(id.clone(), (now + self.ttl, text));
        Ok(id)
    }

    async fn get(&self, id: &str) -> Result<Option<Message>, MailError> {
        let text = {
            let rows = self.rows.lock().unwrap_or_else(PoisonError::into_inner);
            match rows.get(id) {
                Some((expires, text)) if *expires > Instant::now() => text.clone(),
                _ => return Ok(None),
            }
        };
        decode(&text).map(Some)
    }

    async fn delete(&self, id: &str) -> Result<(), MailError> {
        self.rows
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
        Ok(())
    }
}

/// Pending messages in Postgres, in the database the `db` module opened.
#[cfg(feature = "postgres")]
#[derive(Clone, Debug)]
pub struct PgPending {
    db: brocanth_db::Db,
    ttl: Duration,
    /// Set once the table exists, so only the first call runs the schema.
    migrated: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(feature = "postgres")]
impl PgPending {
    /// The table, idempotent.
    pub const SCHEMA: &'static str = "\
CREATE TABLE IF NOT EXISTS brocanth_mail_pending (
    id         text        PRIMARY KEY,
    message    text        NOT NULL,
    expires_at timestamptz NOT NULL
)";

    /// A store over `db` whose rows expire after `ttl`. It creates its
    /// table on first use, with [`PgPending::migrate`]; the pool is lazy, so
    /// `new` waits for nothing.
    pub fn new(db: brocanth_db::Db, ttl: Duration) -> Self {
        Self {
            db,
            ttl,
            migrated: std::sync::Arc::default(),
        }
    }

    /// Runs [`PgPending::migrate`] once per store. Two calls that race both
    /// run it, and that is safe: it is idempotent and locked.
    async fn ready(&self) -> Result<(), MailError> {
        use std::sync::atomic::Ordering;
        if !self.migrated.load(Ordering::Acquire) {
            self.migrate().await?;
            self.migrated.store(true, Ordering::Release);
        }
        Ok(())
    }

    /// Runs [`PgPending::SCHEMA`] under a transaction-level advisory lock,
    /// so two replicas that start together do not race on the catalogue.
    ///
    /// # Errors
    /// The database refused the statement.
    pub async fn migrate(&self) -> Result<(), MailError> {
        use brocanth_db::sqlx;
        let mut tx = self.db.pool().begin().await.map_err(failed)?;
        sqlx::query("SELECT pg_advisory_xact_lock(7093854835689485431)")
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

#[cfg(feature = "postgres")]
fn failed(e: brocanth_db::sqlx::Error) -> MailError {
    MailError::transport(format!(
        "the pending mail table: {e}\n\
         help: check `[db] url` and that the database accepts writes"
    ))
}

#[cfg(feature = "postgres")]
#[async_trait]
impl Pending for PgPending {
    async fn put(&self, message: &Message) -> Result<String, MailError> {
        use brocanth_db::sqlx;
        self.ready().await?;
        let id = new_id()?;
        let text = encode(message)?;
        let ttl = i64::try_from(self.ttl.as_millis()).unwrap_or(i64::MAX);
        sqlx::query("DELETE FROM brocanth_mail_pending WHERE expires_at <= now()")
            .execute(self.db.pool())
            .await
            .map_err(failed)?;
        sqlx::query(
            "INSERT INTO brocanth_mail_pending (id, message, expires_at) \
             VALUES ($1, $2, now() + make_interval(secs => $3::float8 / 1000))",
        )
        .bind(&id)
        .bind(&text)
        .bind(ttl)
        .execute(self.db.pool())
        .await
        .map_err(failed)?;
        Ok(id)
    }

    async fn get(&self, id: &str) -> Result<Option<Message>, MailError> {
        use brocanth_db::sqlx;
        self.ready().await?;
        let text: Option<String> = sqlx::query_scalar(
            "SELECT message FROM brocanth_mail_pending WHERE id = $1 AND expires_at > now()",
        )
        .bind(id)
        .fetch_optional(self.db.pool())
        .await
        .map_err(failed)?;
        text.as_deref().map(decode).transpose()
    }

    async fn delete(&self, id: &str) -> Result<(), MailError> {
        use brocanth_db::sqlx;
        self.ready().await?;
        sqlx::query("DELETE FROM brocanth_mail_pending WHERE id = $1")
            .bind(id)
            .execute(self.db.pool())
            .await
            .map_err(failed)?;
        Ok(())
    }
}

/// What [`deliver`] did with a message.
#[derive(Debug)]
pub enum Delivered {
    /// The transport took it, and its row is deleted.
    Sent,
    /// No row: it expired, or another attempt sent it. Nothing to send.
    Expired,
    /// The last attempt failed too. The row is deleted, so the message and
    /// its links do not wait any longer; log `to` and `subject`.
    GaveUp {
        /// The recipients, comma-separated.
        to: String,
        /// The subject.
        subject: String,
        /// Why the transport refused it.
        error: MailError,
    },
}

/// Sends the pending message `id` once: what a queued job does on each
/// attempt.
///
/// # Errors
/// The transport refused the message and this is not the last attempt, or
/// the store failed. The job retries, and the row is still there to read.
pub async fn deliver(
    mail: &crate::Mail,
    pending: &dyn Pending,
    id: &str,
    last_attempt: bool,
) -> Result<Delivered, MailError> {
    let Some(message) = pending.get(id).await? else {
        return Ok(Delivered::Expired);
    };
    match mail.send(&message).await {
        Ok(()) => {
            // A delete that fails leaves the row to its TTL. It must not
            // fail the job: a retry would send the message twice.
            let _ = pending.delete(id).await;
            Ok(Delivered::Sent)
        }
        Err(error) if last_attempt => {
            let _ = pending.delete(id).await;
            let to: Vec<&str> = message.to().iter().map(crate::Address::address).collect();
            Ok(Delivered::GaveUp {
                to: to.join(", "),
                subject: message.subject().as_str().to_string(),
                error,
            })
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rt::block_on;
    use crate::{Address, Mail, Memory};

    fn message() -> Message {
        let mail = Mail::new(
            std::sync::Arc::new(Memory::new()),
            Address::parse("no-reply@example.test").expect("an address"),
        );
        mail.message()
            .to(Address::parse("person@example.test").expect("an address"))
            .subject("Reset your password")
            .text("https://example.test/reset/SECRET-TOKEN")
            .build()
            .expect("a message")
    }

    #[test]
    fn a_message_reads_back_until_it_is_deleted() {
        let store = MemoryPending::default();
        let id = block_on(store.put(&message())).expect("put");
        assert_eq!(id.len(), 32);
        assert!(!id.contains("SECRET"));
        let back = block_on(store.get(&id)).expect("get").expect("a row");
        assert_eq!(back.subject().as_str(), "Reset your password");
        // A retry reads the same row again.
        assert!(block_on(store.get(&id)).expect("get").is_some());
        block_on(store.delete(&id)).expect("delete");
        assert!(block_on(store.get(&id)).expect("get").is_none());
        block_on(store.delete(&id)).expect("a second delete");
    }

    /// Fails the first send, then takes every one.
    struct Flaky(std::sync::atomic::AtomicU32, std::sync::Arc<Memory>);

    #[async_trait]
    impl crate::Mailer for Flaky {
        async fn send(&self, message: &Message) -> Result<(), MailError> {
            if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                return Err(MailError::transport("the relay is down"));
            }
            crate::Mailer::send(&*self.1, message).await
        }
    }

    #[test]
    fn a_retry_sends_the_message_and_deletes_its_row() {
        let memory = std::sync::Arc::new(Memory::new());
        let flaky = std::sync::Arc::new(Flaky(Default::default(), memory.clone()));
        let mail = Mail::new(
            flaky,
            Address::parse("no-reply@example.test").expect("an address"),
        );
        let store = MemoryPending::default();
        let id = block_on(store.put(&message())).expect("put");
        // The payload a job carries: the id, and nothing of the message.
        let payload = serde_json::json!({ "id": id }).to_string();
        assert!(!payload.contains("SECRET-TOKEN"), "{payload}");

        let first = block_on(deliver(&mail, &store, &id, false));
        assert!(first.is_err(), "the refused send fails the attempt");
        assert!(block_on(store.get(&id)).expect("get").is_some());

        let second = block_on(deliver(&mail, &store, &id, false)).expect("the retry");
        assert!(matches!(second, Delivered::Sent), "{second:?}");
        assert_eq!(memory.sent().len(), 1);
        assert!(memory.sent()[0].headers().contains("Reset your password"));
        assert!(store.is_empty(), "the row is deleted after the send");
        let again = block_on(deliver(&mail, &store, &id, false)).expect("no row");
        assert!(matches!(again, Delivered::Expired), "{again:?}");
    }

    #[test]
    fn the_last_failed_attempt_deletes_the_row() {
        struct Down;
        #[async_trait]
        impl crate::Mailer for Down {
            async fn send(&self, _: &Message) -> Result<(), MailError> {
                Err(MailError::transport("the relay is down"))
            }
        }
        let mail = Mail::new(
            std::sync::Arc::new(Down),
            Address::parse("no-reply@example.test").expect("an address"),
        );
        let store = MemoryPending::default();
        let id = block_on(store.put(&message())).expect("put");
        let last = block_on(deliver(&mail, &store, &id, true)).expect("ends the job");
        match last {
            Delivered::GaveUp { to, subject, .. } => {
                assert_eq!(to, "person@example.test");
                assert_eq!(subject, "Reset your password");
            }
            other => panic!("{other:?}"),
        }
        assert!(store.is_empty());
    }

    #[test]
    fn an_expired_row_is_gone() {
        let store = MemoryPending::new(Duration::ZERO);
        let id = block_on(store.put(&message())).expect("put");
        assert!(block_on(store.get(&id)).expect("get").is_none());
        // The next put deletes it.
        block_on(store.put(&message())).expect("put");
        assert_eq!(store.len(), 1);
    }
}
