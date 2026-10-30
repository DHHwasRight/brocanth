//! Every statement [`PgStore`](super::PgStore) runs, written out.
//!
//! Written out rather than built, as in `brocanth-cache` and
//! `brocanth-storage`: you can paste each one into `psql`. Table names are
//! literals, so there is no identifier to quote and nothing to inject.
//!
//! # Times are `BIGINT`
//!
//! [`UnixTime`](crate::clock::UnixTime) is seconds since the epoch, and the
//! expiry checks compare it with the injected [`Clock`](crate::clock::Clock).
//! A `TIMESTAMPTZ` column would put a second clock (the server's) next to the
//! first. So the store writes the number it was given and reads it back
//! unchanged.
//!
//! # No foreign keys
//!
//! [`OtpStore`](crate::storage::OtpStore) writes challenges against a sentinel
//! account that does not exist, on purpose: see [`crate::otp`]. The traits
//! also have no "delete an account", so a cascade would have nothing to
//! follow. The tables therefore accept any account id, as
//! [`MemoryStore`](crate::memory::MemoryStore) does.
//!
//! # Ordering is `COLLATE "C"`
//!
//! [`AccountDirectory`](crate::storage::AccountDirectory) breaks a tie on the
//! id. `MemoryStore` compares ids as Rust strings, which is byte order. The
//! database default collation is not byte order, so every `ORDER BY ... id`
//! here names the `"C"` collation.

/// Serialises [`SCHEMA`] against every other process that runs it.
///
/// `CREATE TABLE IF NOT EXISTS` is not atomic: two processes that boot
/// together can both decide to create the table, and the second fails on a
/// system catalogue. A transaction-level advisory lock makes the second wait.
/// The constant is the ASCII of `brocauth` read as a 64-bit integer.
pub(crate) const MIGRATION_LOCK: &str = "SELECT pg_advisory_xact_lock(7093854835689485416)";

