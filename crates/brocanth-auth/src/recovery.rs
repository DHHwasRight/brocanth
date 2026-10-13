//! Recovery codes: generated once, hashed at rest, spent once each.
//!
//! # What they are for, which decides everything else
//!
//! A recovery code is the answer to "my phone is in the sea". It is therefore
//! the one credential that is *written down* — on paper, in a password manager,
//! in a screenshot somebody will never delete. Two consequences follow and both
//! are load-bearing.
//!
//! **It is typed by a human, so it is formatted for a human.** Twenty symbols
//! from a thirty-two symbol alphabet, in four groups of five. The alphabet is
//! Crockford's: no `i`, no `l`, no `o`, no `u`, so there is no pair of glyphs a
//! reader can confuse and no accidental word. Input is normalised before it is
//! hashed — case folded, hyphens and spaces dropped, and the three glyphs a
//! human substitutes anyway (`i`, `l` for `1`, `o` for `0`) mapped — so a
//! correct code typed by a tired person is a correct code.
//!
//! **It is long-lived, so it has real entropy.** Twenty symbols at five bits
//! is **100 bits**, which is why the digest at rest is SHA-256 and not argon2:
//! stretching defends a secret that has too little entropy of its own, and this
//! one has 100 bits from the OS CSPRNG. The same argument [`crate::tokens`]
//! makes, for the same reason.
//!
//! A six-digit "backup code" — which several products ship — is 20 bits, lives
//! until it is used, and is the weakest credential on the account by a factor
//! of 2^80. There is no constructor for one here.
//!
//! # Single use is the store's job, not this module's
//!
//! Nothing here keeps state. A code is spent by
//! [`MfaStore::consume_recovery_code`], which deletes the row that matches the
//! digest and answers whether there was one. Deleting *is* the check, so two
//! concurrent uses of one code cannot both succeed.
//!
//! [`MfaStore::consume_recovery_code`]: crate::storage::MfaStore::consume_recovery_code

use crate::error::AuthError;
use crate::random::{random_bytes, sha256};

/// Crockford's base32, lower case. No `i`, `l`, `o` or `u`.
const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// Symbols per code. Twenty at five bits is 100 bits of entropy.
const SYMBOLS: usize = 20;

/// Symbols between hyphens.
const GROUP: usize = 5;

/// How many codes a set holds.
pub const CODES_PER_SET: usize = 10;

/// One recovery code, as the user will see it exactly once.
#[derive(Clone, PartialEq, Eq)]
pub struct RecoveryCode(String);

impl RecoveryCode {
    /// The code, formatted for display: `a1b2c-d3e4f-g5h6j-k7m8n`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The digest the store keeps.
    pub fn digest(&self) -> [u8; 32] {
        digest(&self.0)
    }
}

impl std::fmt::Debug for RecoveryCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RecoveryCode(<redacted>)")
    }
}

/// A freshly generated set, which is shown once and then exists only as
/// digests.
///
/// **Generating a set replaces the previous one.** That is not a convenience:
/// a user who regenerates their codes has just told you they think the old list
/// leaked, and a set that merely appends has done nothing about it.
#[derive(Clone, Debug)]
pub struct RecoveryCodes(Vec<RecoveryCode>);

impl RecoveryCodes {
    /// Ten fresh codes.
    ///
    /// # Errors
    /// See [`random_bytes`].
    pub fn generate() -> Result<Self, AuthError> {
        Self::generate_count(CODES_PER_SET)
    }

    /// `count` fresh codes, for an application that has measured how many its
    /// support desk actually gets through.
    ///
    /// # Errors
    /// Returns [`AuthError::Misconfigured`] for a count outside 1..=64, and
    /// whatever [`random_bytes`] fails with.
    pub fn generate_count(count: usize) -> Result<Self, AuthError> {
        if !(1..=64).contains(&count) {
            return Err(AuthError::misconfigured(format!(
                "a recovery code set holds between 1 and 64 codes, not {count}"
            )));
        }
        let mut codes = Vec::with_capacity(count);
        for _ in 0..count {
            codes.push(RecoveryCode(one_code()?));
        }
        Ok(Self(codes))
    }

