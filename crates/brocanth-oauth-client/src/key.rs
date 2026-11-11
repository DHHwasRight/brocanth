//! The one key, and what is sealed with it.
//!
//! The application sets one secret: 32 random bytes. Two keys come from it
//! through HMAC-SHA256 with a fixed label, so a sealed flow cookie can
//! never open as a sealed token, and the other way round.
//!
//! A seal is XChaCha20-Poly1305: version byte `1`, a random 24-byte nonce,
//! then the ciphertext and its tag. A token's seal binds the account, the
//! provider and the kind (access or refresh) as associated data. So a row
//! copied to another account, or a refresh token copied into the access
//! column, does not open.
//!
//! This key is not the session key of `brocanth-auth`. A session key is
//! rotated to sign everybody out; rotating it must not lose every stored
//! connection.

use base64::engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine as _;
use chacha20poly1305::aead::{Aead, KeyInit as _, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::Error;

const VERSION: u8 = 1;
const NONCE: usize = 24;

/// The secret behind every seal. Its `Debug` shows nothing.
#[derive(Clone)]
pub struct Key {
    state: [u8; 32],
    token: [u8; 32],
}

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Key(<redacted>)")
    }
}

impl Key {
    /// Reads 32 bytes in base64 (url-safe or standard, padded or not).
    ///
    /// # Errors
    /// [`Error::Config`] when the text is not base64 or not 32 bytes long.
    /// A short key is refused, not stretched.
    pub fn parse(raw: &str) -> Result<Self, Error> {
        let raw = raw.trim();
        let bytes = URL_SAFE_NO_PAD
            .decode(raw)
            .or_else(|_| URL_SAFE.decode(raw))
            .or_else(|_| STANDARD.decode(raw))
            .map_err(|_| Error::Config(format!("the key is not base64\nhelp: {}", Self::HELP)))?;
        let bytes: [u8; 32] = bytes.try_into().map_err(|b: Vec<u8>| {
            Error::Config(format!(
                "the key is {} bytes, not 32\nhelp: {}",
                b.len(),
                Self::HELP
            ))
        })?;
        Ok(Self::from_bytes(bytes))
    }

    /// How to make a key, for a `help:` line.
    pub const HELP: &'static str =
        "make one with `head -c 32 /dev/urandom | base64 | tr '+/' '-_' | tr -d '='`";

    /// The key from its 32 bytes.
    pub fn from_bytes(master: [u8; 32]) -> Self {
        Self {
            state: derive(&master, b"brocanth-oauth-client state v1"),
            token: derive(&master, b"brocanth-oauth-client token v1"),
        }
    }

    /// A new random key, as `parse` reads it.
    pub fn generate() -> String {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).expect("the OS random source");
        URL_SAFE_NO_PAD.encode(bytes)
    }

    /// Seals a token for one (account, provider, kind).
    pub(crate) fn seal_token(&self, plain: &[u8], aad: &[u8]) -> Vec<u8> {
        seal(&self.token, plain, aad)
    }

    /// Opens what [`Key::seal_token`] sealed.
    pub(crate) fn open_token(&self, sealed: &[u8], aad: &[u8]) -> Result<Vec<u8>, Error> {
        open(&self.token, sealed, aad).ok_or_else(|| Error::Crypto("the seal is wrong".into()))
    }

    /// Seals a flow cookie, as base64url text.
    pub(crate) fn seal_state(&self, plain: &[u8]) -> String {
        URL_SAFE_NO_PAD.encode(seal(&self.state, plain, b"flow"))
    }

    /// Opens what [`Key::seal_state`] sealed. `None` for anything else.
    pub(crate) fn open_state(&self, sealed: &str) -> Option<Vec<u8>> {
        let bytes = URL_SAFE_NO_PAD.decode(sealed.trim()).ok()?;
        open(&self.state, &bytes, b"flow")
    }
}

fn derive(master: &[u8; 32], label: &[u8]) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(master).expect("HMAC takes a key of any length");
    mac.update(label);
    mac.finalize().into_bytes().into()
}

fn seal(key: &[u8; 32], plain: &[u8], aad: &[u8]) -> Vec<u8> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let mut nonce = [0u8; NONCE];
    getrandom::fill(&mut nonce).expect("the OS random source");
    let sealed = cipher
        .encrypt(&XNonce::from(nonce), Payload { msg: plain, aad })
        .expect("XChaCha20-Poly1305 seals any length this crate gives it");
    let mut out = Vec::with_capacity(1 + NONCE + sealed.len());
    out.push(VERSION);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    out
}

fn open(key: &[u8; 32], sealed: &[u8], aad: &[u8]) -> Option<Vec<u8>> {
    let (&version, rest) = sealed.split_first()?;
    if version != VERSION || rest.len() < NONCE {
        return None;
    }
    let (nonce, body) = rest.split_at(NONCE);
    let nonce: [u8; NONCE] = nonce.try_into().ok()?;
    XChaCha20Poly1305::new(key.into())
        .decrypt(&XNonce::from(nonce), Payload { msg: body, aad })
        .ok()
}

/// Random bytes as base64url text: a `state`.
pub(crate) fn random_text(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("the OS random source");
    URL_SAFE_NO_PAD.encode(buf)
}

/// Equality that takes the same time wherever the first difference is.
pub(crate) fn ct_eq(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq as _;
    a.len() == b.len() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_opens_only_for_its_own_account_and_kind() {
        let key = Key::from_bytes([3; 32]);
        let sealed = key.seal_token(b"secret", b"alice\0github\0access");
        assert_eq!(
            key.open_token(&sealed, b"alice\0github\0access").unwrap(),
            b"secret"
        );
        assert!(key.open_token(&sealed, b"bob\0github\0access").is_err());
        assert!(key.open_token(&sealed, b"alice\0github\0refresh").is_err());
        assert!(!sealed.windows(6).any(|w| w == b"secret"), "encrypted");
    }

    #[test]
    fn another_key_or_a_flipped_bit_does_not_open() {
        let key = Key::from_bytes([3; 32]);
        let mut sealed = key.seal_token(b"secret", b"a");
        assert!(Key::from_bytes([4; 32]).open_token(&sealed, b"a").is_err());
        let last = sealed.len() - 1;
        sealed[last] ^= 1;
        assert!(key.open_token(&sealed, b"a").is_err());
    }

    #[test]
    fn a_state_seal_is_not_a_token_seal() {
        let key = Key::from_bytes([3; 32]);
        let state = key.seal_state(b"x");
        assert_eq!(key.open_state(&state).unwrap(), b"x");
        let raw = URL_SAFE_NO_PAD.decode(&state).unwrap();
        assert!(key.open_token(&raw, b"flow").is_err());
    }

    #[test]
    fn a_short_key_is_refused_with_help() {
        let err = Key::parse("c2hvcnQ").unwrap_err().to_string();
        assert!(err.contains("not 32") && err.contains("help:"), "{err}");
        assert!(Key::parse(&Key::generate()).is_ok());
        assert_eq!(format!("{:?}", Key::from_bytes([1; 32])), "Key(<redacted>)");
    }

    #[test]
    fn equal_text_compares_equal() {
        assert!(ct_eq("abc", "abc"));
        assert!(!ct_eq("abc", "abd"));
        assert!(!ct_eq("abc", "ab"));
    }
}
