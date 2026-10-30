//! ULID-shaped ids: 48 bits of milliseconds and 80 bits of noise, in
//! Crockford base 32, 26 characters.
//!
//! The id must be unique in one project, not unguessable. So the noise comes
//! from the standard library's randomly keyed hasher, and this crate needs no
//! random number crate.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The length of an id.
pub const LEN: usize = 26;

/// A new id.
pub fn new_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let noise = |salt: u64| {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(salt);
        h.write_u64(count);
        h.write_u64(millis);
        h.write_u32(std::process::id());
        h.finish()
    };
    let random = (u128::from(noise(1)) << 16) ^ u128::from(noise(2) & 0xFFFF);
    encode((u128::from(millis & 0xFFFF_FFFF_FFFF) << 80) | (random & ((1u128 << 80) - 1)))
}

/// 128 bits as 26 Crockford base-32 characters, most significant first.
fn encode(value: u128) -> String {
    (0..LEN)
        .map(|i| {
            let shift = 5 * (LEN - 1 - i);
            ALPHABET[((value >> shift) & 31) as usize] as char
        })
        .collect()
}

/// `true` when `text` has the shape of an id.
pub fn is_id(text: &str) -> bool {
    text.len() == LEN
        && text.bytes().all(|b| ALPHABET.contains(&b))
        // 26 characters hold 130 bits; a ULID uses 128, so the first is 0-7.
        && text.as_bytes()[0] <= b'7'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_id_has_the_shape_it_checks_for() {
        for _ in 0..100 {
            let id = new_id();
            assert!(is_id(&id), "{id}");
        }
    }

    #[test]
    fn two_new_ids_differ() {
        let ids: std::collections::BTreeSet<String> = (0..1000).map(|_| new_id()).collect();
        assert_eq!(ids.len(), 1000);
    }

    #[test]
    fn the_shape_check_refuses_near_misses() {
        assert!(is_id("01JBQ7R8V4K2M9X3T6N5P0WZ1A"));
        assert!(!is_id("01JBQ7R8V4K2M9X3T6N5P0WZ1"), "too short");
        assert!(!is_id("01jbq7r8v4k2m9x3t6n5p0wz1a"), "lower case");
        assert!(!is_id("01JBQ7R8V4K2M9X3T6N5P0WZ1I"), "I is not Crockford");
        assert!(!is_id("81JBQ7R8V4K2M9X3T6N5P0WZ1A"), "more than 128 bits");
    }
}
