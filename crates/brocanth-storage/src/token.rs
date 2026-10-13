//! Short-lived bearer tokens for reading a protected file.
//!
//! PocketBase's shape: a private file is not served from a guessable URL, it is
//! served to whoever presents a file token the server just issued. This module
//! is the token half — mint, verify, expire. The HTTP half, which decides
//! *whether* to mint one, is the application's, and that is the important
//! half: **a file token carries no identity.** It says "the bearer may read
//! this object", nothing more. Mint one only for a request that has already
//! passed the policy, because after that the token is the only thing standing
//! between the object and anyone the URL is forwarded to.
//!
//! # Why it is stateless, and what that costs
//!
//! The token is its own record: an object address and an expiry, signed with
//! HMAC-SHA256. Nothing is written anywhere.
//!
//! * `brocanth-auth` does the opposite for a password reset — selector in a
//!   row, verifier hashed — because a reset token must be revocable the
//!   instant it is used, and single use is the entire value of it.
//! * A file token is not single use. The same `<img>` is fetched twice, a
//!   range request arrives after a seek, a CDN revalidates. A row per image
//!   tag is a write per image tag.
//! * And §8 forbids this crate assuming a database. A stateful file token
//!   would need a store trait, an adapter and a sweeper for something whose
//!   whole lifetime is two minutes.
//!
//! The cost is stated plainly: **a token cannot be revoked before it
//! expires.** Revoking the grant revokes nothing already minted. That is why
//! the default TTL is [`DEFAULT_TTL`] seconds rather than an hour, and why
//! rotating the [`FileTokenKey`] is the break-glass — it invalidates every
//! outstanding token at once.
//!
//! # The decisions inside the format
//!
//! * **The MAC covers the expiry.** Otherwise the holder of a two-minute token
//!   holds a permanent one.
//! * **The MAC is checked before anything in the payload is read**, and in
//!   constant time. A comparison that returns on the first differing byte can
//!   be walked one byte at a time by a remote attacker.
//! * **The key is domain-separated** from every other use of the same
//!   deployment secret, so a signature here cannot be replayed as a session
//!   cookie and an address key ([`HashKey`](crate::key::HashKey)) cannot sign
//!   a token.
//! * **A version byte leads the payload**, so a format change is a rejection
//!   rather than a misparse.
//! * **One error for forged, malformed and expired.** Telling the holder of a
//!   bad token which of the three it was tells an attacker whether the address
//!   existed and whether the key is right. Same cut `brocanth-auth` makes for
//!   `InvalidToken`.
//! * **A token names an object, not a variant.** Every derived variant of an
//!   object is covered by the object's token: a thumbnail discloses a subset
//!   of what the original does, and a separate token per size would be a
//!   round trip per image on a gallery page.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::error::StorageError;
use crate::key::{ObjectKey, KEY_BYTES};

/// Seconds since the Unix epoch, as `brocanth-auth` spells it.
pub type UnixTime = i64;

/// How long a file token lives when nothing says otherwise: two minutes.
///
/// Long enough for a browser to follow a redirect and fetch a page full of
/// images, short enough that a URL pasted into a chat is dead before anyone
/// clicks it. The token cannot be revoked, so this number *is* the revocation
/// window.
pub const DEFAULT_TTL: i64 = 120;

const VERSION: u8 = 1;
const PAYLOAD_LEN: usize = 1 + KEY_BYTES + 8;
const MAC_LEN: usize = 32;

/// The secret that signs file tokens.
///
/// One per deployment, held beside the other secrets, and never the same value
/// as an address key — [`FileTokenKey::from_secret`] and
/// [`HashKey::from_secret`](crate::key::HashKey::from_secret) derive from
/// different domains for exactly that reason, so one deployment secret can
/// safely feed both.
///
/// Rotating it invalidates every outstanding token immediately, which is the
/// only revocation this design has.
#[derive(Clone)]
pub struct FileTokenKey([u8; 32]);

impl FileTokenKey {
    /// Takes thirty-two bytes as the key.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Derives the key from an arbitrary deployment secret, domain-separated.
    pub fn from_secret(secret: &[u8]) -> Self {
        let mut h = Sha256::new();
        h.update(b"brocanth-storage/file-token-key/v1");
        h.update(secret);
        Self(h.finalize().into())
    }

    /// A fresh key from the OS CSPRNG.
    ///
    /// Useful in a test and in a single-process deployment. A deployment with
    /// two instances must derive the key from a shared secret instead, or a
    /// token minted by one instance will not verify on the other.
    ///
    /// # Errors
    /// Returns [`StorageError::Backend`] when the operating system refuses to
    /// supply randomness.
    pub fn generate() -> Result<Self, StorageError> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes)
            .map_err(|e| StorageError::backend(format!("no OS randomness: {e}")))?;
        Ok(Self(bytes))
    }
}

impl std::fmt::Debug for FileTokenKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FileTokenKey(<redacted>)")
    }
}

/// A minted token.
///
/// The string is a bearer credential. Put it in the URL you hand to the
/// browser and then drop it; logging it is logging the file.
#[derive(Clone)]
pub struct FileToken {
    token: String,
    /// What it opens.
    pub key: ObjectKey,
    /// When it stops working, as seconds since the epoch.
    pub expires_at: UnixTime,
}

