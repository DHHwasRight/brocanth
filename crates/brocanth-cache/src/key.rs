//! Namespaces and keys: how two features sharing one cache stay out of each
//! other's way, and how a deploy throws a whole class of entries away.
//!
//! # Why a namespace is a column and not a prefix
//!
//! The obvious design is one `TEXT` key column holding `"sessions:user:7"`,
//! with the part before the colon as the namespace. It is wrong in two ways
//! that matter.
//!
//! **Collision stops being structural.** With a prefix, a feature that writes
//! the key `"user:7"` under namespace `"sessions"` and a feature that writes
//! the key `"7"` under namespace `"sessions:user"` produce the same string.
//! Nothing in the type system or the database notices; one feature reads the
//! other's bytes and deserialises them as its own. With two columns there is
//! no string to collide: `('sessions', 'user:7')` and `('sessions:user', '7')`
//! are different rows, always.
//!
//! **Clearing a namespace stops being cheap.** `DELETE WHERE k LIKE 'views:%'`
//! is a prefix scan that the planner may or may not turn into an index range,
//! and it is a pattern match on data you did not escape. `DELETE WHERE
//! namespace = $1` is an equality on the leading column of the primary key: an
//! index range, every time, with a bound parameter.
//!
//! # Two ways to invalidate a class of entries, and the cheap one is better
//!
//! [`Cache::clear`](crate::Cache::clear) deletes a namespace. It is correct,
//! it is immediate, and it is `O(entries)` — a `DELETE` over a namespace with
//! a million rows is a million dead tuples for autovacuum to deal with, on a
//! table that is already the write-hottest thing in your database.
//!
//! [`Namespace::versioned`] is the other answer and usually the right one.
//! Build the namespace from a constant your deploy changes:
//!
//! ```
//! # use brocanth_cache::Namespace;
//! # fn main() -> Result<(), brocanth_cache::CacheError> {
//! const RENDER_VERSION: &str = "v7";
//! let views = Namespace::versioned("views", RENDER_VERSION)?;
//! assert_eq!(views.as_str(), "views:v7");
//! # Ok(())
//! # }
//! ```
//!
//! Bumping `RENDER_VERSION` invalidates every entry in the old namespace in
//! the time it takes to deploy, because nothing reads them any more. Not one
//! row is deleted and not one dead tuple is made; the old rows reach their TTL
//! and the sweep takes them away in its own time. This is the trick worth
//! learning: in a cache, *not reading a row* is a cheaper invalidation than
//! deleting it.
//!
//! # What is allowed in a name
//!
//! A namespace is 1 to [`MAX_NAMESPACE_BYTES`] bytes of
//! `a-z A-Z 0-9 _ . : -`. A key is 1 to [`MAX_KEY_BYTES`] bytes of anything
//! that is not an ASCII control character.
//!
//! The rules are not about safety — every statement binds parameters, so
//! nothing here can be injected — they are about the moment at two in the
//! morning when you open `psql` and run `SELECT namespace, k FROM
//! brocanth_cache`. A key with a newline in it ruins that, and a key that is
//! four kilobytes of user input ruins it differently. The byte caps also keep
//! the pair inside Postgres' ~2700-byte limit for a btree index entry, which
//! is the primary key, so an over-long key is a rejected `Key` here rather
//! than a failed `INSERT` later.

use crate::error::{CacheError, Result};

/// The most bytes a namespace name may be.
pub const MAX_NAMESPACE_BYTES: usize = 64;

/// The most bytes a key may be.
///
/// Together with [`MAX_NAMESPACE_BYTES`] this stays under Postgres' limit on
/// the size of one btree index entry, which the `(namespace, k)` primary key
/// is. A key longer than this is nearly always a key that should have been
/// hashed by the caller.
pub const MAX_KEY_BYTES: usize = 1024;

/// A class of cache entries, owned by one feature.
///
/// Cheap to clone and cheap to keep in a `static`. Build one once, near the
/// code that uses it, and make keys from it with [`Namespace::key`].
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Namespace(String);

