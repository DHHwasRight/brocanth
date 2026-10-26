//! The publishing half: what the model layer says after it writes a row.
//!
//! PLAN.md §10 — *"the model layer publishes, realtime subscribes, and neither
//! depends on the other."* `brocanth-events` is that seam. It was built, it is
//! tested, and `brocanth-realtime` already implements
//! [`ChangeSink`] for its broker. This module is the end that was missing:
//! `docs/framework-gaps.md` G4, *"the model layer publishes to nobody"*.
//!
//! # One word for one row
//!
//! [`brocanth_events::RowChange`] is the word, and this module does not invent
//! a second one. [`crate::Change`] is a *column* — a field name and the two
//! values around it — and a `RowChange` is a *row*: the table, the primary key
//! and whether it was created, updated or deleted. A write produces one
//! `RowChange` and as many `Change`s as it touched columns.
//!
//! # What a write reports to
//!
//! A [`ChangeSink`], and nothing narrower. The model layer never asks what the
//! sink is, so the same call reaches a broker, an audit log, a cache
//! invalidator, the application's plugin hooks (see [`crate::plugin`]) or
//! several of them behind a [`brocanth_events::FanOut`].
//!
//! Nothing here is global, which is the rule the rest of this crate keeps: a
//! sink is a value the caller holds, so two applications in one process
//! publish to two different places.
//!
//! ```ignore
//! let change = post.insert_published(db, &broker).await?;
//! ```
//!
//! [`ChangeSink::wants`] is asked first, every time, so a sink that answers
//! `false` costs one virtual call and the change is never built. An
//! application that installed no sink holds [`brocanth_events::NoopSink`] and
//! pays exactly that.
//!
//! # A change that a rollback must not publish
//!
//! A row written inside a transaction is not a row until the transaction
//! commits, and a sink cannot un-say a change. So a write inside a transaction
//! reports to a [`Pending`], which holds the changes, and the sink hears them
//! only when the transaction commits.
//!
//! [`Writing`] is the shape that makes the mistake unwritable, the same way
//! [`crate::testing::TestTransaction`] does: it holds the transaction and the
//! held changes together, its `commit` is the only thing that publishes, and
//! dropping it publishes nothing.
//!
//! ```ignore
//! let writing = Writing::begin(db, &broker).await?;
//! post.insert_published(&*writing, &writing).await?;
//! writing.commit().await?;  // the row is committed, so now the change goes out
//! ```

use std::borrow::Cow;
use std::fmt;
use std::ops::Deref;
use std::sync::Mutex;

use async_trait::async_trait;
use brocanth_events::{ChangeSink, RowChange, RowId};
use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, ActiveValue, ConnectionTrait, EntityTrait, IdenStatic,
    IntoActiveModel, Iterable, ModelTrait, PrimaryKeyToColumn, TransactionSession,
    TransactionTrait, Value,
};

use crate::{Error, Tracked, Write};

// ---------------------------------------------------------------------------
// Naming a row
// ---------------------------------------------------------------------------

/// The table an entity is stored in, as the database spells it.
///
/// This is the `collection` of every [`RowChange`] the entity publishes, and
/// it is also the name a realtime subscriber builds its topic from. One
/// spelling, taken from the entity itself, so the two cannot drift.
pub fn table_name<E: EntityTrait>() -> &'static str {
    E::default().table_name()
}

/// The primary key of a stored row, as a change names it.
///
/// A composite key is joined with a comma into [`RowId::Text`], because a
/// change names one row and a two-column key is still one row. The order is
/// the entity's own primary-key order, so two calls agree.
pub fn row_id<E: EntityTrait>(model: &E::Model) -> RowId<'static> {
    let columns: Vec<E::Column> = E::PrimaryKey::iter().map(|key| key.into_column()).collect();
    match columns.as_slice() {
        // SeaORM gives every entity a primary key, so this is unreachable. An
        // unnamed row is still better than a panic on a write that succeeded.
        [] => RowId::Text(Cow::Borrowed("")),
        [only] => id_of(model.get(*only)),
        many => RowId::Text(Cow::Owned(
            many.iter()
                .map(|column| model.get(*column).to_string())
                .collect::<Vec<String>>()
                .join(","),
        )),
    }
}

/// The primary key of a record that is about to be written.
///
/// `None` when a key column is `NotSet`, which is a record SeaORM refuses to
/// update or delete. The caller reads the key before the write and publishes
/// after it, so a `None` here is a write that failed.
fn active_row_id<A: ActiveModelTrait>(active: &A) -> Option<RowId<'static>> {
    let mut parts: Vec<Value> = Vec::new();
    for key in <A::Entity as EntityTrait>::PrimaryKey::iter() {
        match active.get(key.into_column()) {
            ActiveValue::Set(value) | ActiveValue::Unchanged(value) => parts.push(value),
            ActiveValue::NotSet => return None,
        }
    }
    match parts.len() {
        0 => None,
        1 => Some(id_of(parts.remove(0))),
        _ => Some(RowId::Text(Cow::Owned(
            parts
                .iter()
                .map(Value::to_string)
                .collect::<Vec<String>>()
                .join(","),
        ))),
    }
}

