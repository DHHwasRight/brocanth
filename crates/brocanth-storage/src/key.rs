//! The address of an object, and the one reason path traversal is impossible
//! here.
//!
//! # Content addressing, and what it costs
//!
//! An object's address is the SHA-256 of its bytes. The alternative is a
//! random identifier, and the choice is not obvious, so here is the whole of
//! it.
//!
//! * **Dedupe is free.** Ten thousand users attaching the same company logo
//!   store one blob. A random identifier stores ten thousand.
//! * **The address is the ETag.** A content address *is* a strong validator,
//!   so conditional requests, CDN keys and cache invalidation all fall out
//!   with no extra column and no `Last-Modified` to get wrong. Content that
//!   changed has a different address, so a stale cache is not expressible.
//! * **Corruption is detectable.** Re-hash the bytes and compare. A random
//!   identifier tells you nothing about what is inside it.
//! * **Deletion stops being local.** Two records that share an address share
//!   bytes, so deleting one deletes both. This crate has no database and
//!   cannot count references, so the contract is blunt and stated in
//!   [`Storage::delete`]: it removes the bytes every holder of that address
//!   shares. An application that attaches one upload to two records must count
//!   references itself, or not dedupe — see the next two points.
//! * **A plain content address is an existence oracle.** Anyone holding a file
//!   can compute its SHA-256 and ask the server whether it is stored. For a
//!   leaked document, a dismissal letter or a known photograph that is a real
//!   disclosure: it reveals *that somebody uploaded it*.
//!
//! The last two are why there are three constructors rather than one, and an
//! application picks per deployment:
//!
//! 1. [`ObjectKey::of`] — plain SHA-256. Deduplicates across the whole
//!    deployment, verifiable by anybody, and guessable by anybody who already
//!    holds the file. Right for public assets.
//! 2. [`ObjectKey::keyed`] — HMAC-SHA256 under a [`HashKey`] the deployment
//!    keeps secret. Still deduplicates, still verifiable *by the server*, and
//!    no longer guessable: without the key an attacker cannot turn a file they
//!    hold into the address it would have. This is the default worth choosing
//!    for user uploads, and it is what [`LocalDisk::with_hash_key`] sets.
//! 3. [`ObjectKey::random`] — 256 bits from the OS CSPRNG. No dedupe at all,
//!    so deletion is per-upload again and nothing is shared between two
//!    records. Right when "two users uploaded the same file" is itself the
//!    secret.
//!
//! All three produce the same type and the same thirty-two bytes of width, so
//! the choice changes nothing above this line.
//!
//! # Why traversal is impossible rather than filtered
//!
//! [`LocalDisk`] builds a path from an [`ObjectKey`] and nothing else. An
//! `ObjectKey` holds `[u8; 32]` and renders as exactly sixty-four lowercase
//! hex characters, so every path component the backend can produce is drawn
//! from `0-9a-f` and is of a fixed length. `.`, `/`, `\`, `%`, a NUL and a
//! drive letter are not expressible in the type, so no amount of cleverness in
//! a filename, a URL or a header reaches a parent directory.
//!
//! There is no constructor from a path, no `Deref<Target = str>`, and
//! [`ObjectKey::parse`] accepts sixty-four hex characters or nothing. A
//! user-supplied name is a [`Filename`], it is metadata, and it never touches
//! the filesystem.
//!
//! [`Filename`]: crate::meta::Filename
//! [`LocalDisk`]: crate::local::LocalDisk
//! [`LocalDisk::with_hash_key`]: crate::local::LocalDisk::with_hash_key
//! [`Storage::delete`]: crate::storage::Storage::delete

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

use crate::error::StorageError;

/// The width of an address, in bytes.
pub const KEY_BYTES: usize = 32;

/// The width of an address, in hex characters.
pub const KEY_HEX_LEN: usize = KEY_BYTES * 2;

/// The secret that turns content addressing into *keyed* content addressing.
///
/// Hold one per deployment, keep it with the other secrets, and understand
/// that rotating it re-addresses every object stored under it: the old
/// addresses stay valid and still resolve, but the same bytes uploaded again
/// get a new address and stop deduplicating against the old one. Rotate on a
/// migration, not on a schedule.
#[derive(Clone)]
pub struct HashKey([u8; KEY_BYTES]);

impl HashKey {
    /// Takes thirty-two bytes as the key.
    pub fn from_bytes(bytes: [u8; KEY_BYTES]) -> Self {
        Self(bytes)
    }

    /// Derives the key from an arbitrary secret.
    ///
    /// Domain-separated, so the same deployment secret can also sign file
    /// tokens without one use being able to forge the other.
    pub fn from_secret(secret: &[u8]) -> Self {
        let mut h = Sha256::new();
        h.update(b"brocanth-storage/address-key/v1");
        h.update(secret);
        Self(h.finalize().into())
    }

