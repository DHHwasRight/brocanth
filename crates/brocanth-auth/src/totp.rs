//! TOTP: RFC 6238, with the parameters every authenticator app assumes.
//!
//! # Why there is no algorithm choice here either
//!
//! [`crate::password`] refuses to make the hash function configurable. This
//! module refuses for the opposite reason. A password hash is ours alone, so we
//! pick the strongest thing; a TOTP secret is *shared with a program we did not
//! write*, and the only parameters that program will agree with are the ones
//! RFC 6238's reference implementation shipped with: **HMAC-SHA-1, six digits,
//! a thirty-second step**. Every scanner assumes them, most ignore the
//! `algorithm` and `digits` fields of the `otpauth:` URI entirely, and the
//! failure mode is not an error message — it is a user who types a code that is
//! simply wrong, forever, with no way to find out why.
//!
//! SHA-1 is not a weakness here. The attack on SHA-1 is collision search, and
//! HMAC does not need collision resistance; the secret is 160 bits from the OS
//! CSPRNG and the output is six digits that live for thirty seconds.
//!
//! [`Totp::custom`] exists, is documented as a trap, and is what the RFC's own
//! eight-digit test vectors need.
//!
//! # The two things implementations forget
//!
//! 1. **Skew.** A phone whose clock is twenty seconds out produces the previous
//!    step's code. One step either side is the usual tolerance and it is what
//!    [`Totp::interoperable`] uses: it widens the window a guesser gets from
//!    one code to three, which is why the window is three and not eleven.
//! 2. **Replay inside a step.** A correct code stays correct for the rest of
//!    its thirty seconds, so an attacker who reads it over the user's shoulder,
//!    off a phishing page or out of a log has thirty seconds to use it *again*.
//!    This module produces the step number it matched, and
//!    [`MfaStore::claim_totp_step`] is where that number is spent exactly once.
//!    Neither half works without the other, which is why `matching_step`
//!    returns the step rather than a `bool`.
//!
//! [`MfaStore::claim_totp_step`]: crate::storage::MfaStore::claim_totp_step

use hmac::{Hmac, KeyInit, Mac};
use sha1::Sha1;

use crate::clock::UnixTime;
use crate::error::AuthError;
use crate::random::{ct_eq_str, random_bytes};

type HmacSha1 = Hmac<Sha1>;

/// RFC 4648 base32, which is what an `otpauth:` URI carries.
const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// A shared TOTP secret.
///
/// # It is stored in the clear, and that is not an oversight
///
/// TOTP is symmetric: the server must hold the same bytes the phone holds, so
/// there is no hashing to do. A database leak hands the attacker a working
/// second factor for every enrolled account, and the only real mitigation is
/// encrypting the column with a key the database does not contain — which needs
/// a key-management decision this crate cannot make for an application and will
/// not pretend to. Say it out loud in your threat model rather than discovering
/// it after a dump.
#[derive(Clone, PartialEq, Eq)]
pub struct TotpSecret(Vec<u8>);

impl TotpSecret {
    /// A fresh 160-bit secret.
    ///
    /// 160 bits because that is HMAC-SHA-1's block output and RFC 4226's
    /// recommended minimum; longer buys nothing against a six-digit code.
    ///
    /// # Errors
    /// See [`random_bytes`].
    pub fn generate() -> Result<Self, AuthError> {
        Ok(Self(random_bytes(20)?))
    }

