//! The key that seals second-factor secrets at rest.
//!
//! A TOTP secret is a credential: whoever reads it makes valid codes. So a
//! store with a key keeps it sealed, XChaCha20-Poly1305 with the account id
//! as associated data, and a backup or a read-only SQL injection reads
//! noise. A row copied to another account does not open either.
//!
//! The key is not the session signing key. That key is rotated to sign
//! everybody out, and rotating it must not turn off every authenticator.
//! It lives outside the database, like the session key: in the environment.
//!
//! A sealed value is the magic `bts1`, a random 24-byte nonce, then the
//! ciphertext and its tag. A row written before the key was set is plain;
//! it still works, and the next enrolment of that account seals it.

use base64::engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine as _;
use chacha20poly1305::aead::{Aead, KeyInit as _, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};

use crate::error::AuthError;
use crate::identity::AccountId;

const MAGIC: &[u8; 4] = b"bts1";
const NONCE: usize = 24;
const TAG: usize = 16;

/// The secret behind every sealed TOTP secret. Its `Debug` shows nothing.
#[derive(Clone)]
pub struct SecretKey([u8; 32]);

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretKey(<redacted>)")
    }
}

impl SecretKey {
    /// How to make a key, for a `help:` line.
    pub const HELP: &'static str =
        "make one with `head -c 32 /dev/urandom | base64 | tr '+/' '-_' | tr -d '='`";

    /// The key from its 32 bytes.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Reads 32 bytes in base64 (url-safe or standard, padded or not). A
    /// short key is refused, not stretched.
    ///
    /// # Errors
    /// [`AuthError::Misconfigured`] when the text is not base64 or not 32
    /// bytes long.
    pub fn parse(raw: &str) -> Result<Self, AuthError> {
        let raw = raw.trim();
        let bytes = URL_SAFE_NO_PAD
            .decode(raw)
            .or_else(|_| URL_SAFE.decode(raw))
            .or_else(|_| STANDARD.decode(raw))
            .map_err(|_| {
                AuthError::misconfigured(format!(
                    "the second-factor key is not base64\nhelp: {}",
                    Self::HELP
                ))
            })?;
        let bytes: [u8; 32] = bytes.try_into().map_err(|b: Vec<u8>| {
            AuthError::misconfigured(format!(
                "the second-factor key is {} bytes, not 32\nhelp: {}",
                b.len(),
                Self::HELP
            ))
        })?;
        Ok(Self(bytes))
    }

    /// Seals one account's TOTP secret.
    ///
    /// # Errors
    /// [`AuthError::Crypto`] when the OS has no randomness for the nonce.
    pub(crate) fn seal_totp(
        &self,
        account: &AccountId,
        plain: &[u8],
    ) -> Result<Vec<u8>, AuthError> {
        let mut nonce = [0u8; NONCE];
        getrandom::fill(&mut nonce)
            .map_err(|e| AuthError::crypto(format!("no randomness for a nonce: {e}")))?;
        let sealed = XChaCha20Poly1305::new((&self.0).into())
            .encrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: plain,
                    aad: &aad(account),
                },
            )
            .map_err(|_| AuthError::crypto("sealing a TOTP secret failed"))?;
        let mut out = Vec::with_capacity(MAGIC.len() + NONCE + sealed.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        Ok(out)
    }

    /// Opens what [`SecretKey::seal_totp`] sealed for this account.
    ///
    /// # Errors
    /// [`AuthError::Crypto`] when the key, the account or the bytes are
    /// wrong.
    pub(crate) fn open_totp(
        &self,
        account: &AccountId,
        sealed: &[u8],
    ) -> Result<Vec<u8>, AuthError> {
        let body = &sealed[MAGIC.len()..];
        let (nonce, body) = body.split_at(NONCE);
        let nonce: [u8; NONCE] = nonce
            .try_into()
            .map_err(|_| AuthError::crypto("a sealed TOTP secret is cut short"))?;
        XChaCha20Poly1305::new((&self.0).into())
            .decrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: body,
                    aad: &aad(account),
                },
            )
            .map_err(|_| {
                AuthError::crypto(
                    "a sealed TOTP secret does not open with this key\n\
                     help: set the second-factor key the secret was sealed with",
                )
            })
    }
}

/// Whether stored bytes are a sealed secret rather than a plain one.
pub(crate) fn is_sealed(stored: &[u8]) -> bool {
    stored.len() >= MAGIC.len() + NONCE + TAG && stored.starts_with(MAGIC)
}

fn aad(account: &AccountId) -> Vec<u8> {
    let mut out = b"totp\0".to_vec();
    out.extend_from_slice(account.as_str().as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_opens_only_for_its_account_and_key() {
        let key = SecretKey::from_bytes([3; 32]);
        let ada = AccountId::new("ada");
        let sealed = key.seal_totp(&ada, b"12345678901234567890").unwrap();
        assert!(is_sealed(&sealed));
        assert!(!sealed.windows(20).any(|w| w == b"12345678901234567890"));
        assert_eq!(
            key.open_totp(&ada, &sealed).unwrap(),
            b"12345678901234567890"
        );
        assert!(key.open_totp(&AccountId::new("eve"), &sealed).is_err());
        assert!(SecretKey::from_bytes([4; 32])
            .open_totp(&ada, &sealed)
            .is_err());
        assert!(!is_sealed(b"12345678901234567890"));
    }

    #[test]
    fn a_short_key_is_refused_with_help() {
        let err = SecretKey::parse("c2hvcnQ").unwrap_err().to_string();
        assert!(err.contains("not 32") && err.contains("help:"), "{err}");
        assert_eq!(
            format!("{:?}", SecretKey::from_bytes([1; 32])),
            "SecretKey(<redacted>)"
        );
    }
}
