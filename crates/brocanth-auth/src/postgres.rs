//! [`PgStore`]: every storage trait in [`crate::storage`], over Postgres.
//!
//! Behind the `postgres` feature, which is off. With it off, this module and
//! `sqlx` are absent from the build.
//!
//! ```no_run
//! # use std::sync::Arc;
//! # use brocanth_auth::PgStore;
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let store = PgStore::connect("postgres://localhost/app").await?;
//! store.migrate().await?;
//! let store = Arc::new(store);
//! // Give `store` to `AuthConfig`, `Sessions`, `Accounts`, `Mfa`, `Otp`, ...
//! # Ok(())
//! # }
//! ```
//!
//! # What is in a row, and what is not
//!
//! * **Single-use tokens.** The row holds the selector and the SHA-256 of the
//!   verifier. [`Tokens`](crate::tokens::Tokens) hashes before it calls the
//!   store, so the secret half never reaches this module.
//! * **Recovery codes and login-alert fingerprints** arrive as digests and are
//!   stored as digests.
//! * **One-time codes** arrive as an argon2id PHC string.
//! * **TOTP secrets** are stored as given. TOTP is symmetric, so the verifier
//!   needs the secret itself: see [`TotpSecret`](crate::totp::TotpSecret).
//! * **Session ids** are stored as given, exactly as
//!   [`MemoryStore`](crate::memory::MemoryStore) keeps them. The trait gives a
//!   session id back to its callers:
//!   [`Sessions::end_impersonation`](crate::session::Sessions::end_impersonation)
//!   reads the operator's id off the impersonated row and seals a new cookie
//!   for it. A digest cannot do that. The cookie is the id **plus an HMAC**
//!   under the [`SigningKey`](crate::session::SigningKey), and that key is not
//!   in the database, so a copy of this table alone does not open a session.
//!
//! # The atomic operations
//!
//! [`crate::storage`] lists five operations that must be atomic. Here is how
//! each one is:
//!
//! | Operation | SQL |
//! |---|---|
//! | [`AuthStore::consume_token`] | one `DELETE ... RETURNING` |
//! | unique email | a unique index on `lower(email)` |
//! | [`OtpStore::claim_otp_attempt`] | `UPDATE ... RETURNING` and a conditional `DELETE`, in one transaction |
//! | [`MfaStore::claim_totp_step`] | `UPDATE ... WHERE last_step < $2`, and the row count |
//! | [`MfaStore::consume_recovery_code`] | one `DELETE`, and the row count |
//!
//! [`SignInLocationStore::record_sign_in_location`] is an
//! `INSERT ... ON CONFLICT DO NOTHING` and, when that inserts nothing, an
//! `UPDATE` that returns the old value under a row lock.
//!
//! # Expired rows
//!
//! Correctness never waits for a sweep: every service checks expiry when it
//! reads a row. [`PgStore::sweep`] deletes the dead rows so the tables stay
//! small. Run it from a job every few minutes.

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::postgres::{PgPool, PgRow};
use sqlx::Row as _;

use crate::clock::{Clock, SystemClock, UnixTime};
use crate::identity::{Account, AccountId, EmailAddress, ProviderLink};
use crate::impersonation::Impersonation;
use crate::mfa::FactorSet;
use crate::otp::OtpChallengeId;
use crate::session::{Privilege, SessionId};
use crate::storage::{
    AccountDirectory, AuthStore, MfaStore, NewAccount, NewProviderLink, OtpChallengeRecord,
    OtpStore, SessionRecord, SignInLocationStore, StoreError, TokenRecord, TotpSecretRecord,
};
use crate::tokens::TokenPurpose;

mod sql;

/// Every storage trait, over Postgres.
///
/// Cheap to clone: a pool handle and an `Arc` around a clock.
#[derive(Clone)]
pub struct PgStore {
    pool: PgPool,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for PgStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgStore").finish_non_exhaustive()
    }
}

/// What one [`PgStore::sweep`] deleted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Swept {
    /// Sessions past their hard deadline, or idle for the timeout or longer.
    pub sessions: u64,
    /// Single-use tokens past their expiry.
    pub tokens: u64,
    /// One-time-code challenges past their expiry.
    pub otp_challenges: u64,
}

