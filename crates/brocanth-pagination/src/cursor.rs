//! Keyset pages: the last row's sort key, carried in the link to the next.
//!
//! An offset page reads and throws away every row before it, so page 4 000
//! costs 4 000 pages, and a row inserted above it moves every row below by
//! one. A keyset page asks for "the rows after this key", which an index
//! answers in one seek, and a new row cannot shift it. It has no total and no
//! page numbers: a feed, an export, an API that is read to the end.
//!
//! # The cursor is not a secret and not a permission
//!
//! A [`Cursor`] is the sort key of the last row, written as text a URL can
//! carry. A client can read it and can forge it. Forging one only moves the
//! window: the policy's condition stays in the `WHERE` beside the cursor's,
//! so a forged cursor never reaches a row the subject may not list.
//!
//! # Next, and no previous
//!
//! A page links to the next one and back to the first. A previous link needs
//! the query run backwards and the rows reversed, and the people who read a
//! feed backwards are few. pagy's keyset does the same.

use std::fmt;

/// One value of a sort key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// An integer: an id, a timestamp in seconds.
    Int(i64),
    /// Text: a slug, an ISO 8601 time.
    Text(String),
}

impl From<i64> for Key {
    fn from(value: i64) -> Self {
        Key::Int(value)
    }
}

impl From<&str> for Key {
    fn from(value: &str) -> Self {
        Key::Text(value.to_string())
    }
}

impl From<String> for Key {
    fn from(value: String) -> Self {
        Key::Text(value)
    }
}

/// The sort key of the last row on a page.
///
/// Written as `i42`, or `t` and the text in hexadecimal, joined by `.`: only
/// `[0-9a-z.-]`, so it goes into a URL with no escaping.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Cursor(Vec<Key>);

/// A cursor that does not parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorError {
    token: String,
    reason: Reason,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Reason {
    /// Not a token, or a key of the wrong kind for its column.
    NotACursor,
    /// A token with another number of keys.
    Width(usize),
    /// The order sorts on a column that a cursor cannot carry. The list's
    /// fault, not the client's.
    Unsortable(&'static str),
}

impl CursorError {
    /// `token` is not a cursor: it does not parse, or a key does not have its
    /// column's kind.
    pub(crate) fn not_a_cursor(token: String) -> Self {
        CursorError {
            token,
            reason: Reason::NotACursor,
        }
    }

    #[cfg(feature = "policy")]
    pub(crate) fn unsortable(token: String, column: &'static str) -> Self {
        CursorError {
            token,
            reason: Reason::Unsortable(column),
        }
    }
}

impl fmt::Display for CursorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.reason {
            Reason::Width(n) => write!(
                f,
                "the cursor `{}` does not hold {n} key(s)\n\
                 help: follow the `next` link of the previous page; do not write a cursor by hand",
                self.token
            ),
            Reason::NotACursor => write!(
                f,
                "`{}` is not a cursor\n\
                 help: follow the `next` link of the previous page, or drop `after` to start at the first page",
                self.token
            ),
            Reason::Unsortable(column) => write!(
                f,
                "the list sorts on `{column}`, and a cursor cannot carry its values\n\
                 help: sort a keyset list on an integer or text column, or use offset pages"
            ),
        }
    }
}

impl std::error::Error for CursorError {}

impl Cursor {
    /// A cursor over these key values, in sort order: the sort column, then
    /// the tie-breaker.
    pub fn new(keys: impl IntoIterator<Item = Key>) -> Self {
        Cursor(keys.into_iter().collect())
    }

    /// The key values.
    pub fn keys(&self) -> &[Key] {
        &self.0
    }

    /// The cursor as a URL writes it.
    pub fn token(&self) -> String {
        let mut out = String::new();
        for (index, key) in self.0.iter().enumerate() {
            if index > 0 {
                out.push('.');
            }
            match key {
                Key::Int(n) => {
                    out.push('i');
                    out.push_str(&n.to_string());
                }
                Key::Text(text) => {
                    out.push('t');
                    for byte in text.as_bytes() {
                        out.push_str(&format!("{byte:02x}"));
                    }
                }
            }
        }
        out
    }

