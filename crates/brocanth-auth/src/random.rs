//! Unpredictable bytes, and comparisons that do not leak where they stopped.
//!
//! Session ids, `state`, `nonce`, PKCE verifiers and single-use tokens are all
//! the same requirement: N bytes from the operating system's CSPRNG, encoded
//! URL-safe. There is one generator here and everything uses it, so there is
//! one place to audit.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::error::AuthError;

/// The width of every secret this crate mints: 256 bits.
///
/// PKCE allows a verifier between 43 and 128 characters; 32 random bytes is 43
/// base64url characters, which is the minimum length and the maximum entropy
/// that length can carry.
pub const SECRET_BYTES: usize = 32;

/// `n` bytes from the OS CSPRNG.
///
/// # Errors
/// Returns [`AuthError::Crypto`] when the operating system refuses to supply
/// randomness. There is no fallback on purpose: a predictable session id is
/// worse than a failed request.
pub fn random_bytes(n: usize) -> Result<Vec<u8>, AuthError> {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf).map_err(|e| AuthError::crypto(format!("no OS randomness: {e}")))?;
    Ok(buf)
}

/// A fresh 256-bit secret, base64url without padding.
///
/// # Errors
/// See [`random_bytes`].
pub fn random_secret() -> Result<String, AuthError> {
    Ok(URL_SAFE_NO_PAD.encode(random_bytes(SECRET_BYTES)?))
}

/// SHA-256 of `data`.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().into()
}

/// SHA-256 of `data`, base64url without padding. This is also the PKCE `S256`
/// transformation, which is why it lives here rather than in two places.
pub fn sha256_b64(data: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(sha256(data))
}

/// Base64url without padding.
pub fn b64(data: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(data)
}

/// Decodes base64url without padding.
///
/// # Errors
/// Returns [`AuthError::InvalidToken`], because every caller here is decoding
/// something a client supplied and a malformed one is simply invalid.
pub fn unb64(text: &str) -> Result<Vec<u8>, AuthError> {
    URL_SAFE_NO_PAD
        .decode(text.as_bytes())
        .map_err(|_| AuthError::InvalidToken)
}

/// Equality that takes the same time whatever the inputs are.
///
/// `==` on `&[u8]` returns at the first differing byte, and a remote attacker
/// who can time the answer recovers a MAC or a token one byte at a time. Every
/// comparison of a secret in this crate goes through here.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    // `ConstantTimeEq` on slices of unequal length answers false immediately,
    // which is correct: the length of a token is not a secret, its contents
    // are.
    a.ct_eq(b).into()
}

/// Constant-time equality for two strings.
pub fn ct_eq_str(a: &str, b: &str) -> bool {
    ct_eq(a.as_bytes(), b.as_bytes())
}