/// The tables. Each statement is idempotent, so [`migrate`] runs them on every
/// boot.
///
/// [`migrate`]: super::PgStore::migrate
pub(crate) const SCHEMA: &[&str] = &[
    // -- accounts ------------------------------------------------------------
    r#"CREATE TABLE IF NOT EXISTS brocanth_auth_accounts (
        id             TEXT    PRIMARY KEY,
        email          TEXT,
        email_verified BOOLEAN NOT NULL DEFAULT FALSE,
        password_hash  TEXT,
        created_at     BIGINT  NOT NULL
    )"#,
    // The unique index the trait requires. On `lower(email)`, so a row that
    // some other code wrote with capitals still collides with the normalised
    // address. Partial, so any number of accounts may have no address.
    r#"CREATE UNIQUE INDEX IF NOT EXISTS brocanth_auth_accounts_email_key
        ON brocanth_auth_accounts (lower(email))
        WHERE email IS NOT NULL"#,
    r#"CREATE INDEX IF NOT EXISTS brocanth_auth_accounts_newest
        ON brocanth_auth_accounts (created_at DESC, id COLLATE "C")"#,
    // -- provider links ------------------------------------------------------
    r#"CREATE TABLE IF NOT EXISTS brocanth_auth_provider_links (
        provider  TEXT   NOT NULL,
        subject   TEXT   NOT NULL,
        account   TEXT   NOT NULL,
        email     TEXT,
        linked_at BIGINT NOT NULL,
        PRIMARY KEY (provider, subject)
    )"#,
    r#"CREATE INDEX IF NOT EXISTS brocanth_auth_provider_links_account
        ON brocanth_auth_provider_links (account)"#,
    // -- sessions ------------------------------------------------------------
    //
    // The four `impersonation_*` columns are all set or all null. The check
    // keeps a half-written marker out of the table: an impersonated session
    // that lost its operator is the audit failure the marker exists to stop.
    r#"CREATE TABLE IF NOT EXISTS brocanth_auth_sessions (
        id                       TEXT   PRIMARY KEY,
        account                  TEXT   NOT NULL,
        privilege                TEXT   NOT NULL
            CHECK (privilege IN ('anonymous', 'authenticated', 'elevated')),
        created_at               BIGINT NOT NULL,
        last_seen_at             BIGINT NOT NULL,
        absolute_expires_at      BIGINT NOT NULL,
        factors                  TEXT[] NOT NULL DEFAULT '{}',
        impersonation_operator   TEXT,
        impersonation_session    TEXT,
        impersonation_started_at BIGINT,
        impersonation_reason     TEXT,
        CHECK (
            (impersonation_operator IS NULL
                AND impersonation_session IS NULL
                AND impersonation_started_at IS NULL
                AND impersonation_reason IS NULL)
            OR
            (impersonation_operator IS NOT NULL
                AND impersonation_session IS NOT NULL
                AND impersonation_started_at IS NOT NULL
                AND impersonation_reason IS NOT NULL)
        )
    )"#,
    // No index on `last_seen_at`: every request writes it, and an index there
    // would stop each of those writes from being a HOT update.
    r#"CREATE INDEX IF NOT EXISTS brocanth_auth_sessions_account
        ON brocanth_auth_sessions (account)"#,
    r#"CREATE INDEX IF NOT EXISTS brocanth_auth_sessions_expiry
        ON brocanth_auth_sessions (absolute_expires_at)"#,
    // -- single-use tokens ---------------------------------------------------
    //
    // `verifier_hash` is the SHA-256 of the secret half. The secret itself is
    // never given to the store.
    r#"CREATE TABLE IF NOT EXISTS brocanth_auth_tokens (
        purpose       TEXT   NOT NULL,
        selector      TEXT   NOT NULL,
        verifier_hash BYTEA  NOT NULL CHECK (octet_length(verifier_hash) = 32),
        account       TEXT   NOT NULL,
        expires_at    BIGINT NOT NULL,
        PRIMARY KEY (purpose, selector)
    )"#,
    r#"CREATE INDEX IF NOT EXISTS brocanth_auth_tokens_account
        ON brocanth_auth_tokens (account, purpose)"#,
    r#"CREATE INDEX IF NOT EXISTS brocanth_auth_tokens_expiry
        ON brocanth_auth_tokens (expires_at)"#,
    // -- second factors ------------------------------------------------------
    r#"CREATE TABLE IF NOT EXISTS brocanth_auth_totp_secrets (
        account      TEXT   PRIMARY KEY,
        secret       BYTEA  NOT NULL,
        created_at   BIGINT NOT NULL,
        confirmed_at BIGINT,
        last_step    BIGINT NOT NULL DEFAULT 0
    )"#,
    r#"CREATE TABLE IF NOT EXISTS brocanth_auth_recovery_codes (
        account TEXT  NOT NULL,
        digest  BYTEA NOT NULL CHECK (octet_length(digest) = 32),
        PRIMARY KEY (account, digest)
    )"#,
    // -- one-time codes ------------------------------------------------------
    r#"CREATE TABLE IF NOT EXISTS brocanth_auth_otp_challenges (
        id           TEXT    PRIMARY KEY,
        account      TEXT    NOT NULL,
        code_hash    TEXT    NOT NULL,
        expires_at   BIGINT  NOT NULL,
        attempts     INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
        max_attempts INTEGER NOT NULL CHECK (max_attempts >= 0)
    )"#,
    r#"CREATE INDEX IF NOT EXISTS brocanth_auth_otp_challenges_expiry
        ON brocanth_auth_otp_challenges (expires_at)"#,
    // -- login alerts --------------------------------------------------------
    r#"CREATE TABLE IF NOT EXISTS brocanth_auth_sign_in_locations (
        account      TEXT   NOT NULL,
        fingerprint  BYTEA  NOT NULL CHECK (octet_length(fingerprint) = 32),
        last_seen_at BIGINT NOT NULL,
        PRIMARY KEY (account, fingerprint)
    )"#,
];

/// The name of the unique index on the email, for telling
/// [`StoreError::EmailTaken`](crate::storage::StoreError::EmailTaken) apart
/// from any other unique violation.
pub(crate) const EMAIL_KEY: &str = "brocanth_auth_accounts_email_key";

