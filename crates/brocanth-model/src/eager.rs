//! Eager loading that counts parents, not joined rows.
//!
//! # The bug this exists to remove
//!
//! `User::find().limit(10).find_with_related(Post).all(db)` emits one
//! `LEFT JOIN` with `LIMIT 10` on the whole result set. The limit therefore
//! counts *joined rows*. Ten users with three posts each gives thirty rows;
//! `LIMIT 10` keeps ten of them, which is three or four users. The call
//! returns successfully, with the wrong answer.
//!
//! Rails splits the query for exactly this reason. So does this module:
//! [`Eager::with_related`] looks at the statement, and routes anything with a
//! `LIMIT` or an `OFFSET` through the two-query loader, where the limit applies
//! to the parent query and the children are fetched by key. Everything else
//! keeps the single join.
//!
//! The choice is reported by [`Eager::strategy`], so it can be asserted in a
//! test rather than inferred from a query log.

use sea_orm::{
    ConnectionTrait, DbErr, EntityTrait, LoaderTrait, QuerySelect, QueryTrait, Related, Select,
};

use crate::Deferred;

/// How an eager load will be issued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EagerStrategy {
    /// One `LEFT JOIN`. Correct whenever the parent set is not limited.
    Join,
    /// Parents first, then their children by key. The only correct shape for a
    /// limited or offset parent set.
    TwoQuery,
}

/// A parent row with its relation slot.
///
/// The slot is a [`Deferred`], so a template that forgot the eager load fails
/// loudly instead of starting an N+1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithRelated<P, C> {
    /// The parent row.
    pub parent: P,
    related: Deferred<Vec<C>>,
}

impl<P, C> WithRelated<P, C> {
    /// A parent whose relation was loaded.
    pub fn loaded(parent: P, related: Vec<C>) -> Self {
        Self {
            parent,
            related: Deferred::Loaded(related),
        }
    }

    /// A parent whose relation was deliberately not loaded.
    pub fn deferred(parent: P) -> Self {
        Self {
            parent,
            related: Deferred::NotLoaded,
        }
    }

    /// The related rows.
    ///
    /// # Panics
    ///
    /// If the relation was never loaded. See [`Deferred::get`].
    pub fn related(&self) -> &[C] {
        self.related.get().as_slice()
    }

    /// The related rows, or an error naming the relation.
    pub fn try_related(&self) -> Result<&[C], crate::NotLoaded> {
        self.related.try_get().map(Vec::as_slice)
    }

    /// `true` when the relation was loaded.
    pub fn is_loaded(&self) -> bool {
        self.related.is_loaded()
    }

    /// Split into the parent and the relation slot.
    pub fn into_parts(self) -> (P, Deferred<Vec<C>>) {
        (self.parent, self.related)
    }
}

/// Turn a `Select` into an eager-loading query.
pub trait EagerExt<E: EntityTrait>: Sized {
    /// Wrap this select so `with_related` picks the right query shape.
    fn eager(self) -> Eager<E>;
}

impl<E: EntityTrait> EagerExt<E> for Select<E> {
    fn eager(self) -> Eager<E> {
        Eager::new(self)
    }
}

/// A select that knows how to eager-load without miscounting parents.
#[derive(Debug, Clone)]
pub struct Eager<E: EntityTrait> {
    select: Select<E>,
}

impl<E: EntityTrait> Eager<E> {
    /// Wrap a select.
    pub fn new(select: Select<E>) -> Self {
        Self { select }
    }

    /// Keep at most `n` **parents**.
    ///
    /// Unlike `Select::limit` followed by `find_with_related`, this means what
    /// it says.
    pub fn limit(mut self, n: u64) -> Self {
        self.select = self.select.limit(n);
        self
    }

    /// Skip `n` **parents**.
    pub fn offset(mut self, n: u64) -> Self {
        self.select = self.select.offset(n);
        self
    }

    /// Apply any other `Select` builder call — `filter`, `order_by`, and so on.
    pub fn map(mut self, f: impl FnOnce(Select<E>) -> Select<E>) -> Self {
        self.select = f(self.select);
        self
    }

    /// The parent select, unchanged.
    pub fn into_select(self) -> Select<E> {
        self.select
    }

    /// Which query shape [`Eager::with_related`] will use.
    ///
    /// Determined by whether the statement carries a `LIMIT` or an `OFFSET`,
    /// found by resetting both on a clone and comparing: `sea_query` exposes no
    /// getter, but `SelectStatement` is `PartialEq`.
    pub fn strategy(&self) -> EagerStrategy {
        let stmt = self.select.as_query();
        let mut unbounded = stmt.clone();
        unbounded.reset_limit().reset_offset();
        if &unbounded == stmt {
            EagerStrategy::Join
        } else {
            EagerStrategy::TwoQuery
        }
    }

    /// Load parents with their related rows, correctly.
    ///
    /// One statement when the parent set is unbounded, two when it is limited
    /// or offset.
    pub async fn with_related<R, C>(
        self,
        db: &C,
    ) -> Result<Vec<WithRelated<E::Model, R::Model>>, DbErr>
    where
        R: EntityTrait,
        E: Related<R>,
        E::Model: Sync,
        R::Model: Send + Sync,
        C: ConnectionTrait,
    {
        match self.strategy() {
            EagerStrategy::Join => {
                let rows = self.select.find_with_related(R::default()).all(db).await?;
                Ok(rows
                    .into_iter()
                    .map(|(parent, related)| WithRelated::loaded(parent, related))
                    .collect())
            }
            EagerStrategy::TwoQuery => {
                let parents = self.select.all(db).await?;
                let related = parents.load_many(R::default(), db).await?;
                Ok(parents
                    .into_iter()
                    .zip(related)
                    .map(|(parent, related)| WithRelated::loaded(parent, related))
                    .collect())
            }
        }
    }

    /// Load the parents only, leaving the relation slot unloaded.
    ///
    /// Reading it later panics with the relation's name. That is the intent:
    /// a forgotten eager load should be a failure in development, not one
    /// query per row in production.
    pub async fn without_related<R, C>(
        self,
        db: &C,
    ) -> Result<Vec<WithRelated<E::Model, R::Model>>, DbErr>
    where
        R: EntityTrait,
        E: Related<R>,
        C: ConnectionTrait,
    {
        Ok(self
            .select
            .all(db)
            .await?
            .into_iter()
            .map(WithRelated::deferred)
            .collect())
    }
}