    /// Reads a token back.
    ///
    /// # Errors
    ///
    /// [`CursorError`] when the token is not one [`Cursor::token`] writes.
    pub fn parse(token: &str) -> Result<Cursor, CursorError> {
        let bad = || CursorError::not_a_cursor(token.to_string());
        if token.is_empty() {
            return Err(bad());
        }
        let mut keys = Vec::new();
        for part in token.split('.') {
            let (kind, rest) = part.split_at_checked(1).ok_or_else(bad)?;
            match kind {
                "i" => keys.push(Key::Int(rest.parse().map_err(|_| bad())?)),
                "t" => {
                    if rest.len() % 2 != 0 {
                        return Err(bad());
                    }
                    // Byte pairs, not `&rest[i..i + 2]`: a multi-byte
                    // character can put a slice end inside itself, and
                    // slicing there panics on input a client typed.
                    let bytes = rest
                        .as_bytes()
                        .chunks(2)
                        .map(|pair| Some(hex(pair[0])? * 16 + hex(pair[1])?))
                        .collect::<Option<Vec<u8>>>()
                        .ok_or_else(bad)?;
                    keys.push(Key::Text(String::from_utf8(bytes).map_err(|_| bad())?));
                }
                _ => return Err(bad()),
            }
        }
        Ok(Cursor(keys))
    }

    /// The same cursor, checked to hold `n` keys.
    ///
    /// # Errors
    ///
    /// [`CursorError`] when it holds another number.
    pub fn expect(self, n: usize) -> Result<Cursor, CursorError> {
        if self.0.len() == n {
            Ok(self)
        } else {
            Err(CursorError {
                token: self.token(),
                reason: Reason::Width(n),
            })
        }
    }
}

fn hex(byte: u8) -> Option<u8> {
    char::from(byte)
        .to_digit(16)
        .and_then(|d| u8::try_from(d).ok())
}

impl fmt::Display for Cursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.token())
    }
}

/// One keyset page: the rows, and the cursor of the next page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeysetPage<R> {
    /// The rows, at most the limit.
    pub items: Vec<R>,
    /// Where the next page starts. `None` on the last page.
    pub next: Option<Cursor>,
}

impl<R> KeysetPage<R> {
    /// Builds a page from rows read with `LIMIT limit + 1`.
    ///
    /// The extra row is how a keyset page knows there is a next one without
    /// a count. It is dropped here, and the cursor is the key of the last row
    /// that stays.
    pub fn from_rows(mut rows: Vec<R>, limit: u64, key: impl Fn(&R) -> Cursor) -> Self {
        let limit = usize::try_from(limit.max(1)).unwrap_or(usize::MAX);
        let more = rows.len() > limit;
        rows.truncate(limit);
        let next = if more { rows.last().map(key) } else { None };
        KeysetPage { items: rows, next }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cursor_round_trips() {
        let cursor = Cursor::new([Key::Text("2026-11-09 é/&".into()), Key::Int(-42)]);
        let token = cursor.token();
        assert!(token
            .bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase() || b == b'.' || b == b'-'));
        assert_eq!(Cursor::parse(&token), Ok(cursor));
    }

    #[test]
    fn a_forged_token_is_refused_with_a_help_line() {
        for token in ["", "x1", "i", "iabc", "t0", "tzz", "i1..i2", "tff"] {
            let error = Cursor::parse(token).expect_err(token);
            assert!(error.to_string().contains("help:"), "{token}");
        }
        let error = Cursor::parse("i1").unwrap().expect(2).unwrap_err();
        assert!(error.to_string().contains("2 key(s)"));
    }

    #[test]
    fn the_extra_row_says_there_is_a_next_page() {
        let page = KeysetPage::from_rows(vec![1, 2, 3], 2, |n| Cursor::new([Key::Int(*n)]));
        assert_eq!(page.items, vec![1, 2]);
        assert_eq!(page.next, Some(Cursor::new([Key::Int(2)])));

        let last = KeysetPage::from_rows(vec![1, 2], 2, |n| Cursor::new([Key::Int(*n)]));
        assert_eq!(last.next, None);
    }
}
