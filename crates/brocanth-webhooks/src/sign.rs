//! Signatures: how a sender proves a delivery is its own, in three schemes.
//!
//! | Scheme | Header | Signed bytes | Replay window |
//! |---|---|---|---|
//! | [`Scheme::Standard`] (Standard Webhooks, svix) | `webhook-id`, `webhook-timestamp`, `webhook-signature: v1,<base64>` | `{id}.{timestamp}.{body}` | yes |
//! | [`Scheme::Stripe`] | `stripe-signature: t=<unix>,v1=<hex>` | `{t}.{body}` | yes |
//! | [`Scheme::Hmac`] (GitHub and most others) | one header, `<prefix><hex>` | the body | no |
//!
//! Every scheme is HMAC-SHA256, and every comparison is constant time. A
//! scheme with a timestamp is checked against a window: a delivery signed
//! more than the window ago, or that far in the future, is refused, so a
//! captured request cannot be sent again next week. A scheme with no
//! timestamp has no window, and only the idempotency key stops a replay.
//! That is a property of the sender's scheme, not a choice made here.
//!
//! The outgoing side signs with [`Scheme::Standard`] through
//! [`sign_standard`], so a receiver built on any Standard Webhooks library
//! verifies it.
//!
//! This module knows no HTTP type. A header is read through a closure, so
//! it works on `http::HeaderMap`, on a test's pairs, or on anything else.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

/// The prefix of a Standard Webhooks secret.
pub const SECRET_PREFIX: &str = "whsec_";

/// The default replay window: five minutes, the value Stripe and Standard
/// Webhooks both recommend.
pub const DEFAULT_TOLERANCE: i64 = 300;

/// How a source signs its deliveries.
#[derive(Clone)]
pub enum Scheme {
    /// Standard Webhooks: `webhook-id`, `webhook-timestamp` and
    /// `webhook-signature`. The id is also the idempotency key.
    Standard {
        /// The key, already decoded from `whsec_<base64>`.
        key: Vec<u8>,
    },
    /// Stripe: `stripe-signature: t=<unix>,v1=<hex>`. The idempotency key is
    /// the event's `id` field.
    Stripe {
        /// The endpoint secret, `whsec_...`, used as it is.
        key: Vec<u8>,
    },
    /// One header holding the hex MAC of the body, after a fixed prefix.
    Hmac {
        /// The header, lower case: `x-hub-signature-256`.
        header: String,
        /// What comes before the hex: `sha256=`, or empty.
        prefix: String,
        /// The shared secret, used as it is.
        key: Vec<u8>,
        /// A header that names the delivery, for the idempotency key:
        /// `x-github-delivery`. `None` falls back to the body.
        id_header: Option<String>,
    },
}

impl std::fmt::Debug for Scheme {
    /// Names the scheme and never prints the key.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Scheme::Standard { .. } => "Scheme::Standard",
            Scheme::Stripe { .. } => "Scheme::Stripe",
            Scheme::Hmac { .. } => "Scheme::Hmac",
        })
    }
}

impl Scheme {
    /// Standard Webhooks, from a secret `whsec_<base64>`.
    ///
    /// # Errors
    /// The secret has no `whsec_` prefix, or what follows is not base64, or
    /// it decodes to fewer than 16 bytes.
    pub fn standard(secret: &str) -> Result<Self, SecretError> {
        Ok(Scheme::Standard {
            key: decode_secret(secret)?,
        })
    }

    /// Stripe, from the endpoint's signing secret.
    ///
    /// # Errors
    /// The secret is empty.
    pub fn stripe(secret: &str) -> Result<Self, SecretError> {
        if secret.trim().is_empty() {
            return Err(SecretError::Empty);
        }
        Ok(Scheme::Stripe {
            key: secret.as_bytes().to_vec(),
        })
    }

    /// GitHub: `x-hub-signature-256: sha256=<hex>`, and `x-github-delivery`
    /// as the idempotency key.
    ///
    /// # Errors
    /// The secret is empty.
    pub fn github(secret: &str) -> Result<Self, SecretError> {
        Self::hmac_sha256("x-hub-signature-256", "sha256=", secret)
            .map(|scheme| scheme.with_id_header("x-github-delivery"))
    }

    /// Any sender that puts `<prefix><hex HMAC-SHA256 of the body>` in one
    /// header.
    ///
    /// # Errors
    /// The secret is empty.
    pub fn hmac_sha256(header: &str, prefix: &str, secret: &str) -> Result<Self, SecretError> {
        if secret.trim().is_empty() {
            return Err(SecretError::Empty);
        }
        Ok(Scheme::Hmac {
            header: header.to_ascii_lowercase(),
            prefix: prefix.to_string(),
            key: secret.as_bytes().to_vec(),
            id_header: None,
        })
    }

