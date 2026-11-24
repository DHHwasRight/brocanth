//! The service secret and the signatures it makes (W3, W4).

use std::fmt;
use std::sync::Arc;

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// What every signature starts with: the protocol and its version, so a
/// signature of another protocol over the same secret never verifies here.
const REQUEST: &[u8] = b"brocanth-backoffice/1 request\n";
/// What a reply's signature starts with, so a request's signature is never
/// a reply's.
const REPLY: &[u8] = b"brocanth-backoffice/1 reply\n";

/// The secret both binaries share: `[backoffice] service_secret`.
///
/// Its `Debug` prints no byte of it.
#[derive(Clone)]
pub struct ServiceSecret(Arc<[u8]>);

impl fmt::Debug for ServiceSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ServiceSecret(<redacted>)")
    }
}

impl ServiceSecret {
    /// The fewest bytes a secret may have.
    pub const MIN_LEN: usize = 32;

    /// The secret in `text`, without the whitespace around it: a file that
    /// holds it usually ends with a newline.
    ///
    /// # Errors
    ///
    /// When it is shorter than [`ServiceSecret::MIN_LEN`] bytes, with a
    /// `help:` line. The message never holds the secret.
    pub fn parse(text: &str) -> Result<ServiceSecret, String> {
        let text = text.trim();
        if text.len() < Self::MIN_LEN {
            return Err(format!(
                "`[backoffice] service_secret` has {} bytes, and it needs {} bytes or more\n\
                 help: make one with `openssl rand -hex 32` and give it to both binaries with \
                 --backoffice.service_secret_file=PATH",
                text.len(),
                Self::MIN_LEN
            ));
        }
        Ok(ServiceSecret(Arc::from(text.as_bytes())))
    }

    fn mac(&self) -> HmacSha256 {
        HmacSha256::new_from_slice(&self.0).expect("HMAC takes a key of any length")
    }

    /// The MAC of a request, before its finalisation.
    fn request(&self, time: &str, nonce: &str, path: &str, body: &[u8]) -> HmacSha256 {
        // One field per line: the time is digits, the nonce hex, and a
        // path holds no newline, so no two requests share a text.
        let mut mac = self.mac();
        mac.update(REQUEST);
        for part in [time, nonce, path] {
            mac.update(part.as_bytes());
            mac.update(b"\n");
        }
        mac.update(body);
        mac
    }

    /// The MAC of a reply to the request with `nonce`.
    fn reply(&self, nonce: &str, status: u16, body: &[u8]) -> HmacSha256 {
        let mut mac = self.mac();
        mac.update(REPLY);
        mac.update(nonce.as_bytes());
        mac.update(b"\n");
        mac.update(status.to_string().as_bytes());
        mac.update(b"\n");
        mac.update(body);
        mac
    }

    /// Whether `signature` (hex) signs this request. Compared in constant
    /// time.
    pub(crate) fn verifies(
        &self,
        time: &str,
        nonce: &str,
        path: &str,
        body: &[u8],
        signature: &str,
    ) -> bool {
        let Some(signature) = unhex(signature) else {
            return false;
        };
        self.request(time, nonce, path, body)
            .verify_slice(&signature)
            .is_ok()
    }

    /// The signature (hex) of a reply to the request with `nonce`.
    pub(crate) fn sign_reply(&self, nonce: &str, status: u16, body: &[u8]) -> String {
        hex(&self.reply(nonce, status, body).finalize().into_bytes())
    }

    /// Whether `signature` (hex) signs this reply. Compared in constant
    /// time.
    pub(crate) fn verifies_reply(
        &self,
        nonce: &str,
        status: u16,
        body: &[u8],
        signature: &str,
    ) -> bool {
        let Some(signature) = unhex(signature) else {
            return false;
        };
        self.reply(nonce, status, body)
            .verify_slice(&signature)
            .is_ok()
    }
}

/// The three headers of one signed request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signed {
    /// [`super::TIME_HEADER`]: Unix seconds.
    pub time: String,
    /// [`super::NONCE_HEADER`]: 128 random bits, in hex.
    pub nonce: String,
    /// [`super::SIGNATURE_HEADER`]: HMAC-SHA256, in hex.
    pub signature: String,
}

/// Signs a request to `path` with `body`, at `time` (Unix seconds), with a
/// fresh nonce.
///
/// The client calls it for every request. It is public so that a test can
/// sign a request with a time of its choice.
pub fn sign(secret: &ServiceSecret, path: &str, body: &[u8], time: u64) -> Signed {
    let time = time.to_string();
    let nonce = random_hex::<16>();
    let signature = hex(&secret
        .request(&time, &nonce, path, body)
        .finalize()
        .into_bytes());
    Signed {
        time,
        nonce,
        signature,
    }
}

/// `N` random bytes, in hex.
pub(crate) fn random_hex<const N: usize>() -> String {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).expect("the OS random source");
    hex(&bytes)
}

/// The current time in Unix seconds.
pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[usize::from(byte >> 4)] as char);
        out.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    out
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let digit = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    };
    text.as_bytes()
        .chunks(2)
        .map(|pair| Some(digit(pair[0])? << 4 | digit(pair[1])?))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret() -> ServiceSecret {
        ServiceSecret::parse("0123456789abcdef0123456789abcdef\n").expect("long enough")
    }

    #[test]
    fn a_signature_verifies_its_own_request_and_nothing_else() {
        let secret = secret();
        let signed = sign(&secret, "/p", b"body", 100);
        let ok = |time: &str, nonce: &str, path: &str, body: &[u8]| {
            secret.verifies(time, nonce, path, body, &signed.signature)
        };
        assert!(ok(&signed.time, &signed.nonce, "/p", b"body"));
        assert!(!ok("101", &signed.nonce, "/p", b"body"), "another time");
        assert!(!ok(&signed.time, "00", "/p", b"body"), "another nonce");
        assert!(
            !ok(&signed.time, &signed.nonce, "/q", b"body"),
            "another path"
        );
        assert!(
            !ok(&signed.time, &signed.nonce, "/p", b"bodies"),
            "another body"
        );
        let other = ServiceSecret::parse("fedcba9876543210fedcba9876543210").expect("long");
        assert!(!other.verifies(
            &signed.time,
            &signed.nonce,
            "/p",
            b"body",
            &signed.signature
        ));
        assert!(!secret.verifies(&signed.time, &signed.nonce, "/p", b"body", "zz"));
    }

    #[test]
    fn a_reply_signature_is_bound_to_the_nonce_and_the_status() {
        let secret = secret();
        let signature = secret.sign_reply("n1", 200, b"{}");
        assert!(secret.verifies_reply("n1", 200, b"{}", &signature));
        assert!(!secret.verifies_reply("n2", 200, b"{}", &signature));
        assert!(!secret.verifies_reply("n1", 401, b"{}", &signature));
    }

    #[test]
    fn nonces_differ_and_the_debug_hides_the_secret() {
        assert_ne!(random_hex::<16>(), random_hex::<16>());
        assert_eq!(random_hex::<16>().len(), 32);
        let printed = format!("{:?}", secret());
        assert!(!printed.contains("0123"), "{printed}");
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(unhex(&hex(&[0, 15, 16, 255])), Some(vec![0, 15, 16, 255]));
        assert_eq!(unhex("abc"), None);
    }
}