    /// A fresh key from the OS CSPRNG.
    ///
    /// # Errors
    /// Returns [`StorageError::Backend`] when the operating system refuses to
    /// supply randomness. There is no fallback on purpose.
    pub fn generate() -> Result<Self, StorageError> {
        let mut bytes = [0u8; KEY_BYTES];
        getrandom::fill(&mut bytes)
            .map_err(|e| StorageError::backend(format!("no OS randomness: {e}")))?;
        Ok(Self(bytes))
    }
}

impl std::fmt::Debug for HashKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HashKey(<redacted>)")
    }
}

/// Where an object lives, and the only thing a backend turns into a path.
///
/// Thirty-two bytes. See the module documentation for the three ways to mint
/// one and for why a path cannot be smuggled through this type.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectKey([u8; KEY_BYTES]);

impl ObjectKey {
    /// The SHA-256 of the content.
    pub fn of(content: &[u8]) -> Self {
        let mut h = Sha256::new();
        h.update(content);
        Self(h.finalize().into())
    }

    /// The HMAC-SHA256 of the content under a deployment secret.
    ///
    /// Deduplicates exactly as [`ObjectKey::of`] does, and is unguessable to
    /// anyone without the key. See the module documentation.
    pub fn keyed(key: &HashKey, content: &[u8]) -> Self {
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(&key.0)
            .expect("HMAC accepts a key of any length");
        mac.update(content);
        Self(mac.finalize().into_bytes().into())
    }

    /// Two hundred and fifty-six bits from the OS CSPRNG, with no relation to
    /// the content.
    ///
    /// # Errors
    /// Returns [`StorageError::Backend`] when the operating system refuses to
    /// supply randomness.
    pub fn random() -> Result<Self, StorageError> {
        let mut bytes = [0u8; KEY_BYTES];
        getrandom::fill(&mut bytes)
            .map_err(|e| StorageError::backend(format!("no OS randomness: {e}")))?;
        Ok(Self(bytes))
    }

    /// Takes thirty-two bytes as an address, for a caller that already has
    /// them — a row, a token payload.
    pub fn from_bytes(bytes: [u8; KEY_BYTES]) -> Self {
        Self(bytes)
    }

    /// Reads an address back from exactly sixty-four lowercase hex characters.
    ///
    /// This is the *only* way a string becomes an address, and it is where a
    /// URL segment is checked. Uppercase is rejected along with everything
    /// else, which is strictness rather than safety: the bytes would be the
    /// same, but two spellings of one address are two cache entries, two CDN
    /// objects and two rows that should have been one.
    ///
    /// # Errors
    /// Returns [`StorageError::InvalidKey`] for any other text whatsoever,
    /// which includes every form of `../`, every encoding of it, an absolute
    /// path, a NUL byte and a UNC share.
    pub fn parse(text: &str) -> Result<Self, StorageError> {
        let bytes = text.as_bytes();
        if bytes.len() != KEY_HEX_LEN {
            return Err(StorageError::InvalidKey);
        }
        let mut out = [0u8; KEY_BYTES];
        for (i, pair) in bytes.as_chunks::<2>().0.iter().enumerate() {
            let hi = hex_digit(pair[0]).ok_or(StorageError::InvalidKey)?;
            let lo = hex_digit(pair[1]).ok_or(StorageError::InvalidKey)?;
            out[i] = (hi << 4) | lo;
        }
        Ok(Self(out))
    }

    /// The thirty-two bytes.
    pub fn as_bytes(&self) -> &[u8; KEY_BYTES] {
        &self.0
    }

    /// The sixty-four character lowercase hex form. This is the canonical
    /// spelling, the URL segment and the path component.
    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(KEY_HEX_LEN);
        for b in self.0 {
            s.push(HEX[(b >> 4) as usize] as char);
            s.push(HEX[(b & 0x0f) as usize] as char);
        }
        s
    }

    /// A strong HTTP entity tag, quotes included.
    ///
    /// Free, and correct by construction: the address changes if and only if
    /// the bytes change. The serving half needs no `Last-Modified` beside it,
    /// which is why [`ObjectMeta`](crate::meta::ObjectMeta) carries no
    /// timestamp.
    pub fn etag(&self) -> String {
        format!("\"{}\"", self.to_hex())
    }

    /// Whether these bytes hash to this address, under plain content
    /// addressing.
    ///
    /// Answers `false` for an address minted by [`ObjectKey::keyed`] or
    /// [`ObjectKey::random`], which is correct: this is the integrity check for
    /// the addressing scheme that supports one.
    pub fn verifies(&self, content: &[u8]) -> bool {
        use subtle::ConstantTimeEq;
        let computed = Self::of(content);
        computed.0.ct_eq(&self.0).into()
    }
}

const HEX: &[u8; 16] = b"0123456789abcdef";

fn hex_digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

impl std::fmt::Display for ObjectKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl std::fmt::Debug for ObjectKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ObjectKey({})", self.to_hex())
    }
}

impl std::str::FromStr for ObjectKey {
    type Err = StorageError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}