/// One key column, as the identity a change carries.
fn id_of(value: Value) -> RowId<'static> {
    match value {
        Value::TinyInt(Some(n)) => RowId::I64(n.into()),
        Value::SmallInt(Some(n)) => RowId::I64(n.into()),
        Value::Int(Some(n)) => RowId::I64(n.into()),
        Value::BigInt(Some(n)) => RowId::I64(n),
        Value::TinyUnsigned(Some(n)) => RowId::I64(n.into()),
        Value::SmallUnsigned(Some(n)) => RowId::I64(n.into()),
        Value::Unsigned(Some(n)) => RowId::I64(n.into()),
        // A `u64` key above `i64::MAX` is not an `i64`, and `RowId::U128`
        // holds it without lying about its width.
        Value::BigUnsigned(Some(n)) => match i64::try_from(n) {
            Ok(n) => RowId::I64(n),
            Err(_) => RowId::U128(n.into()),
        },
        Value::String(Some(s)) => RowId::Text(Cow::Owned(s)),
        Value::Char(Some(c)) => RowId::Text(Cow::Owned(c.to_string())),
        // Everything else as the value prints itself: a UUID, a key type a
        // driver feature added, or a `NULL` the database has not filled in.
        // `RowId::U128` stays for a layer that knows the bits are a UUID --
        // this crate does not turn on sea-orm's `with-uuid`, so it cannot name
        // that variant, and a hyphenated string is the honest reading.
        other => RowId::Text(Cow::Owned(other.to_string())),
    }
}

/// The columns a record will write, as the database spells them.
///
/// Every `Set` column, which is what an `ActiveModel` alone can say. A
/// [`Tracked`] record knows more -- it has the row that was loaded, so it can
/// drop a column set to the value it already held -- and
/// [`Tracked::save_published`] uses that instead.
fn set_columns<A: ActiveModelTrait>(active: &A) -> Vec<&'static str> {
    <A::Entity as EntityTrait>::Column::iter()
        .filter(|column| matches!(active.get(*column), ActiveValue::Set(_)))
        .map(|column| column.as_str())
        .collect()
}

// ---------------------------------------------------------------------------
// The write verbs that publish
// ---------------------------------------------------------------------------

/// The write verbs of [`Write`], each publishing one [`RowChange`].
///
/// Implemented for every `ActiveModel`, like [`Write`] itself. The verbs run
/// the same per-row path, so every hook and every validation still runs; the
/// only addition is the change that goes out after the write succeeded.
///
/// **Exactly one change per write, and none for a write that failed.** The
/// change is built after the database answered, so a rejected validation, a
/// constraint violation and a delete that matched no row all publish nothing.
#[async_trait]
pub trait Publish: Write
where
    <Self::Entity as EntityTrait>::Model: IntoActiveModel<Self>,
{
    /// `INSERT`, then publish [`brocanth_events::ChangeKind::Created`].
    ///
    /// The row id comes from the stored row, so a generated key is the key
    /// that is published.
    async fn insert_published<C, S>(
        self,
        db: &C,
        to: &S,
    ) -> Result<<Self::Entity as EntityTrait>::Model, Error>
    where
        C: ConnectionTrait,
        S: ChangeSink + ?Sized,
        Self: Send,
    {
        let table = table_name::<Self::Entity>();
        let wanted = to.wants(table);
        let model = self.insert_checked(db).await?;
        if wanted {
            to.publish(&RowChange::created(table, row_id::<Self::Entity>(&model)));
        }
        Ok(model)
    }

    /// `UPDATE`, then publish [`brocanth_events::ChangeKind::Updated`] with
    /// the columns that were written.
    ///
    /// The column names are read before the write, because the write consumes
    /// the record, and only when the sink wants them.
    async fn update_published<C, S>(
        self,
        db: &C,
        to: &S,
    ) -> Result<<Self::Entity as EntityTrait>::Model, Error>
    where
        C: ConnectionTrait,
        S: ChangeSink + ?Sized,
        Self: Send,
    {
        let table = table_name::<Self::Entity>();
        let wanted = to.wants(table);
        let fields = if wanted {
            set_columns(&self)
        } else {
            Vec::new()
        };
        let model = self.update_checked(db).await?;
        if wanted {
            to.publish(&RowChange::updated(
                table,
                row_id::<Self::Entity>(&model),
                fields.as_slice(),
            ));
        }
        Ok(model)
    }

    /// `DELETE`, then publish [`brocanth_events::ChangeKind::Deleted`].
    ///
    /// The row id is read before the write, because afterwards there is no row
    /// to read it from. A delete that removed nothing publishes nothing.
    async fn delete_published<C, S>(self, db: &C, to: &S) -> Result<u64, Error>
    where
        C: ConnectionTrait,
        S: ChangeSink + ?Sized,
        Self: Send,
    {
        let table = table_name::<Self::Entity>();
        let id = if to.wants(table) {
            active_row_id(&self)
        } else {
            None
        };
        let removed = self.delete_checked(db).await?;
        if let Some(id) = id {
            if removed > 0 {
                to.publish(&RowChange::deleted(table, id));
            }
        }
        Ok(removed)
    }
}

