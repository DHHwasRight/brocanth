//! Relations that are loud when they were never loaded.
//!
//! Toasty's idea, and the one worth stealing: an unloaded relation should say
//! so, not quietly issue a query. Nothing here can fire a statement, so an N+1
//! cannot start inside a render loop.

use std::fmt;

/// A relation slot: either loaded, or openly empty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Deferred<T> {
    /// The relation was fetched.
    Loaded(T),
    /// The relation was never fetched. Reading it is an error, not a query.
    #[default]
    NotLoaded,
}

/// Returned when a caller reads a relation the query never loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "relation `{relation}` was never loaded; load it with `.eager().with_related::<..>(db)` \
     instead of reading it here"
)]
pub struct NotLoaded {
    /// Type name of the relation that was read.
    pub relation: &'static str,
}

impl<T> Deferred<T> {
    /// `true` when the relation was fetched.
    pub fn is_loaded(&self) -> bool {
        matches!(self, Deferred::Loaded(_))
    }

    /// The loaded value, or an error naming the relation.
    pub fn try_get(&self) -> Result<&T, NotLoaded> {
        match self {
            Deferred::Loaded(v) => Ok(v),
            Deferred::NotLoaded => Err(NotLoaded {
                relation: std::any::type_name::<T>(),
            }),
        }
    }

    /// The loaded value.
    ///
    /// # Panics
    ///
    /// If the relation was never loaded. That is the point: a missing eager
    /// load fails here, in development, rather than becoming one query per row
    /// in production.
    pub fn get(&self) -> &T {
        self.try_get().unwrap_or_else(|e| panic!("{e}"))
    }

    /// The loaded value, if there is one.
    pub fn into_inner(self) -> Option<T> {
        match self {
            Deferred::Loaded(v) => Some(v),
            Deferred::NotLoaded => None,
        }
    }

    /// Apply `f` to the loaded value, keeping the slot shape.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Deferred<U> {
        match self {
            Deferred::Loaded(v) => Deferred::Loaded(f(v)),
            Deferred::NotLoaded => Deferred::NotLoaded,
        }
    }
}

impl<T: fmt::Display> fmt::Display for Deferred<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Deferred::Loaded(v) => v.fmt(f),
            Deferred::NotLoaded => f.write_str("<not loaded>"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unloaded_relation_names_itself() {
        let d: Deferred<Vec<u32>> = Deferred::NotLoaded;
        let err = d.try_get().unwrap_err();
        assert!(err.relation.contains("Vec"));
        assert!(err.to_string().contains("never loaded"));
    }

    #[test]
    #[should_panic(expected = "never loaded")]
    fn reading_an_unloaded_relation_panics_instead_of_querying() {
        let d: Deferred<Vec<u32>> = Deferred::NotLoaded;
        let _ = d.get();
    }
}