    /// Wraps bytes a store read back.
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self(bytes.into())
    }

    /// The raw bytes, for a store writing a row.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// The secret as unpadded base32, which is what a QR code contains.
    pub fn to_base32(&self) -> String {
        let mut out = String::new();
        let mut buffer: u16 = 0;
        let mut bits = 0u32;
        for byte in &self.0 {
            buffer = (buffer << 8) | u16::from(*byte);
            bits += 8;
            while bits >= 5 {
                bits -= 5;
                let index = ((buffer >> bits) & 0b1_1111) as usize;
                out.push(BASE32[index] as char);
            }
        }
        if bits > 0 {
            let index = ((buffer << (5 - bits)) & 0b1_1111) as usize;
            out.push(BASE32[index] as char);
        }
        out
    }

    /// Reads a secret back from base32, with or without padding.
    ///
    /// # Errors
    /// Returns [`AuthError::Misconfigured`] for a character the alphabet does
    /// not contain. This parses an operator's input or a migrated row, never a
    /// value an attacker supplies, so the error says what is wrong.
    pub fn from_base32(text: &str) -> Result<Self, AuthError> {
        let mut out = Vec::new();
        let mut buffer: u16 = 0;
        let mut bits = 0u32;
        for ch in text.chars().filter(|c| *c != '=' && !c.is_whitespace()) {
            let upper = ch.to_ascii_uppercase() as u8;
            let value = BASE32.iter().position(|c| *c == upper).ok_or_else(|| {
                AuthError::misconfigured(format!("`{ch}` is not a base32 character"))
            })?;
            buffer = (buffer << 5) | value as u16;
            bits += 5;
            if bits >= 8 {
                bits -= 8;
                out.push((buffer >> bits) as u8);
            }
        }
        Ok(Self(out))
    }

    /// The `otpauth://totp/...` URI an authenticator app scans.
    ///
    /// `issuer` is your product's name and `account` is how the user will
    /// recognise which of their accounts this is — an email address, usually.
    /// Both are percent-encoded here, because an issuer with a space in it
    /// produces a URI that half the scanners truncate.
    pub fn otpauth_uri(&self, totp: &Totp, issuer: &str, account: &str) -> String {
        let issuer_e = percent_encode(issuer);
        let account_e = percent_encode(account);
        format!(
            "otpauth://totp/{issuer_e}:{account_e}?secret={}&issuer={issuer_e}&algorithm=SHA1&digits={}&period={}",
            self.to_base32(),
            totp.digits,
            totp.step_seconds
        )
    }
}

impl std::fmt::Debug for TotpSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // A secret in a log is a second factor in a bug report.
        f.write_str("TotpSecret(<redacted>)")
    }
}

/// Percent-encodes everything that is not unreserved, which is the only safe
/// rule for a label a user typed.
fn percent_encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// The RFC 6238 parameters in force.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Totp {
    digits: u32,
    step_seconds: i64,
    skew_steps: i64,
}

impl Totp {
    /// Six digits, thirty seconds, one step of tolerance either side.
    ///
    /// These are the values every authenticator app assumes. Use them.
    pub const fn interoperable() -> Self {
        Self {
            digits: 6,
            step_seconds: 30,
            skew_steps: 1,
        }
    }

    /// Other parameters, which you almost certainly must not use.
    ///
    /// Nothing in the ecosystem honours them reliably: a scanner that ignores
    /// the `digits` and `period` fields of the `otpauth:` URI will enrol the
    /// secret happily and then produce codes that never match, with no error
    /// anywhere. The legitimate reasons are a test suite — RFC 6238's published
    /// vectors are eight digits — and a migration from a system that already
    /// chose differently.
    ///
    /// # Errors
    /// Returns [`AuthError::Misconfigured`] for fewer than six or more than ten
    /// digits, a step outside 1..=300 seconds, or a negative skew. Ten digits
    /// is the ceiling because dynamic truncation yields a 31-bit number and
    /// anything past ten digits is a constant leading zero.
    pub fn custom(digits: u32, step_seconds: i64, skew_steps: i64) -> Result<Self, AuthError> {
        if !(6..=10).contains(&digits) {
            return Err(AuthError::misconfigured(format!(
                "a TOTP code is between 6 and 10 digits, not {digits}"
            )));
        }
        if !(1..=300).contains(&step_seconds) {
            return Err(AuthError::misconfigured(format!(
                "a TOTP step is between 1 and 300 seconds, not {step_seconds}"
            )));
        }
        if skew_steps < 0 {
            return Err(AuthError::misconfigured("the TOTP skew cannot be negative"));
        }
        Ok(Self {
            digits,
            step_seconds,
            skew_steps,
        })
    }

