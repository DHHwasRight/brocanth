//! Sessions: a signed cookie over a store, rotated whenever privilege changes.
//!
//! # Why the cookie is signed when the session is in a database anyway
//!
//! The signature is not confidentiality — the identifier is random, not
//! secret. It is an admission filter. An unsigned identifier means every
//! forged or truncated cookie costs a database round trip, which is a free
//! denial-of-service amplifier; a signed one is rejected by an HMAC over 32
//! bytes. The store remains the authority on whether the session is alive.
//!
//! # Why rotation is a method and not a setting
//!
//! Session fixation is: the attacker fixes an identifier in the victim's
//! browser *before* the victim authenticates, then reuses it afterwards. The
//! defence is that the identifier a user arrives with is never the identifier
//! they keep after a privilege change. [`Sessions::rotate`] mints a new
//! identifier, carries the account and the absolute deadline over, and deletes
//! the old row — in that order, so a crash leaves the user logged in rather
//! than locked out.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::clock::{Clock, UnixTime};
use crate::error::AuthError;
use crate::identity::AccountId;
use crate::random::{b64, ct_eq, random_bytes, random_secret, unb64};
use crate::storage::{AuthStore, SessionRecord};

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// A session identifier: 256 random bits, base64url.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SessionId(String);

impl SessionId {
    /// A fresh identifier.
    ///
    /// # Errors
    /// See [`random_secret`].
    pub fn generate() -> Result<Self, AuthError> {
        Ok(Self(random_secret()?))
    }

    /// Wraps an existing identifier, for a store adapter reading a row.
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// The identifier as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a session is allowed to do without proving who it belongs to again.
///
/// This is deliberately coarse. Roles, permissions and ownership are the
/// application's, and in brocanth they are a typed witness rather than a
/// string. The only thing sessions need to know is whether the current
/// identity was proved recently enough for a dangerous action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Privilege {
    /// Nobody has authenticated on this session yet. A pre-login session
    /// exists so a flash message or a cart survives the login redirect.
    Anonymous,
    /// Signed in.
    Authenticated,
    /// Signed in, and re-proved recently: the state a password change, an
    /// email change or an account deletion should require.
    Elevated,
}

/// The HMAC key for cookie signatures.
///
/// It is 32 bytes and it is not the same key as anything else in your
/// application. Key separation means a bug that leaks one signer does not also
/// forge sessions.
#[derive(Clone)]
pub struct SigningKey(Arc<[u8; 32]>);

impl SigningKey {
    /// Takes an existing 32-byte key.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(Arc::new(bytes))
    }

    /// Mints a key from the OS CSPRNG.
    ///
    /// Useful in tests and in a single-process development server. A fleet
    /// must configure one shared key, or every instance invalidates the
    /// others' cookies.
    ///
    /// # Errors
    /// See [`random_bytes`].
    pub fn generate() -> Result<Self, AuthError> {
        let v = random_bytes(32)?;
        let mut k = [0u8; 32];
        k.copy_from_slice(&v);
        Ok(Self::from_bytes(k))
    }

    /// Reads a key from a base64url or hex string of 32 bytes.
    ///
    /// # Errors
    /// Returns [`AuthError::Misconfigured`] when the material is not 32 bytes.
    /// A short key is rejected rather than stretched: silently padding a
    /// four-character secret to 32 bytes is how a deployment ends up with a
    /// forgeable cookie and no warning.
    pub fn parse(text: &str) -> Result<Self, AuthError> {
        let bytes = unb64(text).map_err(|_| {
            AuthError::misconfigured("the session signing key is not valid base64url")
        })?;
        if bytes.len() != 32 {
            return Err(AuthError::misconfigured(format!(
                "the session signing key must be 32 bytes, got {}",
                bytes.len()
            )));
        }
        let mut k = [0u8; 32];
        k.copy_from_slice(&bytes);
        Ok(Self::from_bytes(k))
    }

    /// Signs a message with this key.
    ///
    /// Public because [`crate::oauth::FlowState`] seals itself with the same
    /// key: one key, one signer, one place to audit. It is not a general
    /// signing service — do not sign application data with the session key.
    pub fn sign_message(&self, message: &[u8]) -> Vec<u8> {
        let mut mac = <HmacSha256 as KeyInit>::new_from_slice(self.0.as_ref())
            .expect("HMAC accepts a key of any length");
        mac.update(message);
        mac.finalize().into_bytes().to_vec()
    }
}

impl std::fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the bytes. A key in a log is a key in a bug report.
        f.write_str("SigningKey(<redacted>)")
    }
}

/// `SameSite` for the session cookie.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SameSite {
    /// Never sent cross-site. The right default, and incompatible with an
    /// OAuth provider that returns by `form_post` — which Apple does.
    Strict,
    /// Sent on a top-level cross-site navigation. What an OAuth redirect
    /// callback needs.
    Lax,
    /// Sent always, and only legal with `Secure`.
    None,
}