    /// The header that names a delivery, for [`Scheme::Hmac`]. Ignored by
    /// the other schemes, which carry their own id.
    #[must_use]
    pub fn with_id_header(mut self, name: &str) -> Self {
        if let Scheme::Hmac { id_header, .. } = &mut self {
            *id_header = Some(name.to_ascii_lowercase());
        }
        self
    }

    /// Checks one delivery.
    ///
    /// `header` reads a request header by its lower-case name. `now` is Unix
    /// seconds, and `tolerance` the replay window in seconds.
    ///
    /// # Errors
    /// A [`Refused`] that says which check failed. The caller answers 401
    /// for each one and must not echo the reason to the sender in detail.
    pub fn verify<'h>(
        &self,
        header: impl Fn(&str) -> Option<&'h str>,
        body: &[u8],
        now: i64,
        tolerance: i64,
    ) -> Result<Verified, Refused> {
        match self {
            Scheme::Standard { key } => {
                let id = header("webhook-id").ok_or(Refused::Missing("webhook-id"))?;
                let stamp =
                    header("webhook-timestamp").ok_or(Refused::Missing("webhook-timestamp"))?;
                let signatures =
                    header("webhook-signature").ok_or(Refused::Missing("webhook-signature"))?;
                let timestamp: i64 = stamp
                    .trim()
                    .parse()
                    .map_err(|_| Refused::Malformed("webhook-timestamp"))?;
                in_window(timestamp, now, tolerance)?;
                let expected = mac(
                    key,
                    &[id.as_bytes(), b".", stamp.trim().as_bytes(), b".", body],
                );
                let matched = signatures
                    .split(' ')
                    .filter_map(|one| one.strip_prefix("v1,"))
                    .filter_map(|b64| B64.decode(b64).ok())
                    .any(|given| given.ct_eq(&expected).into());
                if !matched {
                    return Err(Refused::BadSignature);
                }
                Ok(Verified {
                    id: Some(id.to_string()),
                    timestamp: Some(timestamp),
                })
            }
            Scheme::Stripe { key } => {
                let value =
                    header("stripe-signature").ok_or(Refused::Missing("stripe-signature"))?;
                let mut stamp = None;
                let mut given = Vec::new();
                for part in value.split(',') {
                    match part.trim().split_once('=') {
                        Some(("t", t)) => stamp = Some(t),
                        Some(("v1", hex)) => given.push(hex),
                        _ => {}
                    }
                }
                let stamp = stamp.ok_or(Refused::Malformed("stripe-signature"))?;
                let timestamp: i64 = stamp
                    .parse()
                    .map_err(|_| Refused::Malformed("stripe-signature"))?;
                in_window(timestamp, now, tolerance)?;
                let expected = mac(key, &[stamp.as_bytes(), b".", body]);
                let matched = given
                    .iter()
                    .filter_map(|hex| decode_hex(hex))
                    .any(|given| given.ct_eq(&expected).into());
                if !matched {
                    return Err(Refused::BadSignature);
                }
                Ok(Verified {
                    id: None,
                    timestamp: Some(timestamp),
                })
            }
            Scheme::Hmac {
                header: name,
                prefix,
                key,
                id_header,
            } => {
                let value = header(name).ok_or(Refused::MissingNamed(name.clone()))?;
                let hex = value
                    .trim()
                    .strip_prefix(prefix.as_str())
                    .ok_or(Refused::MalformedNamed(name.clone()))?;
                let given = decode_hex(hex).ok_or(Refused::MalformedNamed(name.clone()))?;
                let expected = mac(key, &[body]);
                if !bool::from(given.ct_eq(&expected)) {
                    return Err(Refused::BadSignature);
                }
                Ok(Verified {
                    id: id_header.as_deref().and_then(header).map(str::to_string),
                    timestamp: None,
                })
            }
        }
    }
}

/// What a valid signature told us.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verified {
    /// The delivery's id, when the scheme carries one in a header.
    pub id: Option<String>,
    /// When the sender signed it, when the scheme says.
    pub timestamp: Option<i64>,
}

/// Why a delivery was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// A header the scheme needs is absent.
    Missing(&'static str),
    /// The same, for a header the source configured.
    MissingNamed(String),
    /// A header is there and cannot be read.
    Malformed(&'static str),
    /// The same, for a header the source configured.
    MalformedNamed(String),
    /// The timestamp is outside the replay window.
    Stale {
        /// How far from now, in seconds. Negative is in the future.
        age: i64,
    },
    /// No signature matched.
    BadSignature,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::Missing(name) => write!(f, "the `{name}` header is missing"),
            Refused::MissingNamed(name) => write!(f, "the `{name}` header is missing"),
            Refused::Malformed(name) => write!(f, "the `{name}` header cannot be read"),
            Refused::MalformedNamed(name) => write!(f, "the `{name}` header cannot be read"),
            Refused::Stale { age } => write!(
                f,
                "the delivery was signed {age}s from now, outside the replay window"
            ),
            Refused::BadSignature => f.write_str("no signature matches the body"),
        }
    }
}