impl PgStore {
    /// Opens a pool from a database URL.
    ///
    /// # Errors
    /// [`StoreError::Backend`] when the URL is not a Postgres URL, or the
    /// connection fails.
    pub async fn connect(url: &str) -> Result<Self, StoreError> {
        if !(url.starts_with("postgres://") || url.starts_with("postgresql://")) {
            return Err(StoreError::backend(format!(
                "`{}` is not a Postgres URL.\nhelp: PgStore needs a `postgres://` URL. \
                 For another database, implement `AuthStore` over it.",
                url.split(':').next().unwrap_or(url)
            )));
        }
        Ok(Self::from_pool(
            PgPool::connect(url).await.map_err(backend)?,
        ))
    }

    /// Uses a pool the application already has.
    ///
    /// The models, the queue and the cache can all hand out the same `sqlx`
    /// pool, so accounts do not need a fourth set of connections.
    pub fn from_pool(pool: PgPool) -> Self {
        Self {
            pool,
            clock: Arc::new(SystemClock),
        }
    }

    /// Replaces the clock that stamps `created_at` and `linked_at`.
    ///
    /// Give it the same clock the services use. A test gives it a
    /// [`TestClock`](crate::clock::TestClock).
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// The pool underneath.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// The DDL [`PgStore::migrate`] runs, one statement per item.
    ///
    /// For an application that keeps its schema in reviewed migration files:
    /// copy these into a migration and do not call `migrate`.
    pub fn schema_sql() -> &'static [&'static str] {
        sql::SCHEMA
    }

    /// Creates the tables and indexes that are not there yet.
    ///
    /// Safe to run on every boot and from several processes at once: it runs
    /// in one transaction behind an advisory lock, and every statement is
    /// `IF NOT EXISTS`.
    ///
    /// # Errors
    /// [`StoreError::Backend`] when a statement fails.
    pub async fn migrate(&self) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await.map_err(backend)?;
        sqlx::query(sql::MIGRATION_LOCK)
            .execute(&mut *tx)
            .await
            .map_err(backend)?;
        for statement in sql::SCHEMA {
            let statement: &'static str = statement;
            sqlx::query(statement)
                .execute(&mut *tx)
                .await
                .map_err(backend)?;
        }
        tx.commit().await.map_err(backend)
    }

    /// Deletes dead sessions, tokens and one-time-code challenges.
    ///
    /// A session is dead at `now` when it is past its hard deadline, or when
    /// it has been idle for `idle_timeout` seconds or longer. That is the test
    /// [`Sessions::load`](crate::session::Sessions::load) applies, so pass
    /// [`SessionConfig::idle_timeout`](crate::session::SessionConfig::idle_timeout).
    ///
    /// Sign-in locations are not swept here: their retention is a product
    /// decision. Call [`SignInLocationStore::purge_sign_in_locations`].
    ///
    /// # Errors
    /// [`StoreError::Backend`] when a statement fails.
    pub async fn sweep(&self, now: UnixTime, idle_timeout: i64) -> Result<Swept, StoreError> {
        let sessions = sqlx::query(sql::SWEEP_SESSIONS)
            .bind(now)
            .bind(idle_timeout)
            .execute(&self.pool)
            .await
            .map_err(backend)?
            .rows_affected();
        let tokens = sqlx::query(sql::SWEEP_TOKENS)
            .bind(now)
            .execute(&self.pool)
            .await
            .map_err(backend)?
            .rows_affected();
        let otp_challenges = self.purge_otp_challenges(now).await?;
        Ok(Swept {
            sessions,
            tokens,
            otp_challenges,
        })
    }
}

fn backend(e: sqlx::Error) -> StoreError {
    StoreError::backend(e.to_string())
}

fn no_account(id: &AccountId) -> StoreError {
    StoreError::backend(format!("no account `{id}`"))
}

fn digest(bytes: Vec<u8>, column: &str) -> Result<[u8; 32], StoreError> {
    bytes.try_into().map_err(|b: Vec<u8>| {
        StoreError::backend(format!("`{column}` is {} bytes, not 32", b.len()))
    })
}

