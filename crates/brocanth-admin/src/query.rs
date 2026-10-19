//! What a list screen was asked for: a page, a search, and a sort.
//!
//! [`ListQuery::parse`] reads it from a URL query string. There is no
//! dependency doing that: the admin's query string is five known keys, and a
//! percent-decoder for them is thirty lines, which is cheaper than a crate in
//! the tree of something that claims to add none.

use crate::model::ModelDescriptor;

/// Which way a column is sorted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Direction {
    /// Smallest first.
    #[default]
    Ascending,
    /// Largest first.
    Descending,
}

impl Direction {
    /// The URL spelling: `asc` or `desc`.
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Ascending => "asc",
            Direction::Descending => "desc",
        }
    }

    /// The other one, which is what a column header links to.
    pub fn flipped(self) -> Direction {
        match self {
            Direction::Ascending => Direction::Descending,
            Direction::Descending => Direction::Ascending,
        }
    }

    /// Reads `asc` or `desc`. Anything else is ascending.
    pub fn parse(s: &str) -> Direction {
        if s.eq_ignore_ascii_case("desc") {
            Direction::Descending
        } else {
            Direction::Ascending
        }
    }
}

/// A sort: a column and a direction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sort {
    /// The column sorted on.
    pub column: String,
    /// Which way.
    pub direction: Direction,
}

/// Everything a list screen was asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListQuery {
    /// Requested page, counting from one. Clamped when [`Page`] is built.
    ///
    /// [`Page`]: crate::paging::Page
    pub page: usize,
    /// Rows per page, or `None` to take the model's configured default.
    pub per_page: Option<usize>,
    /// The search term, already trimmed. Empty means no search.
    pub search: String,
    /// The sort, when one was asked for.
    pub sort: Option<Sort>,
}

impl ListQuery {
    /// Page one, no search, no sort.
    pub fn new() -> Self {
        ListQuery {
            page: 1,
            ..ListQuery::default()
        }
    }

    /// Asks for a page.
    pub fn page(mut self, page: usize) -> Self {
        self.page = page;
        self
    }

    /// Asks for a page size.
    pub fn per_page(mut self, per_page: usize) -> Self {
        self.per_page = Some(per_page);
        self
    }

    /// Asks for a search.
    pub fn searching(mut self, term: impl Into<String>) -> Self {
        self.search = term.into().trim().to_string();
        self
    }

    /// Asks for a sort.
    pub fn sorted_by(mut self, column: impl Into<String>, direction: Direction) -> Self {
        self.sort = Some(Sort {
            column: column.into(),
            direction,
        });
        self
    }

    /// Reads a query from a URL query string, with or without a leading `?`.
    ///
    /// Keys: `page`, `per_page`, `q`, `sort`, `dir`. An unparsable number is
    /// the default rather than an error, because a hand-edited URL should show
    /// a list rather than a stack trace.
    pub fn parse(query: &str) -> Self {
        let mut out = ListQuery::new();
        let mut sort_column = String::new();
        let mut direction = Direction::Ascending;
        for pair in query.trim_start_matches('?').split('&') {
            if pair.is_empty() {
                continue;
            }
            let (key, raw) = match pair.split_once('=') {
                Some((k, v)) => (k, v),
                None => (pair, ""),
            };
            let value = percent_decode(raw);
            match key {
                "page" => out.page = value.parse().unwrap_or(1),
                "per_page" => out.per_page = value.parse().ok(),
                "q" => out.search = value.trim().to_string(),
                "sort" => sort_column = value,
                "dir" => direction = Direction::parse(&value),
                _ => {}
            }
        }
        if !sort_column.is_empty() {
            out.sort = Some(Sort {
                column: sort_column,
                direction,
            });
        }
        out
    }

    /// Drops a sort naming a column the model does not have.
    ///
    /// A sort key reaches the data source, so a query string must not be able
    /// to name an arbitrary column. The data source is told the column name
    /// only after this has agreed it exists.
    pub fn validated(mut self, model: &ModelDescriptor, sortable: &[String]) -> Self {
        let keep = self.sort.as_ref().is_some_and(|s| {
            model.column(&s.column).is_some()
                && (sortable.is_empty() || sortable.contains(&s.column))
        });
        if !keep {
            self.sort = None;
        }
        if self.page == 0 {
            self.page = 1;
        }
        self
    }

    /// The query string this query would be read back from, without the `?`.
    ///
    /// Used to build the pagination and column-header links, so a sort
    /// survives a page change and a search survives both.
    pub fn to_query_string(&self, page: usize) -> String {
        let mut parts = vec![format!("page={page}")];
        if let Some(per_page) = self.per_page {
            parts.push(format!("per_page={per_page}"));
        }
        if !self.search.is_empty() {
            parts.push(format!("q={}", percent_encode(&self.search)));
        }
        if let Some(sort) = &self.sort {
            parts.push(format!("sort={}", percent_encode(&sort.column)));
            parts.push(format!("dir={}", sort.direction.as_str()));
        }
        parts.join("&")
    }
}

/// Decodes `%XX` escapes and `+`, leaving anything malformed as written.
///
/// Byte-level throughout. Slicing the `&str` instead would panic on `%` in
/// front of a multi-byte character, because the two bytes after the `%` can
/// fall inside one.
/// Every name/value pair of a urlencoded body or query string, decoded.
///
/// `+` is a space here and not in a path, which is why this is not the
/// same function as the one a router would use.
///
/// Only the plugin reads a body: `Admin::dispatch` takes a `Submission`
/// somebody else built. With `plugin` off the function is absent rather
/// than unused, which is what §8.2 asks of a feature that is off.
#[cfg(feature = "plugin")]
pub(crate) fn pairs(body: &str) -> Vec<(String, String)> {
    body.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, raw) = pair.split_once('=').unwrap_or((pair, ""));
            (
                percent_decode(&key.replace('+', " ")),
                percent_decode(&raw.replace('+', " ")),
            )
        })
        .collect()
}

/// One parameter out of a query string, decoded.
///
/// `ListQuery::parse` knows the five keys the model screens use. A panel
/// has its own, and the decoder it needs is already here.
pub(crate) fn param(query: &str, name: &str) -> Option<String> {
    for pair in query.trim_start_matches('?').split('&') {
        let (key, raw) = pair.split_once('=').unwrap_or((pair, ""));
        if key == name {
            return Some(percent_decode(raw));
        }
    }
    None
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                (Some(high), Some(low)) => {
                    out.push(high * 16 + low);
                    i += 3;
                }
                _ => {
                    out.push(b'%');
                    i += 1;
                }
            },
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// One hexadecimal digit, or `None`.
fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Percent-encodes everything outside the unreserved set.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}
