//! What a subscriber asked for, and the distinction PocketBase got right.
//!
//! PocketBase spells a subscription `collection/*` or `collection/recordId`,
//! and gates the first on the list rule and the second on the view rule. The
//! spelling is interchangeable; the **distinction** is not, and this crate
//! keeps it in the type:
//!
//! * [`Topic::Collection`] — every row of one collection. Gate it with
//!   `Policy::list`.
//! * [`Topic::Row`] — one named row. Gate it with `Policy::view`.
//!
//! A row topic is not an optimisation of a collection topic. It is a different
//! question with a different answer, and conflating them is how a subscriber
//! ends up seeing a row the list rule hides.
//!
//! # The spelling is PocketBase's, deliberately
//!
//! [`Display`](fmt::Display) writes `posts/*` and `posts/42`, and
//! [`Topic::parse`] reads them back. Interoperating with a client that already
//! speaks this costs nothing, and a topic string is what the SSE `event:` name
//! carries, so it has to be text at some point anyway. PocketBase's deprecated
//! bare-collection alias is not accepted: there is no reason to be bug
//! compatible with something its own authors deprecated.
//!
//! # Control characters are rejected here
//!
//! A topic becomes the `event:` field of an SSE frame, and that field is
//! terminated by a newline. A row id carrying `\n` would let a subscriber
//! forge frame boundaries in its own stream. [`Topic::row`] refuses it at
//! subscribe time, which is the only place a human is still listening.

use std::fmt;

use brocanth_events::RowId;

/// What a subscriber asked for.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Topic {
    /// Every row of one collection. Gated by the list rule.
    Collection(String),
    /// One row. Gated by the view rule.
    Row {
        /// The collection the row lives in.
        collection: String,
        /// Which row.
        id: RowId<'static>,
    },
}

impl Topic {
    /// Every row of a collection: `posts/*`.
    pub fn collection(name: impl Into<String>) -> Result<Topic, BadTopic> {
        let name = name.into();
        check(&name)?;
        Ok(Topic::Collection(name))
    }

    /// One row: `posts/42`.
    ///
    /// `Err` when either part carries a control character, because the topic
    /// is written into an SSE `event:` line.
    pub fn row(
        collection: impl Into<String>,
        id: impl Into<RowId<'static>>,
    ) -> Result<Topic, BadTopic> {
        let collection = collection.into();
        check(&collection)?;
        let id = id.into();
        if let RowId::Text(text) = &id {
            check(text)?;
        }
        Ok(Topic::Row { collection, id })
    }

    /// The collection either kind of topic is about.
    pub fn collection_name(&self) -> &str {
        match self {
            Topic::Collection(name) => name,
            Topic::Row { collection, .. } => collection,
        }
    }

    /// The row, for a row topic.
    pub fn row_id(&self) -> Option<&RowId<'static>> {
        match self {
            Topic::Collection(_) => None,
            Topic::Row { id, .. } => Some(id),
        }
    }

    /// `true` for a collection topic.
    pub fn is_wildcard(&self) -> bool {
        matches!(self, Topic::Collection(_))
    }

    /// Read `posts/*` or `posts/42`.
    ///
    /// A parsed row id is always [`RowId::Text`], because the text on the wire
    /// does not say whether the database stores an integer. The caller that
    /// knows builds the topic with [`Topic::row`] instead; a broker matches
    /// `Text("42")` and `I64(42)` as different rows, and
    /// [`brocanth_events::RowId`] documents that as deliberate.
    pub fn parse(text: &str) -> Result<Topic, BadTopic> {
        let Some((collection, rest)) = text.split_once('/') else {
            return Err(BadTopic::NoSlash);
        };
        if collection.is_empty() {
            return Err(BadTopic::Empty);
        }
        if rest == "*" {
            Topic::collection(collection)
        } else if rest.is_empty() {
            Err(BadTopic::Empty)
        } else {
            Topic::row(collection, rest.to_owned())
        }
    }
}

fn check(text: &str) -> Result<(), BadTopic> {
    if text.is_empty() {
        return Err(BadTopic::Empty);
    }
    if text.chars().any(|c| c.is_control()) {
        return Err(BadTopic::Control);
    }
    Ok(())
}

impl fmt::Display for Topic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Topic::Collection(name) => write!(f, "{name}/*"),
            Topic::Row { collection, id } => write!(f, "{collection}/{id}"),
        }
    }
}

/// Why a topic was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BadTopic {
    /// A topic is `collection/*` or `collection/<id>`. PocketBase's bare
    /// `collection` alias is deprecated and not accepted.
    #[error("a topic is `collection/*` or `collection/<id>`")]
    NoSlash,
    /// An empty collection name or row id.
    #[error("a topic has an empty collection or row id")]
    Empty,
    /// A control character, which would forge an SSE frame boundary.
    #[error("a topic carries a control character, which would break the event stream")]
    Control,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        for text in ["posts/*", "posts/42", "a_b/c-d"] {
            let topic = Topic::parse(text).expect(text);
            assert_eq!(topic.to_string(), text);
        }
    }

    #[test]
    fn a_wildcard_and_a_row_are_different_topics() {
        let wildcard = Topic::collection("posts").unwrap();
        let row = Topic::row("posts", 42i64).unwrap();
        assert_ne!(wildcard, row);
        assert!(wildcard.is_wildcard());
        assert!(!row.is_wildcard());
        assert_eq!(wildcard.collection_name(), row.collection_name());
    }

    #[test]
    fn a_newline_in_a_row_id_is_refused() {
        assert_eq!(
            Topic::row("posts", "4\n2".to_owned()),
            Err(BadTopic::Control)
        );
        assert_eq!(Topic::parse("posts"), Err(BadTopic::NoSlash));
        assert_eq!(Topic::parse("/x"), Err(BadTopic::Empty));
    }
}