impl<A> Publish for A
where
    A: ActiveModelTrait + ActiveModelBehavior,
    <A::Entity as EntityTrait>::Model: IntoActiveModel<A>,
{
}

impl<A: ActiveModelTrait> Tracked<A> {
    /// Write the record and publish what it did.
    ///
    /// The best-informed publisher in this module. A tracked record holds the
    /// row it was loaded from, so an update reports the columns that really
    /// differ and a column assigned the value it already held is not reported
    /// at all. It also knows whether it was loaded, so it publishes
    /// [`brocanth_events::ChangeKind::Created`] or
    /// [`brocanth_events::ChangeKind::Updated`] without guessing.
    pub async fn save_published<C, S>(
        self,
        db: &C,
        to: &S,
    ) -> Result<<A::Entity as EntityTrait>::Model, Error>
    where
        A: ActiveModelBehavior + Send,
        <A::Entity as EntityTrait>::Model: IntoActiveModel<A>,
        C: ConnectionTrait,
        S: ChangeSink + ?Sized,
    {
        let table = table_name::<A::Entity>();
        let wanted = to.wants(table);
        let creating = self.original().is_none();
        let fields = if wanted && !creating {
            self.changed()
        } else {
            Vec::new()
        };

        let model = self.save(db).await?;
        if wanted {
            let id = row_id::<A::Entity>(&model);
            let change = if creating {
                RowChange::created(table, id)
            } else {
                RowChange::updated(table, id, fields.as_slice())
            };
            to.publish(&change);
        }
        Ok(model)
    }
}

// ---------------------------------------------------------------------------
// Holding a change until the transaction commits
// ---------------------------------------------------------------------------

/// A sink that holds every change until somebody says the rows are committed.
///
/// A row written inside a transaction is not a row yet, and a sink cannot
/// un-say a change. So a write inside a transaction reports here, and
/// [`Pending::commit`] is what lets the changes through. Dropping a `Pending`
/// publishes nothing, which is the behaviour a rollback needs.
///
/// Held changes are owned ([`RowChange::into_owned`]), because they outlive
/// the write that made them. That allocation is paid once per change, and only
/// by a caller that chose to hold it.
pub struct Pending<S> {
    inner: S,
    held: Mutex<Vec<RowChange<'static>>>,
}

impl<S> Pending<S> {
    /// Hold changes on their way to `inner`.
    pub fn new(inner: S) -> Pending<S> {
        Pending {
            inner,
            held: Mutex::new(Vec::new()),
        }
    }

    /// How many changes are waiting.
    pub fn len(&self) -> usize {
        self.held().len()
    }

    /// `true` when nothing is waiting.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The sink behind the hold.
    pub fn inner(&self) -> &S {
        &self.inner
    }

    /// Drop every held change. Nothing is published.
    pub fn discard(self) {}