    /// The codes, to show the user once and never again.
    pub fn codes(&self) -> &[RecoveryCode] {
        &self.0
    }

    /// The digests, to hand to [`MfaStore::replace_recovery_codes`].
    ///
    /// [`MfaStore::replace_recovery_codes`]: crate::storage::MfaStore::replace_recovery_codes
    pub fn digests(&self) -> Vec<[u8; 32]> {
        self.0.iter().map(RecoveryCode::digest).collect()
    }
}

/// Folds a code the way a human typed it onto the way it was generated.
///
/// Case, hyphens and whitespace are dropped. `i` and `l` become `1` and `o`
/// becomes `0`, which is Crockford's own rule and the reason those four glyphs
/// are not in the alphabet.
pub fn normalise(presented: &str) -> String {
    presented
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| match c.to_ascii_lowercase() {
            'i' | 'l' => '1',
            'o' => '0',
            other => other,
        })
        .collect()
}

/// The digest of a presented code, after normalisation.
///
/// This is the only value that ever reaches the store, and it is what
/// [`MfaStore::consume_recovery_code`] deletes by.
///
/// [`MfaStore::consume_recovery_code`]: crate::storage::MfaStore::consume_recovery_code
pub fn digest(presented: &str) -> [u8; 32] {
    sha256(normalise(presented).as_bytes())
}

fn one_code() -> Result<String, AuthError> {
    // Five bits per symbol, taken from the byte stream rather than by `%`:
    // `random_byte % 32` is unbiased only because 32 divides 256, and relying
    // on that silently breaks the day somebody edits the alphabet.
    let bits_needed = SYMBOLS * 5;
    let bytes = random_bytes(bits_needed.div_ceil(8))?;
    let mut out = String::with_capacity(SYMBOLS + SYMBOLS / GROUP);
    for i in 0..SYMBOLS {
        let bit = i * 5;
        let (byte, offset) = (bit / 8, bit % 8);
        let window = (u16::from(bytes[byte]) << 8) | u16::from(*bytes.get(byte + 1).unwrap_or(&0));
        let index = ((window >> (11 - offset)) & 0b1_1111) as usize;
        if i > 0 && i % GROUP == 0 {
            out.push('-');
        }
        out.push(ALPHABET[index] as char);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_is_four_groups_of_five_from_an_unambiguous_alphabet() {
        let set = RecoveryCodes::generate().unwrap();
        assert_eq!(set.codes().len(), CODES_PER_SET);
        for code in set.codes() {
            let text = code.as_str();
            assert_eq!(text.len(), SYMBOLS + 3, "{text}");
            assert_eq!(text.matches('-').count(), 3, "{text}");
            for ch in text.chars().filter(|c| *c != '-') {
                assert!(ALPHABET.contains(&(ch as u8)), "`{ch}` in {text}");
            }
        }
    }

    #[test]
    fn a_tired_human_still_types_the_right_code() {
        let code = "a1b2c-d3e4f-g5h6j-k7m8n";
        for typed in [
            "a1b2c-d3e4f-g5h6j-k7m8n",
            "A1B2C-D3E4F-G5H6J-K7M8N",
            "a1b2cd3e4fg5h6jk7m8n",
            " a1b2c d3e4f g5h6j k7m8n ",
            // `l` for `1` — one of the substitutions the alphabet exists to
            // make harmless.
            "alb2c-d3e4f-g5h6j-k7m8n",
        ] {
            assert_eq!(digest(typed), digest(code), "`{typed}`");
        }
    }

    #[test]
    fn two_sets_never_collide() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..16 {
            for code in RecoveryCodes::generate().unwrap().codes() {
                assert!(seen.insert(code.as_str().to_string()), "a code repeated");
            }
        }
    }

    #[test]
    fn an_absurd_set_size_is_refused() {
        assert!(RecoveryCodes::generate_count(0).is_err());
        assert!(RecoveryCodes::generate_count(65).is_err());
    }
}