fn privilege_name(p: Privilege) -> &'static str {
    match p {
        Privilege::Anonymous => "anonymous",
        Privilege::Authenticated => "authenticated",
        Privilege::Elevated => "elevated",
    }
}

fn privilege_from(name: &str) -> Result<Privilege, StoreError> {
    match name {
        "anonymous" => Ok(Privilege::Anonymous),
        "authenticated" => Ok(Privilege::Authenticated),
        "elevated" => Ok(Privilege::Elevated),
        other => Err(StoreError::backend(format!("`{other}` is not a privilege"))),
    }
}

fn purpose_from(name: &str) -> Result<TokenPurpose, StoreError> {
    match name {
        "password_reset" => Ok(TokenPurpose::PasswordReset),
        "email_verification" => Ok(TokenPurpose::EmailVerification),
        other => Err(StoreError::backend(format!(
            "`{other}` is not a token purpose"
        ))),
    }
}

fn email_from(raw: Option<String>) -> Result<Option<EmailAddress>, StoreError> {
    raw.map(|e| EmailAddress::parse(&e).map_err(|err| StoreError::backend(err.to_string())))
        .transpose()
}

fn account_from(row: &PgRow) -> Result<Account, StoreError> {
    Ok(Account {
        id: AccountId::new(row.try_get::<String, _>("id").map_err(backend)?),
        email: email_from(row.try_get("email").map_err(backend)?)?,
        email_verified: row.try_get("email_verified").map_err(backend)?,
        password_hash: row.try_get("password_hash").map_err(backend)?,
        created_at: row.try_get("created_at").map_err(backend)?,
    })
}

fn link_from(row: &PgRow) -> Result<ProviderLink, StoreError> {
    Ok(ProviderLink {
        account: AccountId::new(row.try_get::<String, _>("account").map_err(backend)?),
        provider: row.try_get("provider").map_err(backend)?,
        subject: row.try_get("subject").map_err(backend)?,
        email: email_from(row.try_get("email").map_err(backend)?)?,
        linked_at: row.try_get("linked_at").map_err(backend)?,
    })
}

fn session_from(row: &PgRow) -> Result<SessionRecord, StoreError> {
    let factors: Vec<String> = row.try_get("factors").map_err(backend)?;
    let operator: Option<String> = row.try_get("impersonation_operator").map_err(backend)?;
    let impersonation = match operator {
        None => None,
        Some(operator) => Some(Impersonation {
            operator: AccountId::new(operator),
            operator_session: SessionId::new(
                row.try_get::<String, _>("impersonation_session")
                    .map_err(backend)?,
            ),
            started_at: row.try_get("impersonation_started_at").map_err(backend)?,
            reason: row.try_get("impersonation_reason").map_err(backend)?,
        }),
    };
    Ok(SessionRecord {
        id: SessionId::new(row.try_get::<String, _>("id").map_err(backend)?),
        account: AccountId::new(row.try_get::<String, _>("account").map_err(backend)?),
        privilege: privilege_from(&row.try_get::<String, _>("privilege").map_err(backend)?)?,
        created_at: row.try_get("created_at").map_err(backend)?,
        last_seen_at: row.try_get("last_seen_at").map_err(backend)?,
        absolute_expires_at: row.try_get("absolute_expires_at").map_err(backend)?,
        factors: FactorSet::try_from(factors).map_err(StoreError::backend)?,
        impersonation,
    })
}

fn token_from(row: &PgRow) -> Result<TokenRecord, StoreError> {
    Ok(TokenRecord {
        purpose: purpose_from(&row.try_get::<String, _>("purpose").map_err(backend)?)?,
        selector: row.try_get("selector").map_err(backend)?,
        verifier_hash: digest(
            row.try_get("verifier_hash").map_err(backend)?,
            "verifier_hash",
        )?,
        account: AccountId::new(row.try_get::<String, _>("account").map_err(backend)?),
        expires_at: row.try_get("expires_at").map_err(backend)?,
    })
}