    /// How many digits a code has.
    pub fn digits(self) -> u32 {
        self.digits
    }

    /// How long a step lasts, in seconds.
    pub fn step_seconds(self) -> i64 {
        self.step_seconds
    }

    /// How many steps either side are accepted.
    pub fn skew_steps(self) -> i64 {
        self.skew_steps
    }

    /// The step number covering an instant. This is the value replay
    /// prevention spends.
    pub fn step_at(self, at: UnixTime) -> i64 {
        at.div_euclid(self.step_seconds)
    }

    /// The code for one step.
    pub fn code_for_step(self, secret: &TotpSecret, step: i64) -> String {
        let mut mac = <HmacSha1 as KeyInit>::new_from_slice(secret.as_bytes())
            .expect("HMAC accepts a key of any length");
        mac.update(&(step as u64).to_be_bytes());
        let tag = mac.finalize().into_bytes();

        // Dynamic truncation, RFC 4226 §5.3: the low nibble of the last byte
        // picks a four-byte window, and the top bit is cleared so the result
        // does not depend on the platform's signedness.
        let offset = (tag[tag.len() - 1] & 0x0f) as usize;
        let binary = u32::from_be_bytes([
            tag[offset] & 0x7f,
            tag[offset + 1],
            tag[offset + 2],
            tag[offset + 3],
        ]);
        let modulus = 10u32.pow(self.digits);
        format!("{:0width$}", binary % modulus, width = self.digits as usize)
    }

    /// The code for an instant.
    pub fn code_at(self, secret: &TotpSecret, at: UnixTime) -> String {
        self.code_for_step(secret, self.step_at(at))
    }

    /// Which step a presented code belongs to, within the skew window.
    ///
    /// Returns `None` when nothing in the window matches. The comparison is
    /// constant time and the loop does **not** stop at the first match: an
    /// early return tells a caller who can measure it *which* step matched,
    /// which is thirty seconds of the user's clock offset per sign-in.
    ///
    /// The step is the return value rather than `true` because a `bool` is the
    /// whole reason implementations accept a replay. See the module
    /// documentation.
    pub fn matching_step(self, secret: &TotpSecret, presented: &str, now: UnixTime) -> Option<i64> {
        let here = self.step_at(now);
        let mut found: Option<i64> = None;
        for offset in -self.skew_steps..=self.skew_steps {
            let step = here + offset;
            let hit = ct_eq_str(&self.code_for_step(secret, step), presented);
            // No `break`. The whole window is always evaluated.
            if hit && found.is_none() {
                found = Some(step);
            }
        }
        found
    }
}

impl Default for Totp {
    fn default() -> Self {
        Self::interoperable()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base32_round_trips() {
        for len in 1..=24 {
            let secret = TotpSecret::from_bytes(vec![0xa7u8; len]);
            let text = secret.to_base32();
            assert!(text.chars().all(|c| BASE32.contains(&(c as u8))), "{text}");
            assert_eq!(TotpSecret::from_base32(&text).unwrap(), secret);
        }
    }

    #[test]
    fn the_uri_escapes_an_issuer_a_human_typed() {
        let secret = TotpSecret::from_bytes(b"12345678901234567890".to_vec());
        let uri = secret.otpauth_uri(&Totp::interoperable(), "Brocanth Ltd", "ada@example.com");
        assert!(uri.contains("Brocanth%20Ltd"), "{uri}");
        assert!(uri.contains("ada%40example.com"), "{uri}");
        assert!(uri.contains("algorithm=SHA1&digits=6&period=30"), "{uri}");
    }

    #[test]
    fn absurd_parameters_are_refused_at_construction() {
        assert!(Totp::custom(4, 30, 1).is_err());
        assert!(Totp::custom(11, 30, 1).is_err());
        assert!(Totp::custom(6, 0, 1).is_err());
        assert!(Totp::custom(6, 30, -1).is_err());
    }
}
