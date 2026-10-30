//! Tags: a name for a group of entries, so one write can invalidate them all.
//!
//! A key answers "which entry". A tag answers "which entries depend on this".
//! A rendered post page depends on the post and on its author; a fragment
//! holding the sidebar depends on every post. Store each entry with the tags
//! of what it read, and when a post changes, call
//! [`Cache::invalidate_tag`](crate::Cache::invalidate_tag) with that post's
//! tag. Every entry that read the post is gone, in every namespace, and no
//! code had to know their keys.
//!
//! # Tags are global, keys are not
//!
//! A [`Key`](crate::Key) lives in a [`Namespace`](crate::Namespace). A tag
//! does not. The route cache, the fragment cache and a handler's own entries
//! all depend on the same rows, and an invalidation that had to name each
//! namespace would be one that forgot one.
//!
//! # The spelling the rest of the crate uses
//!
//! [`Tag::table`] and [`Tag::row`] spell a collection and one row in it:
//! `posts` and `posts:42`. [`TagInvalidator`](crate::TagInvalidator) emits
//! exactly these from model changes, so an entry tagged with them is
//! invalidated by the write that changed the row, with no code in between.
//!
//! # What invalidation promises
//!
//! **Over-invalidation is allowed. A stale hit is not.** An invalidation can
//! remove an entry that did not need to go: a key rewritten with a plain
//! [`Cache::set`](crate::Cache::set) keeps the tags of its previous tagged
//! write until it expires. A miss is always a correct answer for a cache, so
//! this is the safe direction.
//!
//! **A write racing an invalidation can survive it.** A value computed from
//! the old row and stored after the invalidation ran is not removed. This is
//! the race every tag cache has. A short TTL bounds it.

use crate::error::{CacheError, Result};

/// The most bytes a tag may be.
pub const MAX_TAG_BYTES: usize = 256;

/// A name for a group of cache entries.
///
/// Any text from 1 to [`MAX_TAG_BYTES`] bytes with no control character.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Tag(String);

impl Tag {
    /// Builds a tag, checking it.
    ///
    /// # Errors
    /// [`CacheError::InvalidTag`] when the tag is empty, longer than
    /// [`MAX_TAG_BYTES`], or holds a control character.
    pub fn new(tag: impl Into<String>) -> Result<Self> {
        let tag = tag.into();
        let reason = if tag.is_empty() {
            Some("it is empty")
        } else if tag.len() > MAX_TAG_BYTES {
            Some("it is longer than 256 bytes")
        } else if tag.chars().any(char::is_control) {
            Some("it holds a control character")
        } else {
            None
        };
        match reason {
            Some(reason) => Err(CacheError::InvalidTag { tag, reason }),
            None => Ok(Tag(tag)),
        }
    }

    /// The tag for a whole collection: `posts`.
    ///
    /// # Errors
    /// As [`Tag::new`].
    pub fn table(table: &str) -> Result<Self> {
        Self::new(table)
    }

    /// The tag for one row: `posts:42`.
    ///
    /// # Errors
    /// As [`Tag::new`].
    pub fn row(table: &str, id: impl std::fmt::Display) -> Result<Self> {
        Self::new(format!("{table}:{id}"))
    }

    /// The tag as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Tag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for Tag {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_table_and_a_row_have_the_spelling_the_invalidator_uses() {
        assert_eq!(Tag::table("posts").unwrap().as_str(), "posts");
        assert_eq!(Tag::row("posts", 42).unwrap().as_str(), "posts:42");
    }

    #[test]
    fn an_empty_a_long_or_a_control_tag_is_refused() {
        assert!(Tag::new("").is_err());
        assert!(Tag::new("x".repeat(MAX_TAG_BYTES + 1)).is_err());
        assert!(Tag::new("a\nb").is_err());
        assert!(Tag::new("x".repeat(MAX_TAG_BYTES)).is_ok());
    }
}