fn otp_from(row: &PgRow) -> Result<OtpChallengeRecord, StoreError> {
    let count = |column: &str| -> Result<u32, StoreError> {
        let n: i32 = row.try_get(column).map_err(backend)?;
        u32::try_from(n).map_err(|_| StoreError::backend(format!("`{column}` is negative")))
    };
    Ok(OtpChallengeRecord {
        id: OtpChallengeId::new(row.try_get::<String, _>("id").map_err(backend)?),
        account: AccountId::new(row.try_get::<String, _>("account").map_err(backend)?),
        code_hash: row.try_get("code_hash").map_err(backend)?,
        expires_at: row.try_get("expires_at").map_err(backend)?,
        attempts: count("attempts")?,
        max_attempts: count("max_attempts")?,
    })
}

/// A `u32` for an `INTEGER` column, refusing what does not fit rather than
/// wrapping it negative.
fn int(n: u32, what: &str) -> Result<i32, StoreError> {
    i32::try_from(n)
        .map_err(|_| StoreError::backend(format!("`{what}` {n} does not fit a Postgres INTEGER")))
}

fn non_negative(n: i64) -> u64 {
    u64::try_from(n).unwrap_or(0)
}

#[async_trait]
impl AuthStore for PgStore {
    async fn account(&self, id: &AccountId) -> Result<Option<Account>, StoreError> {
        sqlx::query(sql::ACCOUNT)
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(backend)?
            .as_ref()
            .map(account_from)
            .transpose()
    }