impl SameSite {
    fn as_str(self) -> &'static str {
        match self {
            SameSite::Strict => "Strict",
            SameSite::Lax => "Lax",
            SameSite::None => "None",
        }
    }
}

/// How sessions behave.
#[derive(Clone, Debug)]
pub struct SessionConfig {
    /// Cookie name. The `__Host-` prefix is the default because it is the only
    /// way a browser enforces `Secure`, a path of `/` and no `Domain` — which
    /// stops a subdomain takeover from setting a session cookie for the parent.
    pub cookie_name: String,
    /// Seconds of inactivity before the session is dead.
    pub idle_timeout: i64,
    /// Seconds from creation before the session is dead regardless of use.
    ///
    /// Both timeouts exist because they answer different questions: idle
    /// bounds an unattended browser, absolute bounds a stolen cookie that is
    /// being kept warm.
    pub absolute_timeout: i64,
    /// Whether to set `Secure`. Off only makes sense on plain-HTTP localhost,
    /// and `__Host-` forbids it anyway.
    pub secure: bool,
    /// `SameSite`.
    pub same_site: SameSite,
    /// Cookie path.
    pub path: String,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            cookie_name: "__Host-session".into(),
            // Eight hours idle, thirty days absolute: long enough that a
            // normal user is not logged out mid-task, short enough that a
            // cookie lifted from a shared machine is not a season pass.
            idle_timeout: 8 * 60 * 60,
            absolute_timeout: 30 * 24 * 60 * 60,
            secure: true,
            // Lax, not Strict, because the OAuth callback is a cross-site
            // top-level navigation and Strict would drop the cookie on the way
            // back from every provider.
            same_site: SameSite::Lax,
            path: "/".into(),
        }
    }
}

/// A `Set-Cookie` header value, ready to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetCookie(String);

impl SetCookie {
    /// The header value.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SetCookie {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A new session and the cookie that carries it.
#[derive(Clone, Debug)]
pub struct StartedSession {
    /// The stored session.
    pub session: SessionRecord,
    /// The cookie to set on the response.
    pub cookie: SetCookie,
}

/// Session management over an [`AuthStore`].
pub struct Sessions<S: AuthStore, C: Clock> {
    store: Arc<S>,
    clock: Arc<C>,
    key: SigningKey,
    config: SessionConfig,
}

impl<S: AuthStore, C: Clock> Sessions<S, C> {
    /// Builds the manager.
    pub fn new(store: Arc<S>, clock: Arc<C>, key: SigningKey, config: SessionConfig) -> Self {
        Self {
            store,
            clock,
            key,
            config,
        }
    }

    /// The configuration in force.
    pub fn config(&self) -> &SessionConfig {
        &self.config
    }

    /// Opens a session.
    ///
    /// # Errors
    /// Fails when randomness or the store fails.
    pub async fn begin(
        &self,
        account: &AccountId,
        privilege: Privilege,
    ) -> Result<StartedSession, AuthError> {
        let now = self.clock.now();
        self.create(
            account.clone(),
            privilege,
            now,
            now + self.config.absolute_timeout,
        )
        .await
    }

    /// Loads the session a cookie refers to, and marks it used.
    ///
    /// The order is signature, then store, then expiry. A forged cookie never
    /// reaches the database.
    ///
    /// # Errors
    /// Returns [`AuthError::NoSession`] for an absent, unsigned, forged,
    /// unknown, idle or expired session — one variant, because the holder of a
    /// bad cookie is entitled to none of those distinctions.
    pub async fn load(&self, cookie_value: &str) -> Result<SessionRecord, AuthError> {
        let id = self.unseal(cookie_value).ok_or(AuthError::NoSession)?;
        let session = self
            .store
            .session(&id)
            .await?
            .ok_or(AuthError::NoSession)?;
        let now = self.clock.now();
        if now >= session.absolute_expires_at || now - session.last_seen_at >= self.config.idle_timeout
        {
            // A dead session is deleted on sight rather than left to a sweeper:
            // the request that discovered it is the cheapest place to do it.
            self.store.delete_session(&session.id).await?;
            return Err(AuthError::NoSession);
        }
        self.store.touch_session(&session.id, now).await?;
        Ok(SessionRecord {
            last_seen_at: now,
            ..session
        })
    }