// ---------------------------------------------------------------------------
// accounts
// ---------------------------------------------------------------------------

pub(crate) const ACCOUNT: &str = "SELECT id, email, email_verified, password_hash, created_at \
     FROM brocanth_auth_accounts WHERE id = $1";

/// `lower(...)` on both sides, so the lookup uses the unique index and finds
/// a row written with capitals.
pub(crate) const ACCOUNT_BY_EMAIL: &str =
    "SELECT id, email, email_verified, password_hash, created_at \
     FROM brocanth_auth_accounts WHERE lower(email) = lower($1)";

pub(crate) const CREATE_ACCOUNT: &str = "INSERT INTO brocanth_auth_accounts \
     (id, email, email_verified, password_hash, created_at) \
     VALUES ($1, $2, $3, $4, $5)";

pub(crate) const SET_PASSWORD_HASH: &str =
    "UPDATE brocanth_auth_accounts SET password_hash = $2 WHERE id = $1";

pub(crate) const SET_EMAIL_VERIFIED: &str =
    "UPDATE brocanth_auth_accounts SET email_verified = $2 WHERE id = $1";

pub(crate) const ACCOUNTS_PAGE: &str =
    "SELECT id, email, email_verified, password_hash, created_at \
     FROM brocanth_auth_accounts \
     ORDER BY created_at DESC, id COLLATE \"C\" \
     LIMIT $1 OFFSET $2";

pub(crate) const COUNT_ACCOUNTS: &str = "SELECT count(*) FROM brocanth_auth_accounts";

// ---------------------------------------------------------------------------
// provider links
// ---------------------------------------------------------------------------

pub(crate) const LINK: &str = "SELECT account, provider, subject, email, linked_at \
     FROM brocanth_auth_provider_links WHERE provider = $1 AND subject = $2";

pub(crate) const LINKS_FOR_ACCOUNT: &str = "SELECT account, provider, subject, email, linked_at \
     FROM brocanth_auth_provider_links WHERE account = $1 \
     ORDER BY linked_at, provider COLLATE \"C\", subject COLLATE \"C\"";

/// Insert, or refresh a link that already belongs to this account.
///
/// The `WHERE` on the conflict arm is the guard: a subject linked to another
/// account updates nothing and returns no row, and the caller turns that into
/// an error. One statement, so two concurrent links of one subject to two
/// accounts cannot both win.
pub(crate) const LINK_PROVIDER: &str = "INSERT INTO brocanth_auth_provider_links \
     (provider, subject, account, email, linked_at) VALUES ($1, $2, $3, $4, $5) \
     ON CONFLICT (provider, subject) DO UPDATE \
       SET email = EXCLUDED.email, linked_at = EXCLUDED.linked_at \
       WHERE brocanth_auth_provider_links.account = EXCLUDED.account \
     RETURNING account, provider, subject, email, linked_at";

pub(crate) const UNLINK_PROVIDER: &str =
    "DELETE FROM brocanth_auth_provider_links WHERE account = $1 AND provider = $2";

// ---------------------------------------------------------------------------
// sessions
// ---------------------------------------------------------------------------

/// An upsert, because `MemoryStore` replaces a session on the same id.
pub(crate) const CREATE_SESSION: &str = "INSERT INTO brocanth_auth_sessions \
     (id, account, privilege, created_at, last_seen_at, absolute_expires_at, factors, \
      impersonation_operator, impersonation_session, impersonation_started_at, \
      impersonation_reason) \
     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) \
     ON CONFLICT (id) DO UPDATE SET \
       account = EXCLUDED.account, privilege = EXCLUDED.privilege, \
       created_at = EXCLUDED.created_at, last_seen_at = EXCLUDED.last_seen_at, \
       absolute_expires_at = EXCLUDED.absolute_expires_at, factors = EXCLUDED.factors, \
       impersonation_operator = EXCLUDED.impersonation_operator, \
       impersonation_session = EXCLUDED.impersonation_session, \
       impersonation_started_at = EXCLUDED.impersonation_started_at, \
       impersonation_reason = EXCLUDED.impersonation_reason";

