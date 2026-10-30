//! The small writers: JSON strings, timestamps, request ids.
//!
//! No serde and no chrono. A log line and a probe body need a string escaper
//! and a UTC timestamp, and both fit on a screen.

use std::collections::hash_map::RandomState;
use std::fmt::Write as _;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Appends `value` to `out` as a JSON string, quotes included.
pub(crate) fn json_string(out: &mut String, value: &str) {
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Appends a finite float as JSON. JSON has no NaN or infinity, so those
/// become `null`.
pub(crate) fn json_f64(out: &mut String, value: f64) {
    if value.is_finite() {
        let _ = write!(out, "{value}");
    } else {
        out.push_str("null");
    }
}

/// Milliseconds as a float with microsecond precision: `1.234`.
pub(crate) fn millis(d: std::time::Duration) -> f64 {
    (d.as_micros() as f64) / 1000.0
}

/// `2026-10-30T12:34:56.789Z`: RFC 3339, UTC, milliseconds.
#[cfg(feature = "logs")]
pub(crate) fn rfc3339_now() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    rfc3339(now.as_secs() as i64, now.subsec_millis())
}

#[cfg_attr(not(feature = "logs"), allow(dead_code))]
pub(crate) fn rfc3339(secs: i64, millis: u32) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

#[cfg_attr(not(feature = "logs"), allow(dead_code))]
/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 to a date.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// The longest incoming request id kept. Longer, and a new one is made.
const MAX_ID: usize = 200;

/// The incoming id when it is safe to log and echo, else `None`.
///
/// Visible ASCII only, no spaces, at most 200 bytes: an id goes into a log
/// line and a response header, and a client must not be able to split
/// either.
pub(crate) fn accepted_id(value: &str) -> Option<&str> {
    let value = value.trim();
    let ok =
        !value.is_empty() && value.len() <= MAX_ID && value.bytes().all(|b| b.is_ascii_graphic());
    ok.then_some(value)
}

/// A new request id: 32 lowercase hex characters.
///
/// Unique, not secret. Two 64-bit SipHash outputs under keys the standard
/// library seeds from the OS, over a process counter and the clock. No `rand`
/// and no `uuid` for a value whose only job is to differ from the last one.
pub(crate) fn new_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let state = RandomState::new();
    let mut a = state.build_hasher();
    a.write_u64(n);
    a.write_u128(nanos);
    let mut b = state.build_hasher();
    b.write_u128(nanos);
    b.write_u64(!n);
    format!("{:016x}{:016x}", a.finish(), b.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_string_is_escaped_so_a_line_stays_one_line() {
        let mut out = String::new();
        json_string(&mut out, "a \"b\"\n\\c\u{1}");
        assert_eq!(out, r#""a \"b\"\n\\c\u0001""#);
    }

    #[test]
    fn timestamps_are_rfc3339_utc() {
        assert_eq!(rfc3339(0, 0), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339(1_798_632_896, 789), "2026-12-30T12:14:56.789Z");
        assert_eq!(rfc3339(951_782_400, 5), "2000-02-29T00:00:00.005Z");
    }

    #[test]
    fn an_id_a_client_could_abuse_is_refused() {
        assert_eq!(accepted_id("abc-123"), Some("abc-123"));
        assert_eq!(accepted_id(" abc "), Some("abc"));
        assert_eq!(accepted_id("a b"), None);
        assert_eq!(accepted_id("a\r\nb"), None);
        assert_eq!(accepted_id(""), None);
        assert_eq!(accepted_id(&"x".repeat(201)), None);
    }

    #[test]
    fn new_ids_are_32_hex_and_differ() {
        let a = new_id();
        let b = new_id();
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}
