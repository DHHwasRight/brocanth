//! Where the history is kept: [`AuditStore`], [`Query`], and
//! [`MemoryStore`] for a test or a project with no database.

use std::fmt;
use std::sync::Mutex;

use async_trait::async_trait;

use crate::entry::Entry;

/// The most entries one read returns. A page shows 50.
pub const MAX_LIMIT: usize = 500;

/// A store failed. The message says what, and for a setup mistake how to
/// fix it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreError(String);

impl StoreError {
    /// A failure with this message.
    pub fn new(message: impl Into<String>) -> StoreError {
        StoreError(message.into())
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StoreError {}

/// Which entries to read: newest first, optionally of one table or one
/// row, older than an id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    /// Only this table.
    pub table: Option<String>,
    /// Only this row. Read with `table`; alone it matches nothing useful
    /// and is ignored.
    pub row: Option<String>,
    /// Only entries with an id below this one: the next page.
    pub before: Option<i64>,
    /// At most this many, capped at [`MAX_LIMIT`].
    pub limit: usize,
}

impl Default for Query {
    fn default() -> Self {
        Query {
            table: None,
            row: None,
            before: None,
            limit: 50,
        }
    }
}

impl Query {
    /// The newest entries of every table.
    pub fn recent() -> Query {
        Query::default()
    }

    /// The history of one row.
    pub fn row(table: impl Into<String>, row: impl Into<String>) -> Query {
        Query {
            table: Some(table.into()),
            row: Some(row.into()),
            ..Query::default()
        }
    }

    /// Only entries older than `id`.
    pub fn before(mut self, id: i64) -> Query {
        self.before = Some(id);
        self
    }

    /// At most `limit` entries.
    pub fn limit(mut self, limit: usize) -> Query {
        self.limit = limit;
        self
    }

    /// Whether `entry` matches. The rule a store applies, here once.
    pub fn matches(&self, entry: &Entry) -> bool {
        let table = self.table.as_deref().is_none_or(|t| t == entry.table);
        let row = match (&self.table, &self.row) {
            (Some(_), Some(r)) => *r == entry.row,
            _ => true,
        };
        let before = self.before.is_none_or(|id| entry.id < id);
        table && row && before
    }

    /// The limit a store uses: at least 1, at most [`MAX_LIMIT`].
    pub fn capped(&self) -> usize {
        self.limit.clamp(1, MAX_LIMIT)
    }
}

/// Where the history lives.
///
/// Append-only: an entry is never changed, and only
/// [`purge`](AuditStore::purge) removes one.
#[async_trait]
pub trait AuditStore: Send + Sync {
    /// Stores `entries`, in order. Their `id` is ignored: the store gives
    /// each a new one, higher than every id before it.
    async fn append(&self, entries: &[Entry]) -> Result<(), StoreError>;

    /// The entries `query` matches, newest (highest id) first.
    async fn find(&self, query: &Query) -> Result<Vec<Entry>, StoreError>;

    /// Removes every entry made before `before` (seconds since 1970), and
    /// says how many.
    async fn purge(&self, before: i64) -> Result<u64, StoreError>;

    /// Whether the entries survive a restart. `true` by default;
    /// [`MemoryStore`] says `false`, and the History tab says so.
    fn is_durable(&self) -> bool {
        true
    }
}

/// Entries in memory, lost at restart. For a test, and for a project that
/// registered the plugin without a database.
#[derive(Debug, Default)]
pub struct MemoryStore {
    entries: Mutex<Held>,
}

/// The entries, and the id the next one gets. A counter of its own, so an
/// id is never given twice, also after a purge empties the store.
#[derive(Debug, Default)]
struct Held {
    entries: Vec<Entry>,
    last_id: i64,
}

impl MemoryStore {
    /// An empty store.
    pub fn new() -> MemoryStore {
        MemoryStore::default()
    }

    /// How many entries it holds.
    pub fn len(&self) -> usize {
        self.held().entries.len()
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.held().entries.is_empty()
    }

    fn held(&self) -> std::sync::MutexGuard<'_, Held> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[async_trait]
impl AuditStore for MemoryStore {
    async fn append(&self, entries: &[Entry]) -> Result<(), StoreError> {
        let mut held = self.held();
        for entry in entries {
            held.last_id += 1;
            let id = held.last_id;
            held.entries.push(Entry {
                id,
                ..entry.clone()
            });
        }
        Ok(())
    }

    async fn find(&self, query: &Query) -> Result<Vec<Entry>, StoreError> {
        Ok(self
            .held()
            .entries
            .iter()
            .rev()
            .filter(|e| query.matches(e))
            .take(query.capped())
            .cloned()
            .collect())
    }

    fn is_durable(&self) -> bool {
        false
    }

    async fn purge(&self, before: i64) -> Result<u64, StoreError> {
        let mut held = self.held();
        let was = held.entries.len();
        held.entries.retain(|e| e.at >= before);
        Ok((was - held.entries.len()) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brocanth_events::RowChange;

    #[tokio::test]
    async fn an_id_is_never_given_twice_also_after_a_purge_empties_the_store() {
        let store = MemoryStore::new();
        let entry = Entry::from_change(&RowChange::created("post", 1_i64), None, 10);
        store.append(&[entry.clone(), entry.clone()]).await.unwrap();
        assert_eq!(store.purge(100).await.unwrap(), 2);
        store.append(&[entry]).await.unwrap();
        let found = store.find(&Query::recent()).await.unwrap();
        assert_eq!(found[0].id, 3, "the next id, not the first again");
    }
}