impl std::error::Error for Refused {}

/// A secret that cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretError {
    /// No bytes at all: every MAC would be forgeable.
    Empty,
    /// Not `whsec_<base64>`.
    NotStandard,
    /// Fewer than 16 bytes once decoded.
    TooShort,
}

impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecretError::Empty => f.write_str(
                "the webhook secret is empty\nhelp: copy the signing secret from the sender's dashboard",
            ),
            SecretError::NotStandard => f.write_str(
                "the webhook secret is not `whsec_<base64>`\nhelp: a Standard Webhooks secret starts with `whsec_`; make one with `brocanth_webhooks::sign::new_secret()`",
            ),
            SecretError::TooShort => f.write_str(
                "the webhook secret is shorter than 16 bytes\nhelp: make one with `brocanth_webhooks::sign::new_secret()`",
            ),
        }
    }
}

impl std::error::Error for SecretError {}

/// The `webhook-signature` value for one delivery: `v1,<base64>`.
///
/// `key` is the decoded secret ([`decode_secret`]). The id stays the same
/// across retries of one delivery, so a receiver can drop a duplicate; the
/// timestamp is the time of this attempt.
pub fn sign_standard(key: &[u8], id: &str, timestamp: i64, body: &[u8]) -> String {
    let stamp = timestamp.to_string();
    let tag = mac(key, &[id.as_bytes(), b".", stamp.as_bytes(), b".", body]);
    format!("v1,{}", B64.encode(tag))
}

/// The `stripe-signature` value for one delivery, as Stripe would send it.
/// For tests of a Stripe receiver.
pub fn sign_stripe(secret: &str, timestamp: i64, body: &[u8]) -> String {
    let stamp = timestamp.to_string();
    let tag = mac(secret.as_bytes(), &[stamp.as_bytes(), b".", body]);
    format!("t={stamp},v1={}", encode_hex(&tag))
}

/// `<prefix><hex>` for [`Scheme::Hmac`]. For tests of such a receiver.
pub fn sign_hmac(secret: &str, prefix: &str, body: &[u8]) -> String {
    format!("{prefix}{}", encode_hex(&mac(secret.as_bytes(), &[body])))
}

/// The key inside a Standard Webhooks secret.
///
/// # Errors
/// See [`Scheme::standard`].
pub fn decode_secret(secret: &str) -> Result<Vec<u8>, SecretError> {
    let secret = secret.trim();
    if secret.is_empty() {
        return Err(SecretError::Empty);
    }
    let b64 = secret
        .strip_prefix(SECRET_PREFIX)
        .ok_or(SecretError::NotStandard)?;
    let key = B64.decode(b64).map_err(|_| SecretError::NotStandard)?;
    if key.len() < 16 {
        return Err(SecretError::TooShort);
    }
    Ok(key)
}

/// A new Standard Webhooks secret: `whsec_` and 32 random bytes in base64.
///
/// # Panics
/// When the operating system has no random source, which no supported
/// platform lacks.
pub fn new_secret() -> String {
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).expect("the OS random source");
    format!("{SECRET_PREFIX}{}", B64.encode(key))
}

/// A new message id: `msg_` and 16 random bytes in hex.
///
/// # Panics
/// When the operating system has no random source.
pub fn new_message_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the OS random source");
    format!("msg_{}", encode_hex(&bytes))
}

/// The hex SHA-256 of some bytes. The idempotency key of a delivery whose
/// scheme names none.
pub fn digest_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    encode_hex(&Sha256::digest(bytes))
}

fn in_window(timestamp: i64, now: i64, tolerance: i64) -> Result<(), Refused> {
    let age = now.saturating_sub(timestamp);
    if age.abs() > tolerance {
        return Err(Refused::Stale { age });
    }
    Ok(())
}

fn mac(key: &[u8], parts: &[&[u8]]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes a key of any length");
    for part in parts {
        mac.update(part);
    }
    mac.finalize().into_bytes().to_vec()
}

fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[usize::from(byte >> 4)] as char);
        out.push(DIGITS[usize::from(byte & 0xf)] as char);
    }
    out
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    let text = text.trim().as_bytes();
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let digit = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    };
    text.chunks(2)
        .map(|pair| Some(digit(pair[0])? << 4 | digit(pair[1])?))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_700_000_000;

    fn pairs<'a>(list: &'a [(&'a str, String)]) -> impl Fn(&str) -> Option<&'a str> {
        move |name| {
            list.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.as_str())
        }
    }

    /// The test vector from the Standard Webhooks specification.
    #[test]
    fn standard_matches_the_specification_vector() {
        let key = decode_secret("whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw").unwrap();
        let body = br#"{"test": 2432232314}"#;
        let signature = sign_standard(&key, "msg_p5jXN8AQM9LWM0D4loKWxJek", 1_614_265_330, body);
        assert_eq!(signature, "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE=");
    }

    #[test]
    fn standard_accepts_its_own_signature_and_returns_the_id() {
        let secret = new_secret();
        let scheme = Scheme::standard(&secret).unwrap();
        let key = decode_secret(&secret).unwrap();
        let body = b"{\"a\":1}";
        let headers = [
            ("webhook-id", "msg_1".to_string()),
            ("webhook-timestamp", NOW.to_string()),
            (
                "webhook-signature",
                format!("v1,bm9wZQ== {}", sign_standard(&key, "msg_1", NOW, body)),
            ),
        ];
        let verified = scheme.verify(pairs(&headers), body, NOW + 10, 300).unwrap();
        assert_eq!(verified.id.as_deref(), Some("msg_1"));
        assert_eq!(verified.timestamp, Some(NOW));
    }

    #[test]
    fn standard_refuses_a_changed_body_an_old_stamp_and_a_missing_header() {
        let secret = new_secret();
        let scheme = Scheme::standard(&secret).unwrap();
        let key = decode_secret(&secret).unwrap();
        let headers = [
            ("webhook-id", "msg_1".to_string()),
            ("webhook-timestamp", NOW.to_string()),
            (
                "webhook-signature",
                sign_standard(&key, "msg_1", NOW, b"{}"),
            ),
        ];
        assert_eq!(
            scheme.verify(pairs(&headers), b"{\"x\":1}", NOW, 300),
            Err(Refused::BadSignature)
        );
        assert_eq!(
            scheme.verify(pairs(&headers), b"{}", NOW + 301, 300),
            Err(Refused::Stale { age: 301 })
        );
        assert_eq!(
            scheme.verify(pairs(&headers[..2]), b"{}", NOW, 300),
            Err(Refused::Missing("webhook-signature"))
        );
    }

    #[test]
    fn stripe_accepts_a_signature_and_refuses_a_replay_outside_the_window() {
        let scheme = Scheme::stripe("whsec_test").unwrap();
        let body = br#"{"id":"evt_1"}"#;
        let headers = [("stripe-signature", sign_stripe("whsec_test", NOW, body))];
        assert!(scheme.verify(pairs(&headers), body, NOW, 300).is_ok());
        assert!(matches!(
            scheme.verify(pairs(&headers), body, NOW + 3600, 300),
            Err(Refused::Stale { .. })
        ));
        let forged = [("stripe-signature", sign_stripe("other", NOW, body))];
        assert_eq!(
            scheme.verify(pairs(&forged), body, NOW, 300),
            Err(Refused::BadSignature)
        );
    }

    #[test]
    fn github_reads_the_prefix_and_the_delivery_header() {
        let scheme = Scheme::github("s3cret").unwrap();
        let body = b"{\"zen\":\"hi\"}";
        let headers = [
            ("x-hub-signature-256", sign_hmac("s3cret", "sha256=", body)),
            ("x-github-delivery", "d-1".to_string()),
        ];
        let verified = scheme.verify(pairs(&headers), body, NOW, 300).unwrap();
        assert_eq!(verified.id.as_deref(), Some("d-1"));
        let bare = [("x-hub-signature-256", sign_hmac("s3cret", "", body))];
        assert_eq!(
            scheme.verify(pairs(&bare), body, NOW, 300),
            Err(Refused::MalformedNamed("x-hub-signature-256".into()))
        );
    }

    #[test]
    fn a_bad_secret_is_refused_with_a_help_line() {
        assert_eq!(
            Scheme::standard("nope").unwrap_err(),
            SecretError::NotStandard
        );
        assert_eq!(
            Scheme::standard("whsec_AAAA").unwrap_err(),
            SecretError::TooShort
        );
        assert!(Scheme::stripe(" ")
            .unwrap_err()
            .to_string()
            .contains("help:"));
    }

    #[test]
    fn debug_never_prints_the_key() {
        let scheme = Scheme::stripe("very-secret").unwrap();
        assert!(!format!("{scheme:?}").contains("very-secret"));
    }
}