macro_rules! session_columns {
    () => {
        "id, account, privilege, created_at, last_seen_at, absolute_expires_at, factors, \
         impersonation_operator, impersonation_session, impersonation_started_at, \
         impersonation_reason"
    };
}

pub(crate) const SESSION: &str = concat!(
    "SELECT ",
    session_columns!(),
    " FROM brocanth_auth_sessions WHERE id = $1"
);

pub(crate) const SESSIONS_FOR_ACCOUNT: &str = concat!(
    "SELECT ",
    session_columns!(),
    " FROM brocanth_auth_sessions WHERE account = $1 \
     ORDER BY created_at DESC, id COLLATE \"C\""
);

pub(crate) const TOUCH_SESSION: &str =
    "UPDATE brocanth_auth_sessions SET last_seen_at = $2 WHERE id = $1";

pub(crate) const DELETE_SESSION: &str = "DELETE FROM brocanth_auth_sessions WHERE id = $1";

pub(crate) const DELETE_SESSIONS_FOR_ACCOUNT: &str =
    "DELETE FROM brocanth_auth_sessions WHERE account = $1";

// ---------------------------------------------------------------------------
// single-use tokens
// ---------------------------------------------------------------------------

pub(crate) const STORE_TOKEN: &str = "INSERT INTO brocanth_auth_tokens \
     (purpose, selector, verifier_hash, account, expires_at) VALUES ($1, $2, $3, $4, $5) \
     ON CONFLICT (purpose, selector) DO UPDATE SET \
       verifier_hash = EXCLUDED.verifier_hash, account = EXCLUDED.account, \
       expires_at = EXCLUDED.expires_at";

/// The atomic spend. `DELETE ... RETURNING` takes the row lock, so a second
/// caller waits for the first to commit and then finds no row.
pub(crate) const CONSUME_TOKEN: &str = "DELETE FROM brocanth_auth_tokens \
     WHERE purpose = $1 AND selector = $2 \
     RETURNING purpose, selector, verifier_hash, account, expires_at";

pub(crate) const DELETE_TOKENS_FOR_ACCOUNT: &str =
    "DELETE FROM brocanth_auth_tokens WHERE account = $1 AND purpose = $2";

// ---------------------------------------------------------------------------
// second factors
// ---------------------------------------------------------------------------

pub(crate) const TOTP_SECRET: &str = "SELECT secret, created_at, confirmed_at, last_step \
     FROM brocanth_auth_totp_secrets WHERE account = $1";

pub(crate) const PUT_TOTP_SECRET: &str = "INSERT INTO brocanth_auth_totp_secrets \
     (account, secret, created_at, confirmed_at, last_step) VALUES ($1, $2, $3, $4, $5) \
     ON CONFLICT (account) DO UPDATE SET \
       secret = EXCLUDED.secret, created_at = EXCLUDED.created_at, \
       confirmed_at = EXCLUDED.confirmed_at, last_step = EXCLUDED.last_step";

pub(crate) const CONFIRM_TOTP_SECRET: &str =
    "UPDATE brocanth_auth_totp_secrets SET confirmed_at = $2 WHERE account = $1";

pub(crate) const DELETE_TOTP_SECRET: &str =
    "DELETE FROM brocanth_auth_totp_secrets WHERE account = $1";

/// The replay defence: the row count is the answer.
pub(crate) const CLAIM_TOTP_STEP: &str = "UPDATE brocanth_auth_totp_secrets \
     SET last_step = $2 WHERE account = $1 AND last_step < $2";

pub(crate) const DELETE_RECOVERY_CODES: &str =
    "DELETE FROM brocanth_auth_recovery_codes WHERE account = $1";

pub(crate) const INSERT_RECOVERY_CODES: &str =
    "INSERT INTO brocanth_auth_recovery_codes (account, digest) \
     SELECT $1, d FROM unnest($2::bytea[]) AS d ON CONFLICT DO NOTHING";