impl Namespace {
    /// Builds a namespace, checking it.
    ///
    /// # Errors
    /// [`CacheError::InvalidNamespace`] when the name is empty, too long, or
    /// holds a character outside `a-z A-Z 0-9 _ . : -`.
    pub fn new(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        let reason = if name.is_empty() {
            Some("it is empty")
        } else if name.len() > MAX_NAMESPACE_BYTES {
            Some("it is longer than 64 bytes")
        } else if !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'-'))
        {
            Some("it may only hold letters, digits, `_`, `.`, `:` and `-`")
        } else {
            None
        };
        match reason {
            Some(reason) => Err(CacheError::InvalidNamespace { name, reason }),
            None => Ok(Namespace(name)),
        }
    }

    /// Builds the namespace `"{name}:{version}"`.
    ///
    /// The cheap way to throw a whole class of entries away: change the
    /// version, deploy, and nothing reads the old rows again. See the module
    /// documentation for why this usually beats
    /// [`Cache::clear`](crate::Cache::clear).
    ///
    /// # Errors
    /// [`CacheError::InvalidNamespace`] when the joined name breaks a rule.
    pub fn versioned(name: &str, version: &str) -> Result<Self> {
        Self::new(format!("{name}:{version}"))
    }

    /// The name, as stored.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// A key in this namespace.
    ///
    /// # Errors
    /// [`CacheError::InvalidKey`] when the key is empty, over
    /// [`MAX_KEY_BYTES`], or holds an ASCII control character.
    pub fn key(&self, key: impl Into<String>) -> Result<Key> {
        Key::new(self.clone(), key)
    }
}

impl std::fmt::Display for Namespace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One cache entry's address: a namespace and a key within it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key {
    namespace: Namespace,
    key: String,
}

impl Key {
    /// Builds a key, checking it.
    ///
    /// # Errors
    /// [`CacheError::InvalidKey`] when the key is empty, over
    /// [`MAX_KEY_BYTES`], or holds an ASCII control character.
    pub fn new(namespace: Namespace, key: impl Into<String>) -> Result<Self> {
        let key = key.into();
        let reason = if key.is_empty() {
            Some("it is empty")
        } else if key.len() > MAX_KEY_BYTES {
            Some("it is longer than 1024 bytes; hash it before caching on it")
        } else if key.chars().any(|c| c.is_control()) {
            Some("it holds a control character, which makes the table unreadable")
        } else {
            None
        };
        match reason {
            Some(reason) => Err(CacheError::InvalidKey { reason }),
            None => Ok(Key { namespace, key }),
        }
    }

    /// Which namespace this key is in.
    pub fn namespace(&self) -> &Namespace {
        &self.namespace
    }

    /// The key itself, without its namespace.
    pub fn as_str(&self) -> &str {
        &self.key
    }
}

impl std::fmt::Display for Key {
    /// `namespace/key`, which is how an error message spells one.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.namespace.0, self.key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_namespace_takes_the_characters_a_name_needs() {
        for good in ["views", "views:v7", "auth.sessions", "a-b_c", "x"] {
            assert!(Namespace::new(good).is_ok(), "{good} should be allowed");
        }
    }

    #[test]
    fn a_namespace_refuses_what_would_make_the_table_unreadable() {
        for bad in ["", "views/v7", "views v7", "views\n", "views%"] {
            assert!(Namespace::new(bad).is_err(), "{bad:?} should be refused");
        }
        assert!(Namespace::new("n".repeat(MAX_NAMESPACE_BYTES + 1)).is_err());
    }

    #[test]
    fn two_namespaces_cannot_be_made_to_collide_by_moving_the_separator() {
        // The whole argument for two columns rather than one prefixed string:
        // these are different addresses, and under a prefix scheme they would
        // be the same string.
        let a = Namespace::new("sessions").unwrap().key("user:7").unwrap();
        let b = Namespace::new("sessions:user").unwrap().key("7").unwrap();
        assert_ne!(a, b);
        assert_ne!(a.namespace(), b.namespace());
    }

    #[test]
    fn a_version_is_just_part_of_the_name() {
        let ns = Namespace::versioned("views", "v7").unwrap();
        assert_eq!(ns.as_str(), "views:v7");
        assert_ne!(ns, Namespace::versioned("views", "v8").unwrap());
    }

    #[test]
    fn a_key_refuses_control_characters_and_overlong_input() {
        let ns = Namespace::new("t").unwrap();
        assert!(ns.key("fine/enough?yes=1").is_ok());
        assert!(ns.key("").is_err());
        assert!(ns.key("two\nlines").is_err());
        assert!(ns.key("k".repeat(MAX_KEY_BYTES + 1)).is_err());
    }

    #[test]
    fn a_key_displays_as_namespace_slash_key() {
        let key = Namespace::new("views").unwrap().key("home").unwrap();
        assert_eq!(key.to_string(), "views/home");
    }
}