    /// A poisoned lock is still a list of changes. `ChangeSink::publish` must
    /// not panic, so this never unwraps a poisoned guard away.
    fn held(&self) -> std::sync::MutexGuard<'_, Vec<RowChange<'static>>> {
        self.held.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl<S: ChangeSink> Pending<S> {
    /// Publish everything that was held, in the order it was written.
    ///
    /// Returns how many changes went out.
    pub fn commit(self) -> usize {
        let held = self.held.into_inner().unwrap_or_else(|e| e.into_inner());
        let count = held.len();
        for change in &held {
            self.inner.publish(change);
        }
        count
    }
}

impl<S: ChangeSink> ChangeSink for Pending<S> {
    fn publish(&self, change: &RowChange<'_>) {
        self.held().push(change.clone().into_owned());
    }

    /// What the sink behind the hold says. Holding a change nobody will read
    /// would be the one cost this module promises not to pay.
    fn wants(&self, collection: &str) -> bool {
        self.inner.wants(collection)
    }
}

impl<S> fmt::Debug for Pending<S> {
    /// The count, because a `dyn ChangeSink` cannot be printed.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pending")
            .field("held", &self.len())
            .finish()
    }
}

/// A transaction and the changes its writes made, together.
///
/// The shape that makes "I published a row the transaction then rolled back"
/// unwritable: the only thing that publishes is [`Writing::commit`], and it
/// publishes after the commit returned. Dropping the guard rolls the
/// transaction back and publishes nothing, which is the same bargain
/// [`crate::testing::TestTransaction`] makes.
///
/// Derefs to the transaction, so a write takes `&*writing` as its connection
/// and `&writing` as its sink.
///
/// `T` is the connection's own transaction type, which SeaORM 2.0 makes an
/// associated type rather than one struct. A nested `begin` is a `SAVEPOINT`,
/// so a guard opened inside a test's transaction behaves the same way.
pub struct Writing<T, S> {
    txn: T,
    pending: Pending<S>,
}

impl<T, S> Writing<T, S>
where
    T: ConnectionTrait + TransactionSession + Send,
    S: ChangeSink,
{
    /// Open a transaction whose changes go to `sink` if it commits.
    pub async fn begin<C>(db: &C, sink: S) -> Result<Writing<T, S>, Error>
    where
        C: TransactionTrait<Transaction = T>,
    {
        Ok(Writing {
            txn: db.begin().await?,
            pending: Pending::new(sink),
        })
    }

    /// Commit the rows, then publish the changes. Returns how many went out.
    ///
    /// # Errors
    ///
    /// Whatever the commit failed with. Nothing is published then: the rows
    /// are not there.
    pub async fn commit(self) -> Result<usize, Error> {
        self.txn.commit().await?;
        Ok(self.pending.commit())
    }

    /// Discard the rows and the changes.
    ///
    /// # Errors
    ///
    /// Whatever the rollback failed with. Nothing is published either way.
    pub async fn rollback(self) -> Result<(), Error> {
        self.pending.discard();
        self.txn.rollback().await.map_err(Error::from)
    }

    /// How many changes are waiting on the commit.
    pub fn held(&self) -> usize {
        self.pending.len()
    }
}

impl<T, S> Deref for Writing<T, S> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.txn
    }
}

impl<T: Send + Sync, S: ChangeSink> ChangeSink for Writing<T, S> {
    fn publish(&self, change: &RowChange<'_>) {
        self.pending.publish(change);
    }

    fn wants(&self, collection: &str) -> bool {
        self.pending.wants(collection)
    }
}

impl<T, S> fmt::Debug for Writing<T, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Writing")
            .field("held", &self.pending.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brocanth_events::{ChangeKind, NoopSink};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct Recorder {
        seen: Mutex<Vec<RowChange<'static>>>,
        asked: AtomicUsize,
    }

    impl ChangeSink for Recorder {
        fn publish(&self, change: &RowChange<'_>) {
            self.seen.lock().unwrap().push(change.clone().into_owned());
        }

        fn wants(&self, _collection: &str) -> bool {
            self.asked.fetch_add(1, Ordering::SeqCst);
            true
        }
    }

    fn created() -> RowChange<'static> {
        RowChange::created("post", 1_i64)
    }

    #[test]
    fn a_held_change_reaches_the_sink_only_on_commit() {
        let recorder = Recorder::default();
        let pending = Pending::new(&recorder);
        pending.publish(&created());
        pending.publish(&RowChange::deleted("post", 2_i64));
        assert_eq!(pending.len(), 2);
        assert!(recorder.seen.lock().unwrap().is_empty());

        assert_eq!(pending.commit(), 2);
        let seen = recorder.seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].kind, ChangeKind::Created);
        assert_eq!(seen[1].row, RowId::I64(2));
    }

    #[test]
    fn a_held_change_that_is_dropped_is_never_published() {
        let recorder = Recorder::default();
        {
            let pending = Pending::new(&recorder);
            pending.publish(&created());
            assert_eq!(pending.len(), 1);
        }
        assert!(recorder.seen.lock().unwrap().is_empty());
    }

    #[test]
    fn holding_asks_the_sink_behind_it_what_it_wants() {
        // Holding a change nobody will read is the one cost this module
        // promises not to pay, so `wants` goes straight through.
        let pending = Pending::new(NoopSink);
        assert!(!pending.wants("post"));

        let recorder = Recorder::default();
        let pending = Pending::new(&recorder);
        assert!(pending.wants("post"));
        assert_eq!(recorder.asked.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_key_keeps_its_width() {
        assert_eq!(id_of(Value::Int(Some(42))), RowId::I64(42));
        assert_eq!(id_of(Value::BigInt(Some(-1))), RowId::I64(-1));
        assert_eq!(
            id_of(Value::BigUnsigned(Some(u64::MAX))),
            RowId::U128(u128::from(u64::MAX))
        );
        assert_eq!(
            id_of(Value::String(Some("slug".to_owned()))),
            RowId::Text(Cow::Borrowed("slug"))
        );
    }
}