    /// Replaces a session's identifier, keeping the account and the absolute
    /// deadline, and sets the new privilege.
    ///
    /// Call this at *every* privilege change: on login, on elevation after a
    /// re-authentication, and on any step-down. It is the session-fixation
    /// defence. The absolute deadline is carried over rather than reset, so
    /// rotating repeatedly cannot be used to hold a session open forever.
    ///
    /// # Errors
    /// Returns [`AuthError::NoSession`] when the old session is gone.
    pub async fn rotate(
        &self,
        old: &SessionId,
        privilege: Privilege,
    ) -> Result<StartedSession, AuthError> {
        let previous = self.store.session(old).await?.ok_or(AuthError::NoSession)?;
        let now = self.clock.now();
        let started = self
            .create(
                previous.account.clone(),
                privilege,
                now,
                previous.absolute_expires_at,
            )
            .await?;
        // New row first, old row second. The reverse order loses the session
        // entirely if the process dies between the two.
        self.store.delete_session(old).await?;
        Ok(started)
    }

    /// Ends one session.
    ///
    /// # Errors
    /// Fails when the store fails.
    pub async fn revoke(&self, id: &SessionId) -> Result<SetCookie, AuthError> {
        self.store.delete_session(id).await?;
        Ok(self.clearing_cookie())
    }

    /// Ends every session an account has, and answers how many.
    ///
    /// This is "sign out everywhere", and it is mandatory after a password
    /// change or reset: rotating the password does nothing about the cookie an
    /// attacker already holds.
    ///
    /// # Errors
    /// Fails when the store fails.
    pub async fn revoke_all(&self, account: &AccountId) -> Result<u64, AuthError> {
        Ok(self.store.delete_sessions_for_account(account).await?)
    }

    /// The cookie that removes the session cookie.
    pub fn clearing_cookie(&self) -> SetCookie {
        SetCookie(format!(
            "{}=; Path={}; Max-Age=0; HttpOnly{}; SameSite={}",
            self.config.cookie_name,
            self.config.path,
            if self.config.secure { "; Secure" } else { "" },
            self.config.same_site.as_str()
        ))
    }

    /// The signed cookie value for a session id, without the attributes.
    ///
    /// Exposed because an application that stores the identifier somewhere
    /// else — a bearer header for a native client, say — needs the same
    /// sealing.
    pub fn seal(&self, id: &SessionId) -> String {
        let sig = self.key.sign_message(id.as_str().as_bytes());
        format!("{}.{}", id.as_str(), b64(&sig))
    }

    /// Recovers a session id from a signed cookie value, or `None`.
    pub fn unseal(&self, value: &str) -> Option<SessionId> {
        let (id, sig) = value.rsplit_once('.')?;
        let presented = unb64(sig).ok()?;
        let expected = self.key.sign_message(id.as_bytes());
        // Constant time: a tag comparison that short-circuits is forgeable one
        // byte at a time by anybody who can measure the response.
        if ct_eq(&presented, &expected) {
            Some(SessionId::new(id))
        } else {
            None
        }
    }

    async fn create(
        &self,
        account: AccountId,
        privilege: Privilege,
        now: UnixTime,
        absolute_expires_at: UnixTime,
    ) -> Result<StartedSession, AuthError> {
        let id = SessionId::generate()?;
        let session = SessionRecord {
            id: id.clone(),
            account,
            privilege,
            created_at: now,
            last_seen_at: now,
            absolute_expires_at,
        };
        self.store.create_session(session.clone()).await?;
        let cookie = self.cookie_for(&id, absolute_expires_at - now);
        Ok(StartedSession { session, cookie })
    }

    fn cookie_for(&self, id: &SessionId, max_age: i64) -> SetCookie {
        SetCookie(format!(
            "{}={}; Path={}; Max-Age={}; HttpOnly{}; SameSite={}",
            self.config.cookie_name,
            self.seal(id),
            self.config.path,
            max_age.max(0),
            if self.config.secure { "; Secure" } else { "" },
            self.config.same_site.as_str()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tampered_cookie_does_not_unseal() {
        let key = SigningKey::from_bytes([7u8; 32]);
        let sessions: Sessions<crate::memory::MemoryStore, crate::clock::TestClock> =
            Sessions::new(
                Arc::new(crate::memory::MemoryStore::new()),
                Arc::new(crate::clock::TestClock::default()),
                key,
                SessionConfig::default(),
            );
        let id = SessionId::new("abcdef");
        let sealed = sessions.seal(&id);
        assert_eq!(sessions.unseal(&sealed), Some(id));

        let forged = format!("abcdeg.{}", sealed.rsplit_once('.').unwrap().1);
        assert_eq!(sessions.unseal(&forged), None);
        assert_eq!(sessions.unseal("abcdef"), None);
        assert_eq!(sessions.unseal("abcdef.not-base64!!"), None);
    }

    #[test]
    fn a_short_signing_key_is_refused_rather_than_stretched() {
        let err = SigningKey::parse(&b64(b"too short")).unwrap_err();
        assert!(err.to_string().contains("32 bytes"), "{err}");
    }
}