impl FileToken {
    /// The value to put in the URL.
    pub fn as_str(&self) -> &str {
        &self.token
    }

    /// The value to put in the URL, consuming the token.
    pub fn into_string(self) -> String {
        self.token
    }
}

impl std::fmt::Debug for FileToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileToken")
            .field("key", &self.key)
            .field("expires_at", &self.expires_at)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// Mints and verifies file tokens.
#[derive(Clone, Debug)]
pub struct FileTokens {
    key: FileTokenKey,
    ttl: i64,
}

impl FileTokens {
    /// Builds the service with a [`DEFAULT_TTL`] second lifetime.
    pub fn new(key: FileTokenKey) -> Self {
        Self {
            key,
            ttl: DEFAULT_TTL,
        }
    }

    /// Overrides the lifetime, in seconds.
    ///
    /// Read [`DEFAULT_TTL`] before raising it: this number is the window in
    /// which a leaked URL still works.
    #[must_use]
    pub fn with_ttl(mut self, seconds: i64) -> Self {
        self.ttl = seconds;
        self
    }

    /// The lifetime, in seconds.
    pub fn ttl(&self) -> i64 {
        self.ttl
    }

    /// Mints a token for an object, expiring [`FileTokens::ttl`] seconds from
    /// now.
    pub fn issue(&self, key: &ObjectKey) -> FileToken {
        self.issue_at(key, now())
    }

    /// Mints a token as if the time were `now`.
    ///
    /// The clock is a parameter rather than a trait, because every expiry test
    /// worth writing is "move the clock past it" and a trait would be three
    /// types to express one integer.
    pub fn issue_at(&self, key: &ObjectKey, now: UnixTime) -> FileToken {
        let expires_at = now.saturating_add(self.ttl);
        let payload = payload(key, expires_at);
        let mac = self.mac(&payload);
        FileToken {
            token: format!(
                "{}.{}",
                URL_SAFE_NO_PAD.encode(payload),
                URL_SAFE_NO_PAD.encode(mac)
            ),
            key: *key,
            expires_at,
        }
    }

    /// Verifies a token and answers which object it opens.
    ///
    /// # Errors
    /// Returns [`StorageError::InvalidToken`] for a forged, malformed, expired
    /// or wrong-version token. One variant, for the reason in the module
    /// documentation.
    pub fn verify(&self, presented: &str) -> Result<ObjectKey, StorageError> {
        self.verify_at(presented, now())
    }

    /// Verifies a token as if the time were `now`.
    ///
    /// # Errors
    /// See [`FileTokens::verify`].
    pub fn verify_at(&self, presented: &str, now: UnixTime) -> Result<ObjectKey, StorageError> {
        // A bound before any decoding, so a megabyte in a query string is
        // rejected by a comparison rather than by an allocator.
        if presented.len() > 256 {
            return Err(StorageError::InvalidToken);
        }
        let (payload_b64, mac_b64) = presented
            .split_once('.')
            .ok_or(StorageError::InvalidToken)?;
        let payload = URL_SAFE_NO_PAD
            .decode(payload_b64)
            .map_err(|_| StorageError::InvalidToken)?;
        let presented_mac = URL_SAFE_NO_PAD
            .decode(mac_b64)
            .map_err(|_| StorageError::InvalidToken)?;
        if payload.len() != PAYLOAD_LEN || presented_mac.len() != MAC_LEN {
            return Err(StorageError::InvalidToken);
        }
        // Authenticate first. Nothing below this line reads the payload until
        // the MAC says the payload is ours.
        let expected = self.mac(&payload);
        if !bool::from(expected.ct_eq(&presented_mac[..])) {
            return Err(StorageError::InvalidToken);
        }
        if payload[0] != VERSION {
            return Err(StorageError::InvalidToken);
        }
        let mut address = [0u8; KEY_BYTES];
        address.copy_from_slice(&payload[1..1 + KEY_BYTES]);
        let mut expiry = [0u8; 8];
        expiry.copy_from_slice(&payload[1 + KEY_BYTES..]);
        let expires_at = i64::from_be_bytes(expiry);
        if now >= expires_at {
            return Err(StorageError::InvalidToken);
        }
        Ok(ObjectKey::from_bytes(address))
    }

    fn mac(&self, payload: &[u8]) -> [u8; MAC_LEN] {
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(&self.key.0)
            .expect("HMAC accepts a key of any length");
        // The domain string is inside the MAC, not only in the key
        // derivation, so a signature cannot be lifted into another protocol
        // that happens to share the key.
        mac.update(b"brocanth-storage/file-token/v1");
        mac.update(payload);
        mac.finalize().into_bytes().into()
    }
}

fn payload(key: &ObjectKey, expires_at: UnixTime) -> Vec<u8> {
    let mut out = Vec::with_capacity(PAYLOAD_LEN);
    out.push(VERSION);
    out.extend_from_slice(key.as_bytes());
    out.extend_from_slice(&expires_at.to_be_bytes());
    out
}

/// Seconds since the Unix epoch, from the system clock.
pub fn now() -> UnixTime {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
