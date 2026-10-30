//! Invalidation from model changes: a [`ChangeSink`] that turns each row
//! change into two tag invalidations.
//!
//! A change to row 42 of `posts` invalidates the tags `posts` and `posts:42`
//! ([`Tag::table`] and [`Tag::row`]). An entry stored with either tag is gone
//! after the write, and no handler had to remember it.
//!
//! ```no_run
//! # use std::sync::Arc;
//! # use brocanth_cache::{PgCache, TagInvalidator};
//! # use brocanth_events::ChangeSink;
//! # async fn example(cache: PgCache) {
//! let sink = TagInvalidator::new(Arc::new(cache)).only(["posts", "users"]);
//! // Hand `sink` to the model layer, alone or in a `FanOut`.
//! # let _: Box<dyn ChangeSink> = Box::new(sink);
//! # }
//! ```
//!
//! # Why the invalidation runs on a spawned task
//!
//! [`ChangeSink::publish`] is synchronous and must not block the writer, and
//! an invalidation is a database round trip. So `publish` spawns the work on
//! the Tokio runtime it was built on and returns. The invalidation lands
//! shortly after the write, not inside it: a read in that window can still
//! see the old entry. A failed invalidation is dropped, because `publish`
//! has nobody to report to; the entry's TTL is the bound.

use std::sync::Arc;

use brocanth_events::{ChangeSink, RowChange};

use crate::cache::Cache;
use crate::tag::Tag;

/// Invalidates `table` and `table:id` for every row change it is given.
pub struct TagInvalidator {
    cache: Arc<dyn Cache>,
    runtime: Option<tokio::runtime::Handle>,
    only: Option<Vec<String>>,
}

impl TagInvalidator {
    /// An invalidator for every collection, on the current Tokio runtime.
    ///
    /// Built outside a runtime, it spawns on whichever runtime is current at
    /// each `publish`, and does nothing when none is.
    pub fn new(cache: Arc<dyn Cache>) -> Self {
        Self {
            cache,
            runtime: tokio::runtime::Handle::try_current().ok(),
            only: None,
        }
    }

    /// Spawns on this runtime.
    #[must_use]
    pub fn on(mut self, runtime: tokio::runtime::Handle) -> Self {
        self.runtime = Some(runtime);
        self
    }

    /// Listens to these collections only. [`ChangeSink::wants`] answers
    /// `false` for the rest, so the model layer skips building their changes.
    #[must_use]
    pub fn only(mut self, collections: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.only = Some(collections.into_iter().map(Into::into).collect());
        self
    }

    /// The tags one change invalidates.
    pub fn tags_for(change: &RowChange<'_>) -> Vec<Tag> {
        [
            Tag::table(&change.collection),
            Tag::row(&change.collection, &change.row),
        ]
        .into_iter()
        .filter_map(Result::ok)
        .collect()
    }
}

impl ChangeSink for TagInvalidator {
    fn publish(&self, change: &RowChange<'_>) {
        if !self.wants(&change.collection) {
            return;
        }
        let tags = Self::tags_for(change);
        if tags.is_empty() {
            return;
        }
        let Some(runtime) = self
            .runtime
            .clone()
            .or_else(|| tokio::runtime::Handle::try_current().ok())
        else {
            return;
        };
        let cache = self.cache.clone();
        runtime.spawn(async move {
            let _ = cache.invalidate_tags(&tags).await;
        });
    }

    fn wants(&self, collection: &str) -> bool {
        match &self.only {
            None => true,
            Some(list) => list.iter().any(|c| c == collection),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_change_names_its_table_and_its_row() {
        let change = RowChange::deleted("posts", 42_i64);
        let tags = TagInvalidator::tags_for(&change);
        let names: Vec<&str> = tags.iter().map(Tag::as_str).collect();
        assert_eq!(names, ["posts", "posts:42"]);
    }
}