/// Deleting is the check: the row count is the answer.
pub(crate) const CONSUME_RECOVERY_CODE: &str =
    "DELETE FROM brocanth_auth_recovery_codes WHERE account = $1 AND digest = $2";

pub(crate) const COUNT_RECOVERY_CODES: &str =
    "SELECT count(*) FROM brocanth_auth_recovery_codes WHERE account = $1";

// ---------------------------------------------------------------------------
// one-time codes
// ---------------------------------------------------------------------------

pub(crate) const CREATE_OTP_CHALLENGE: &str = "INSERT INTO brocanth_auth_otp_challenges \
     (id, account, code_hash, expires_at, attempts, max_attempts) \
     VALUES ($1, $2, $3, $4, $5, $6) \
     ON CONFLICT (id) DO UPDATE SET \
       account = EXCLUDED.account, code_hash = EXCLUDED.code_hash, \
       expires_at = EXCLUDED.expires_at, attempts = EXCLUDED.attempts, \
       max_attempts = EXCLUDED.max_attempts";

/// The first half of a claim. The `UPDATE` takes the row lock and holds it
/// to the end of the transaction, so a concurrent claim waits here and then
/// sees the counted attempt, or no row.
pub(crate) const CLAIM_OTP_ATTEMPT: &str = "UPDATE brocanth_auth_otp_challenges \
     SET attempts = attempts + 1 WHERE id = $1 \
     RETURNING id, account, code_hash, expires_at, attempts, max_attempts";

pub(crate) const DELETE_OTP_CHALLENGE: &str =
    "DELETE FROM brocanth_auth_otp_challenges WHERE id = $1";

/// `MemoryStore` keeps a challenge while `expires_at > now`, so this deletes
/// the complement.
pub(crate) const PURGE_OTP_CHALLENGES: &str =
    "DELETE FROM brocanth_auth_otp_challenges WHERE expires_at <= $1";

// ---------------------------------------------------------------------------
// login alerts
// ---------------------------------------------------------------------------

/// The first half of a sign-in record. A concurrent insert of the same key
/// waits for this one to commit and then does nothing, so only one caller
/// gets a row back and reports the network as new.
pub(crate) const INSERT_SIGN_IN_LOCATION: &str = "INSERT INTO brocanth_auth_sign_in_locations \
     (account, fingerprint, last_seen_at) VALUES ($1, $2, $3) \
     ON CONFLICT (account, fingerprint) DO NOTHING RETURNING 1";

/// The second half: write the new time and return the old one. The sub-select
/// locks the row, so two callers each get the value the other did not write.
pub(crate) const TOUCH_SIGN_IN_LOCATION: &str = "UPDATE brocanth_auth_sign_in_locations l \
     SET last_seen_at = $3 \
     FROM (SELECT account, fingerprint, last_seen_at FROM brocanth_auth_sign_in_locations \
           WHERE account = $1 AND fingerprint = $2 FOR UPDATE) old \
     WHERE l.account = old.account AND l.fingerprint = old.fingerprint \
     RETURNING old.last_seen_at";

pub(crate) const COUNT_SIGN_IN_LOCATIONS: &str =
    "SELECT count(*) FROM brocanth_auth_sign_in_locations WHERE account = $1";

pub(crate) const PURGE_SIGN_IN_LOCATIONS: &str =
    "DELETE FROM brocanth_auth_sign_in_locations WHERE last_seen_at < $1";

// ---------------------------------------------------------------------------
// sweeping
// ---------------------------------------------------------------------------

/// The same test `Sessions::load` applies: past the hard deadline, or idle
/// for the timeout or longer.
pub(crate) const SWEEP_SESSIONS: &str = "DELETE FROM brocanth_auth_sessions \
     WHERE absolute_expires_at <= $1 OR last_seen_at <= $1 - $2";

/// `Tokens::consume` refuses a token once `now >= expires_at`.
pub(crate) const SWEEP_TOKENS: &str = "DELETE FROM brocanth_auth_tokens WHERE expires_at <= $1";