    async fn account_by_email(&self, email: &EmailAddress) -> Result<Option<Account>, StoreError> {
        sqlx::query(sql::ACCOUNT_BY_EMAIL)
            .bind(email.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(backend)?
            .as_ref()
            .map(account_from)
            .transpose()
    }

    async fn create_account(&self, new: NewAccount) -> Result<Account, StoreError> {
        let id = AccountId::generate().map_err(|e| StoreError::backend(e.to_string()))?;
        let created_at = self.clock.now();
        let inserted = sqlx::query(sql::CREATE_ACCOUNT)
            .bind(id.as_str())
            .bind(new.email.as_ref().map(EmailAddress::as_str))
            .bind(new.email_verified)
            .bind(new.password_hash.as_deref())
            .bind(created_at)
            .execute(&self.pool)
            .await;
        match inserted {
            Ok(_) => Ok(Account {
                id,
                email: new.email,
                email_verified: new.email_verified,
                password_hash: new.password_hash,
                created_at,
            }),
            Err(sqlx::Error::Database(db))
                if db.is_unique_violation() && db.constraint() == Some(sql::EMAIL_KEY) =>
            {
                Err(StoreError::EmailTaken)
            }
            Err(e) => Err(backend(e)),
        }
    }

    async fn set_password_hash(
        &self,
        id: &AccountId,
        hash: Option<&str>,
    ) -> Result<(), StoreError> {
        let done = sqlx::query(sql::SET_PASSWORD_HASH)
            .bind(id.as_str())
            .bind(hash)
            .execute(&self.pool)
            .await
            .map_err(backend)?;
        if done.rows_affected() == 0 {
            return Err(no_account(id));
        }
        Ok(())
    }

    async fn set_email_verified(&self, id: &AccountId, verified: bool) -> Result<(), StoreError> {
        let done = sqlx::query(sql::SET_EMAIL_VERIFIED)
            .bind(id.as_str())
            .bind(verified)
            .execute(&self.pool)
            .await
            .map_err(backend)?;
        if done.rows_affected() == 0 {
            return Err(no_account(id));
        }
        Ok(())
    }

    async fn link(
        &self,
        provider: &str,
        subject: &str,
    ) -> Result<Option<ProviderLink>, StoreError> {
        sqlx::query(sql::LINK)
            .bind(provider)
            .bind(subject)
            .fetch_optional(&self.pool)
            .await
            .map_err(backend)?
            .as_ref()
            .map(link_from)
            .transpose()
    }

    async fn links_for_account(
        &self,
        account: &AccountId,
    ) -> Result<Vec<ProviderLink>, StoreError> {
        sqlx::query(sql::LINKS_FOR_ACCOUNT)
            .bind(account.as_str())
            .fetch_all(&self.pool)
            .await
            .map_err(backend)?
            .iter()
            .map(link_from)
            .collect()
    }

    async fn link_provider(
        &self,
        account: &AccountId,
        new: NewProviderLink,
    ) -> Result<ProviderLink, StoreError> {
        let row = sqlx::query(sql::LINK_PROVIDER)
            .bind(&new.provider)
            .bind(&new.subject)
            .bind(account.as_str())
            .bind(new.email.as_ref().map(EmailAddress::as_str))
            .bind(self.clock.now())
            .fetch_optional(&self.pool)
            .await
            .map_err(backend)?;
        match row {
            Some(row) => link_from(&row),
            None => Err(StoreError::backend(
                "that provider subject is already linked to another account",
            )),
        }
    }

    async fn unlink_provider(&self, account: &AccountId, provider: &str) -> Result<(), StoreError> {
        sqlx::query(sql::UNLINK_PROVIDER)
            .bind(account.as_str())
            .bind(provider)
            .execute(&self.pool)
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn create_session(&self, session: SessionRecord) -> Result<(), StoreError> {
        let factors: Vec<String> = session.factors.into();
        let marker = session.impersonation.as_ref();
        sqlx::query(sql::CREATE_SESSION)
            .bind(session.id.as_str())
            .bind(session.account.as_str())
            .bind(privilege_name(session.privilege))
            .bind(session.created_at)
            .bind(session.last_seen_at)
            .bind(session.absolute_expires_at)
            .bind(factors)
            .bind(marker.map(|m| m.operator.as_str()))
            .bind(marker.map(|m| m.operator_session.as_str()))
            .bind(marker.map(|m| m.started_at))
            .bind(marker.map(|m| m.reason.as_str()))
            .execute(&self.pool)
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn session(&self, id: &SessionId) -> Result<Option<SessionRecord>, StoreError> {
        sqlx::query(sql::SESSION)
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(backend)?
            .as_ref()
            .map(session_from)
            .transpose()
    }

    async fn touch_session(&self, id: &SessionId, at: UnixTime) -> Result<(), StoreError> {
        sqlx::query(sql::TOUCH_SESSION)
            .bind(id.as_str())
            .bind(at)
            .execute(&self.pool)
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn delete_session(&self, id: &SessionId) -> Result<(), StoreError> {
        sqlx::query(sql::DELETE_SESSION)
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn delete_sessions_for_account(&self, account: &AccountId) -> Result<u64, StoreError> {
        Ok(sqlx::query(sql::DELETE_SESSIONS_FOR_ACCOUNT)
            .bind(account.as_str())
            .execute(&self.pool)
            .await
            .map_err(backend)?
            .rows_affected())
    }

    async fn store_token(&self, token: TokenRecord) -> Result<(), StoreError> {
        sqlx::query(sql::STORE_TOKEN)
            .bind(token.purpose.as_str())
            .bind(&token.selector)
            .bind(&token.verifier_hash[..])
            .bind(token.account.as_str())
            .bind(token.expires_at)
            .execute(&self.pool)
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn consume_token(
        &self,
        purpose: TokenPurpose,
        selector: &str,
    ) -> Result<Option<TokenRecord>, StoreError> {
        sqlx::query(sql::CONSUME_TOKEN)
            .bind(purpose.as_str())
            .bind(selector)
            .fetch_optional(&self.pool)
            .await
            .map_err(backend)?
            .as_ref()
            .map(token_from)
            .transpose()
    }

    async fn delete_tokens_for_account(
        &self,
        account: &AccountId,
        purpose: TokenPurpose,
    ) -> Result<(), StoreError> {
        sqlx::query(sql::DELETE_TOKENS_FOR_ACCOUNT)
            .bind(account.as_str())
            .bind(purpose.as_str())
            .execute(&self.pool)
            .await
            .map_err(backend)?;
        Ok(())
    }
}

#[async_trait]
impl AccountDirectory for PgStore {
    async fn accounts(&self, limit: u32, offset: u32) -> Result<Vec<Account>, StoreError> {
        sqlx::query(sql::ACCOUNTS_PAGE)
            .bind(i64::from(limit))
            .bind(i64::from(offset))
            .fetch_all(&self.pool)
            .await
            .map_err(backend)?
            .iter()
            .map(account_from)
            .collect()
    }

    async fn count_accounts(&self) -> Result<u64, StoreError> {
        let n: i64 = sqlx::query(sql::COUNT_ACCOUNTS)
            .fetch_one(&self.pool)
            .await
            .map_err(backend)?
            .try_get(0)
            .map_err(backend)?;
        Ok(non_negative(n))
    }

    async fn sessions_for_account(
        &self,
        account: &AccountId,
    ) -> Result<Vec<SessionRecord>, StoreError> {
        sqlx::query(sql::SESSIONS_FOR_ACCOUNT)
            .bind(account.as_str())
            .fetch_all(&self.pool)
            .await
            .map_err(backend)?
            .iter()
            .map(session_from)
            .collect()
    }

    fn is_durable(&self) -> bool {
        true
    }
}

#[async_trait]
impl MfaStore for PgStore {
    async fn totp_secret(
        &self,
        account: &AccountId,
    ) -> Result<Option<TotpSecretRecord>, StoreError> {
        let Some(row) = sqlx::query(sql::TOTP_SECRET)
            .bind(account.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(backend)?
        else {
            return Ok(None);
        };
        Ok(Some(TotpSecretRecord {
            secret: row.try_get("secret").map_err(backend)?,
            created_at: row.try_get("created_at").map_err(backend)?,
            confirmed_at: row.try_get("confirmed_at").map_err(backend)?,
            last_step: row.try_get("last_step").map_err(backend)?,
        }))
    }

    async fn put_totp_secret(
        &self,
        account: &AccountId,
        record: TotpSecretRecord,
    ) -> Result<(), StoreError> {
        sqlx::query(sql::PUT_TOTP_SECRET)
            .bind(account.as_str())
            .bind(&record.secret)
            .bind(record.created_at)
            .bind(record.confirmed_at)
            .bind(record.last_step)
            .execute(&self.pool)
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn confirm_totp_secret(
        &self,
        account: &AccountId,
        at: UnixTime,
    ) -> Result<(), StoreError> {
        sqlx::query(sql::CONFIRM_TOTP_SECRET)
            .bind(account.as_str())
            .bind(at)
            .execute(&self.pool)
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn delete_totp_secret(&self, account: &AccountId) -> Result<(), StoreError> {
        sqlx::query(sql::DELETE_TOTP_SECRET)
            .bind(account.as_str())
            .execute(&self.pool)
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn claim_totp_step(&self, account: &AccountId, step: i64) -> Result<bool, StoreError> {
        let done = sqlx::query(sql::CLAIM_TOTP_STEP)
            .bind(account.as_str())
            .bind(step)
            .execute(&self.pool)
            .await
            .map_err(backend)?;
        Ok(done.rows_affected() == 1)
    }

    async fn replace_recovery_codes(
        &self,
        account: &AccountId,
        digests: Vec<[u8; 32]>,
    ) -> Result<(), StoreError> {
        // One transaction, so a reader never sees the old set gone and the new
        // set not yet there.
        let mut tx = self.pool.begin().await.map_err(backend)?;
        sqlx::query(sql::DELETE_RECOVERY_CODES)
            .bind(account.as_str())
            .execute(&mut *tx)
            .await
            .map_err(backend)?;
        if !digests.is_empty() {
            let digests: Vec<Vec<u8>> = digests.iter().map(|d| d.to_vec()).collect();
            sqlx::query(sql::INSERT_RECOVERY_CODES)
                .bind(account.as_str())
                .bind(digests)
                .execute(&mut *tx)
                .await
                .map_err(backend)?;
        }
        tx.commit().await.map_err(backend)
    }

    async fn consume_recovery_code(
        &self,
        account: &AccountId,
        digest: &[u8; 32],
    ) -> Result<bool, StoreError> {
        let done = sqlx::query(sql::CONSUME_RECOVERY_CODE)
            .bind(account.as_str())
            .bind(&digest[..])
            .execute(&self.pool)
            .await
            .map_err(backend)?;
        Ok(done.rows_affected() == 1)
    }

    async fn count_recovery_codes(&self, account: &AccountId) -> Result<u32, StoreError> {
        let n: i64 = sqlx::query(sql::COUNT_RECOVERY_CODES)
            .bind(account.as_str())
            .fetch_one(&self.pool)
            .await
            .map_err(backend)?
            .try_get(0)
            .map_err(backend)?;
        Ok(u32::try_from(n).unwrap_or(u32::MAX))
    }
}

#[async_trait]
impl OtpStore for PgStore {
    async fn create_otp_challenge(&self, challenge: OtpChallengeRecord) -> Result<(), StoreError> {
        sqlx::query(sql::CREATE_OTP_CHALLENGE)
            .bind(challenge.id.as_str())
            .bind(challenge.account.as_str())
            .bind(&challenge.code_hash)
            .bind(challenge.expires_at)
            .bind(int(challenge.attempts, "attempts")?)
            .bind(int(challenge.max_attempts, "max_attempts")?)
            .execute(&self.pool)
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn claim_otp_attempt(
        &self,
        id: &OtpChallengeId,
        now: UnixTime,
    ) -> Result<Option<OtpChallengeRecord>, StoreError> {
        // The `UPDATE` takes the row lock and the transaction holds it until
        // the `DELETE` is done. A concurrent claim waits on the lock, then
        // either counts the next attempt or finds the row gone. No two claims
        // share one attempt, and no claim counts past the cap.
        let mut tx = self.pool.begin().await.map_err(backend)?;
        let Some(row) = sqlx::query(sql::CLAIM_OTP_ATTEMPT)
            .bind(id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(backend)?
        else {
            tx.rollback().await.map_err(backend)?;
            return Ok(None);
        };
        let claimed = otp_from(&row)?;
        if claimed.attempts >= claimed.max_attempts || now >= claimed.expires_at {
            sqlx::query(sql::DELETE_OTP_CHALLENGE)
                .bind(id.as_str())
                .execute(&mut *tx)
                .await
                .map_err(backend)?;
        }
        tx.commit().await.map_err(backend)?;
        Ok(Some(claimed))
    }

    async fn delete_otp_challenge(&self, id: &OtpChallengeId) -> Result<(), StoreError> {
        sqlx::query(sql::DELETE_OTP_CHALLENGE)
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn purge_otp_challenges(&self, now: UnixTime) -> Result<u64, StoreError> {
        Ok(sqlx::query(sql::PURGE_OTP_CHALLENGES)
            .bind(now)
            .execute(&self.pool)
            .await
            .map_err(backend)?
            .rows_affected())
    }
}

#[async_trait]
impl SignInLocationStore for PgStore {
    async fn record_sign_in_location(
        &self,
        account: &AccountId,
        fingerprint: &[u8; 32],
        at: UnixTime,
    ) -> Result<Option<UnixTime>, StoreError> {
        // Insert first. Of two concurrent first sign-ins, one inserts and the
        // other waits, finds the row and takes the update path. A purge can
        // delete the row between the two statements; then the insert path is
        // free again, so try once more.
        for _ in 0..3 {
            let inserted = sqlx::query(sql::INSERT_SIGN_IN_LOCATION)
                .bind(account.as_str())
                .bind(&fingerprint[..])
                .bind(at)
                .fetch_optional(&self.pool)
                .await
                .map_err(backend)?;
            if inserted.is_some() {
                return Ok(None);
            }
            let previous = sqlx::query(sql::TOUCH_SIGN_IN_LOCATION)
                .bind(account.as_str())
                .bind(&fingerprint[..])
                .bind(at)
                .fetch_optional(&self.pool)
                .await
                .map_err(backend)?;
            if let Some(row) = previous {
                return Ok(Some(row.try_get(0).map_err(backend)?));
            }
        }
        Err(StoreError::backend(
            "a sign-in location was deleted three times while it was being recorded",
        ))
    }

    async fn count_sign_in_locations(&self, account: &AccountId) -> Result<u64, StoreError> {
        let n: i64 = sqlx::query(sql::COUNT_SIGN_IN_LOCATIONS)
            .bind(account.as_str())
            .fetch_one(&self.pool)
            .await
            .map_err(backend)?
            .try_get(0)
            .map_err(backend)?;
        Ok(non_negative(n))
    }

    async fn purge_sign_in_locations(&self, unseen_since: UnixTime) -> Result<u64, StoreError> {
        Ok(sqlx::query(sql::PURGE_SIGN_IN_LOCATIONS)
            .bind(unseen_since)
            .execute(&self.pool)
            .await
            .map_err(backend)?
            .rows_affected())
    }
}
