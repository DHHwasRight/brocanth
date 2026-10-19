//! The seam between a layer that writes rows and a layer that reacts to them.
//!
//! PLAN.md §10: *"A tiny `brocanth-events` crate carries `Change` and
//! `ChangeSink`; the model layer publishes, realtime subscribes, and neither
//! depends on the other. The same trait over `LISTEN/NOTIFY` buys
//! multi-process realtime."*
//!
//! One row changed, and somebody wants to know. That is the whole subject.
//!
//! # What this crate refuses to know
//!
//! §8 is the governing constraint, so the refusals are the design:
//!
//! * **No dependencies.** `std` only. `serde` is an optional feature, off by
//!   default, and nothing here needs it.
//! * **No policy, no request, no response, no database, no ORM.** A
//!   [`RowChange`] names a collection, a row and what happened to it. It does
//!   not know what a collection is stored in, who may see it, or what a
//!   subscriber is.
//! * **No audience and no topic index.** §10 wants realtime to invert a policy
//!   into an audience key and broadcast by map lookup. That inversion needs
//!   the *identity* of the row that changed, which is exactly what a
//!   [`RowChange`] carries, and it needs a policy, which lives in
//!   `brocanth-policy`. Realtime depends on both and does the join. This crate
//!   stops at identity on purpose: an audience type here would make the seam
//!   an opinion.
//!
//! The dependency arrow points from the model layer to this crate, and from
//! realtime to this crate. Never between them.
//!
//! # Two types are called a change, and they mean different things
//!
//! `brocanth_model::dirty::Change` is **one column**: a field name, the value
//! that was loaded, the value that will be written. It is produced by
//! `Tracked::changes()`.
//!
//! This crate's [`RowChange`] is **one row**: the collection, the row id, and
//! whether it was created, updated or deleted. PLAN.md §10 calls it `Change`;
//! the extra word is here because `use brocanth_events::Change;` beside `use
//! brocanth_model::dirty::Change;` is a collision that would be paid for with
//! an `as` rename at every call site, forever, by people who did not choose
//! it. The plan's word and the code's word differ by one syllable and this
//! paragraph reconciles them. [`ChangeSink`] keeps §10's name exactly, because
//! nothing else in the workspace claims it.
//!
//! # Publishing
//!
//! ```
//! use brocanth_events::{ChangeSink, FanOut, NoopSink, RowChange};
//!
//! // The model layer holds one of these and never asks what it is.
//! let sink: Box<dyn ChangeSink> = Box::new(NoopSink);
//!
//! // Field names from dirty tracking are `&'static str`; they are borrowed,
//! // not copied.
//! let changed: Vec<&'static str> = vec!["title", "body"];
//! let change = RowChange::updated("posts", 42_i64, changed.as_slice());
//!
//! if sink.wants(&change.collection) {
//!     sink.publish(&change);
//! }
//! # let _ = FanOut::new();
//! ```
//!
//! # Getting a change onto `LISTEN/NOTIFY`
//!
//! A downstream crate must be able to put a change on a Postgres channel, and
//! this crate must not depend on `serde` to let it. Both halves are provided,
//! and a reader should know which one they are using:
//!
//! 1. **Every field is public and every type is plain data.** A crate that
//!    already links `serde` can write six lines of `impl Serialize` against
//!    this surface, or build its own wire struct from it. This is the path
//!    that costs this crate nothing.
//! 2. **The `serde` feature** derives the impls for the common case. It is a
//!    real optional dependency behind `dep:serde`: with the feature off, no
//!    `serde` code is compiled into this crate and nothing is linked.
//!
//! A change that arrives back from a channel is a [`RowChange<'static>`] whose
//! strings are owned — see [`RowChange::into_owned`] and
//! [`ChangedFields::Owned`]. The borrowed and the owned form are one type, so
//! a sink never cares which side of the wire its argument came from.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Row identity
// ---------------------------------------------------------------------------

/// The primary key of the row that changed.
///
/// Not every entity has an `i64` id, and a change that forces a `String` would
/// put an allocation on the write path of every entity that does not need one.
/// So the two shapes that cover almost every schema are stored inline:
///
/// * [`RowId::I64`] — a serial or identity column.
/// * [`RowId::U128`] — a UUID or a ULID, as its 128 bits. Held as bits because
///   formatting is the caller's convention, not ours, and because reading them
///   as a `u128` is what lets this crate have no `uuid` dependency.
/// * [`RowId::Text`] — anything else: a slug, a natural key, or a composite
///   key the model layer has already spelled as one string. A [`Cow`], so the
///   publisher borrows and only a change that outlives its row allocates.
///
/// Comparing two ids of different variants is comparing two different columns,
/// which is a question this type does not answer: `I64(1) != Text("1")`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RowId<'a> {
    /// An integer key.
    I64(i64),
    /// A 128-bit key: a UUID, a ULID, or any other.
    U128(u128),
    /// A textual key.
    Text(Cow<'a, str>),
}

impl RowId<'_> {
    /// Detaches the id from whatever it borrowed, allocating only for
    /// [`RowId::Text`].
    pub fn into_owned(self) -> RowId<'static> {
        match self {
            RowId::I64(n) => RowId::I64(n),
            RowId::U128(n) => RowId::U128(n),
            RowId::Text(s) => RowId::Text(Cow::Owned(s.into_owned())),
        }
    }
}

impl fmt::Display for RowId<'_> {
    /// Renders the id for a channel name or a log line. A [`RowId::U128`] is
    /// written in decimal, not as a hyphenated UUID, because this crate does
    /// not know that the bits are a UUID. A layer that does know should format
    /// them itself.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RowId::I64(n) => write!(f, "{n}"),
            RowId::U128(n) => write!(f, "{n}"),
            RowId::Text(s) => f.write_str(s),
        }
    }
}

impl From<i64> for RowId<'_> {
    fn from(n: i64) -> Self {
        RowId::I64(n)
    }
}

impl From<u128> for RowId<'_> {
    fn from(n: u128) -> Self {
        RowId::U128(n)
    }
}

impl<'a> From<&'a str> for RowId<'a> {
    fn from(s: &'a str) -> Self {
        RowId::Text(Cow::Borrowed(s))
    }
}

impl From<String> for RowId<'_> {
    fn from(s: String) -> Self {
        RowId::Text(Cow::Owned(s))
    }
}

// ---------------------------------------------------------------------------
// Changed fields
// ---------------------------------------------------------------------------

/// The columns an update wrote, named as the database spells them.
///
/// §9 wants PocketBase's `:changed` as `Policy::writable() -> FieldMask`,
/// checked against the dirty tracking the model layer already has. That
/// tracking hands out `Vec<&'static str>` from `Tracked::changed()`, so
/// [`ChangedFields::Borrowed`] takes a slice of it directly and the publish
/// path copies no names and allocates nothing.
///
/// [`ChangedFields::Owned`] exists for the other direction: a change read back
/// off a `LISTEN/NOTIFY` channel has no `'static` names to borrow. One type
/// with two representations, rather than two types, so a sink written against
/// an in-process publisher works unchanged against a cross-process one.
///
/// The order is the publisher's and carries no meaning; equality is by
/// sequence, so a sink that wants set semantics should compare with
/// [`ChangedFields::contains`].
#[derive(Clone)]
pub enum ChangedFields<'a> {
    /// Names borrowed from the publisher, usually straight from dirty
    /// tracking.
    Borrowed(&'a [&'a str]),
    /// Names this change owns, because it crossed a process boundary.
    Owned(Vec<String>),
}

impl ChangedFields<'_> {
    /// No fields: a created or deleted row, and the cheap default.
    ///
    /// `const`, so a publisher that has nothing to report constructs it
    /// without touching the allocator.
    pub const NONE: ChangedFields<'static> = ChangedFields::Borrowed(&[]);

    /// How many columns were written.
    pub fn len(&self) -> usize {
        match self {
            ChangedFields::Borrowed(names) => names.len(),
            ChangedFields::Owned(names) => names.len(),
        }
    }

    /// `true` when no column was reported.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Every column name, in the publisher's order.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        // Two iterator types, one signature: a sink should never match on the
        // representation to read the names.
        let (borrowed, owned) = match self {
            ChangedFields::Borrowed(names) => (Some(names.iter().copied()), None),
            ChangedFields::Owned(names) => (None, Some(names.iter().map(String::as_str))),
        };
        borrowed
            .into_iter()
            .flatten()
            .chain(owned.into_iter().flatten())
    }

    /// `true` when this column was written.
    ///
    /// Named for `Tracked::changed_field`, which asks the same question one
    /// step earlier.
    pub fn contains(&self, field: &str) -> bool {
        self.iter().any(|name| name == field)
    }

    /// Detaches the names from whatever they borrowed.
    pub fn into_owned(self) -> ChangedFields<'static> {
        match self {
            ChangedFields::Borrowed(names) => {
                ChangedFields::Owned(names.iter().map(|n| (*n).to_owned()).collect())
            }
            ChangedFields::Owned(names) => ChangedFields::Owned(names),
        }
    }
}

impl Default for ChangedFields<'_> {
    fn default() -> Self {
        Self::NONE
    }
}

impl fmt::Debug for ChangedFields<'_> {
    /// Prints the names, not the representation, so a failing assertion reads
    /// the same whichever side of the wire built the change.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl PartialEq for ChangedFields<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter())
    }
}

impl Eq for ChangedFields<'_> {}

impl<'a> From<&'a [&'a str]> for ChangedFields<'a> {
    fn from(names: &'a [&'a str]) -> Self {
        ChangedFields::Borrowed(names)
    }
}

impl<'a> From<&'a Vec<&'a str>> for ChangedFields<'a> {
    fn from(names: &'a Vec<&'a str>) -> Self {
        ChangedFields::Borrowed(names.as_slice())
    }
}

impl From<Vec<String>> for ChangedFields<'_> {
    fn from(names: Vec<String>) -> Self {
        ChangedFields::Owned(names)
    }
}

// ---------------------------------------------------------------------------
// The change itself
// ---------------------------------------------------------------------------

/// What happened to the row.
///
/// A create carries no field list. For a new row every column is new, so the
/// list would say nothing a reader of the row does not already know, and §9's
/// `:changed` question is a question about an update. A delete carries none
/// for the same reason in reverse: there is no row left to compare against.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ChangeKind<'a> {
    /// The row is new.
    Created,
    /// The row was written, and these columns differ from what was loaded.
    Updated {
        /// The columns that were written.
        fields: ChangedFields<'a>,
    },
    /// The row is gone.
    Deleted,
}

/// One row, in one collection, and what happened to it.
///
/// Every field is public because §10 needs a downstream crate to put this on a
/// `LISTEN/NOTIFY` channel without this crate depending on `serde` to let it.
/// Plain data with no invariants to protect has nothing to gain from
/// accessors.
///
/// The lifetime is the publisher's. A sink that only reads the change keeps
/// nothing; a sink that queues it calls [`RowChange::into_owned`] and pays for
/// the copy at the point that decided to keep it.
///
/// # The audience seam
///
/// §10 broadcasts by inverting a policy into an audience key, so that one
/// write is a map lookup rather than one query per subscriber. That inversion
/// needs to name the row — `("posts", 42)` — and to know which columns moved,
/// because a change to a column no policy mentions cannot move anybody between
/// audiences. Both are here. The audience key, the policy it is derived from
/// and the index it is looked up in belong to realtime, which depends on this
/// crate and on `brocanth-policy`; neither of them appears in this file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RowChange<'a> {
    /// The table or collection the row lives in, as the database spells it.
    pub collection: Cow<'a, str>,
    /// Which row.
    pub row: RowId<'a>,
    /// What happened to it.
    pub kind: ChangeKind<'a>,
}

impl<'a> RowChange<'a> {
    /// A row was inserted.
    pub fn created(collection: impl Into<Cow<'a, str>>, row: impl Into<RowId<'a>>) -> Self {
        Self::new(collection, row, ChangeKind::Created)
    }

    /// A row was updated, writing these columns.
    pub fn updated(
        collection: impl Into<Cow<'a, str>>,
        row: impl Into<RowId<'a>>,
        fields: impl Into<ChangedFields<'a>>,
    ) -> Self {
        Self::new(
            collection,
            row,
            ChangeKind::Updated {
                fields: fields.into(),
            },
        )
    }

    /// A row was deleted.
    pub fn deleted(collection: impl Into<Cow<'a, str>>, row: impl Into<RowId<'a>>) -> Self {
        Self::new(collection, row, ChangeKind::Deleted)
    }

    /// A change of any kind.
    pub fn new(
        collection: impl Into<Cow<'a, str>>,
        row: impl Into<RowId<'a>>,
        kind: ChangeKind<'a>,
    ) -> Self {
        Self {
            collection: collection.into(),
            row: row.into(),
            kind,
        }
    }

    /// The columns this change wrote, empty for a create or a delete.
    ///
    /// Spares a sink the match on [`ChangeKind`] when all it wants is the
    /// names.
    pub fn changed_fields(&self) -> &ChangedFields<'a> {
        // A `static` rather than a temporary, so the empty case returns a
        // reference with the same lifetime as the populated one.
        static NONE: ChangedFields<'static> = ChangedFields::NONE;
        match &self.kind {
            ChangeKind::Updated { fields } => fields,
            ChangeKind::Created | ChangeKind::Deleted => &NONE,
        }
    }

    /// `true` when this change wrote that column.
    ///
    /// Named for `Tracked::changed_field`, which asks the same question of the
    /// record before it is written.
    pub fn changed_field(&self, field: &str) -> bool {
        self.changed_fields().contains(field)
    }

    /// Detaches the change from everything it borrowed, so it can be queued,
    /// sent to another thread, or held past the write that produced it.
    pub fn into_owned(self) -> RowChange<'static> {
        RowChange {
            collection: Cow::Owned(self.collection.into_owned()),
            row: self.row.into_owned(),
            kind: match self.kind {
                ChangeKind::Created => ChangeKind::Created,
                ChangeKind::Deleted => ChangeKind::Deleted,
                ChangeKind::Updated { fields } => ChangeKind::Updated {
                    fields: fields.into_owned(),
                },
            },
        }
    }
}

// ---------------------------------------------------------------------------
// The sink
// ---------------------------------------------------------------------------

/// Where a change goes. Implemented by realtime, by an audit log, by a cache
/// invalidator — by anything that cares, and by nothing this crate names.
///
/// # Object safe, deliberately
///
/// No generic methods, no `Self: Sized`, no associated types. The model layer
/// holds one `dyn ChangeSink` and must not be generic over it: a type
/// parameter there would spread through every entity, every repository and
/// every signature that touches a write, to express a choice the application
/// makes once in `main`. One virtual call per write is the cheaper trade.
///
/// # Infallible, deliberately
///
/// [`publish`](ChangeSink::publish) returns nothing. A subscriber that has
/// gone away, a full queue or a broken socket must not fail the write that
/// caused the change — the row is already committed and there is nobody left
/// to apologise to. §10 makes the overflow policy the sink's own explicit
/// decision, which is where the knowledge to make it lives.
///
/// `Send + Sync` because a sink is shared by every thread that writes.
pub trait ChangeSink: Send + Sync {
    /// Reports a change. Must not block the writer for long and must not
    /// panic.
    fn publish(&self, change: &RowChange<'_>);

    /// Whether a change in this collection would be used.
    ///
    /// This is the gate that makes an uninstalled sink free. Building a change
    /// is not free — the publisher asks dirty tracking which columns moved,
    /// which allocates a `Vec` — so a publisher calls `wants` first and skips
    /// the work when the answer is `false`.
    ///
    /// The default is `true`: a sink that does not override it is simply
    /// always asked. Answering `false` wrongly drops events silently, so a
    /// sink that is unsure must answer `true`.
    fn wants(&self, collection: &str) -> bool {
        let _ = collection;
        true
    }
}

impl<T: ChangeSink + ?Sized> ChangeSink for &T {
    fn publish(&self, change: &RowChange<'_>) {
        (**self).publish(change);
    }

    fn wants(&self, collection: &str) -> bool {
        (**self).wants(collection)
    }
}

impl<T: ChangeSink + ?Sized> ChangeSink for Box<T> {
    fn publish(&self, change: &RowChange<'_>) {
        (**self).publish(change);
    }

    fn wants(&self, collection: &str) -> bool {
        (**self).wants(collection)
    }
}

impl<T: ChangeSink + ?Sized> ChangeSink for Arc<T> {
    fn publish(&self, change: &RowChange<'_>) {
        (**self).publish(change);
    }

    fn wants(&self, collection: &str) -> bool {
        (**self).wants(collection)
    }
}

/// The sink for an application that installed none.
///
/// §8 says a plugin nobody asked for contributes nothing, and §10 says the
/// model layer publishes whether or not realtime exists. This is how both are
/// true at once: the model layer always has a sink, so there is no
/// `Option<Box<dyn ChangeSink>>` and no `if let` on the write path, and the
/// sink it has reports [`wants`](ChangeSink::wants) `false`, so the publisher
/// never builds the change in the first place. What is left is a virtual call
/// returning `false`.
///
/// It is also what a test installs to assert that nothing listens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoopSink;

impl ChangeSink for NoopSink {
    fn publish(&self, _change: &RowChange<'_>) {}

    /// Always `false`. Nothing downstream will read the change, so nothing
    /// upstream should pay to build it.
    fn wants(&self, _collection: &str) -> bool {
        false
    }
}

/// Every installed sink, behind one sink.
///
/// The model layer holds one sink; an application may install realtime, an
/// audit log and a cache invalidator. This is the adapter, and it is the whole
/// of this crate's opinion about how several sinks coexist. Order is
/// registration order; a sink that panics takes the writer with it, which is
/// the same contract [`ChangeSink`] already states.
#[derive(Default)]
pub struct FanOut {
    sinks: Vec<Box<dyn ChangeSink>>,
}

impl FanOut {
    /// An empty fan-out, which behaves exactly like [`NoopSink`].
    pub fn new() -> Self {
        Self { sinks: Vec::new() }
    }

    /// Adds a sink, for a builder chain.
    #[must_use]
    pub fn with(mut self, sink: impl ChangeSink + 'static) -> Self {
        self.push(sink);
        self
    }

    /// Adds a sink.
    pub fn push(&mut self, sink: impl ChangeSink + 'static) {
        self.sinks.push(Box::new(sink));
    }

    /// How many sinks are installed.
    pub fn len(&self) -> usize {
        self.sinks.len()
    }

    /// `true` when no sink is installed.
    pub fn is_empty(&self) -> bool {
        self.sinks.is_empty()
    }
}

impl fmt::Debug for FanOut {
    /// A `dyn ChangeSink` cannot be printed, so the count is what there is to
    /// say.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FanOut")
            .field("sinks", &self.sinks.len())
            .finish()
    }
}

impl ChangeSink for FanOut {
    fn publish(&self, change: &RowChange<'_>) {
        for sink in &self.sinks {
            sink.publish(change);
        }
    }

    /// `true` when any installed sink wants the collection. An empty fan-out
    /// answers `false`, which is what makes an application that installed no
    /// sink pay exactly what [`NoopSink`] pays.
    fn wants(&self, collection: &str) -> bool {
        self.sinks.iter().any(|sink| sink.wants(collection))
    }
}

// ---------------------------------------------------------------------------
// serde
// ---------------------------------------------------------------------------

/// `ChangedFields` is written by hand because its borrowed representation
/// cannot be derived: nothing can deserialise into `&'static str`. The wire
/// form of both representations is a sequence of strings, and a change that
/// arrives from a channel is always [`ChangedFields::Owned`].
#[cfg(feature = "serde")]
impl serde::Serialize for ChangedFields<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter())
    }
}

#[cfg(feature = "serde")]
impl<'de, 'a> serde::Deserialize<'de> for ChangedFields<'a> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(ChangedFields::Owned(serde::Deserialize::deserialize(
            deserializer,
        )?))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A sink that records what it was given, which is what every test here
    /// needs and nothing in the public surface should provide.
    #[derive(Default)]
    struct Recorder {
        seen: Arc<AtomicUsize>,
        last: std::sync::Mutex<Option<RowChange<'static>>>,
        wanted: bool,
    }

    impl Recorder {
        fn new(seen: Arc<AtomicUsize>) -> Self {
            Self {
                seen,
                last: std::sync::Mutex::new(None),
                wanted: true,
            }
        }

        fn uninterested(seen: Arc<AtomicUsize>) -> Self {
            Self {
                wanted: false,
                ..Self::new(seen)
            }
        }
    }

    impl ChangeSink for Recorder {
        fn publish(&self, change: &RowChange<'_>) {
            self.seen.fetch_add(1, Ordering::SeqCst);
            *self.last.lock().unwrap() = Some(change.clone().into_owned());
        }

        fn wants(&self, _collection: &str) -> bool {
            self.wanted
        }
    }

    fn update() -> RowChange<'static> {
        RowChange::updated("posts", 42_i64, ChangedFields::Borrowed(&["title", "body"]))
    }

    #[test]
    fn a_noop_sink_accepts_a_change_and_does_nothing_with_it() {
        let sink = NoopSink;
        sink.publish(&update());
        // Nothing to assert but the absence of an effect, so the guarantee is
        // the one below: the publisher is told not to bother at all.
        assert!(!sink.wants("posts"));
    }

    #[test]
    fn a_noop_sink_tells_the_publisher_not_to_build_the_change() {
        assert!(!NoopSink.wants("posts"));
        assert!(!NoopSink.wants("anything_else"));
    }

    #[test]
    fn an_empty_fan_out_costs_what_a_noop_sink_costs() {
        let fan = FanOut::new();
        assert!(fan.is_empty());
        assert!(!fan.wants("posts"));
        fan.publish(&update());
    }

    #[test]
    fn a_fan_out_sink_reaches_every_subscriber() {
        let first = Arc::new(AtomicUsize::new(0));
        let second = Arc::new(AtomicUsize::new(0));
        let third = Arc::new(AtomicUsize::new(0));

        let fan = FanOut::new()
            .with(Recorder::new(Arc::clone(&first)))
            .with(Recorder::new(Arc::clone(&second)))
            .with(Recorder::new(Arc::clone(&third)));

        assert_eq!(fan.len(), 3);
        fan.publish(&update());

        assert_eq!(first.load(Ordering::SeqCst), 1);
        assert_eq!(second.load(Ordering::SeqCst), 1);
        assert_eq!(third.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_fan_out_wants_a_collection_when_any_one_subscriber_does() {
        let count = Arc::new(AtomicUsize::new(0));
        let quiet = FanOut::new().with(Recorder::uninterested(Arc::clone(&count)));
        assert!(!quiet.wants("posts"));

        let mixed = FanOut::new()
            .with(Recorder::uninterested(Arc::clone(&count)))
            .with(Recorder::new(Arc::clone(&count)));
        assert!(mixed.wants("posts"));
    }

    #[test]
    fn an_update_reports_exactly_the_fields_that_changed() {
        let change = update();
        assert!(change.changed_field("title"));
        assert!(change.changed_field("body"));
        assert!(!change.changed_field("author_id"));
        assert_eq!(change.changed_fields().len(), 2);
        assert_eq!(
            change.changed_fields().iter().collect::<Vec<_>>(),
            ["title", "body"]
        );
    }

    #[test]
    fn a_create_and_a_delete_report_no_changed_fields() {
        for change in [
            RowChange::created("posts", 1_i64),
            RowChange::deleted("posts", 1_i64),
        ] {
            assert!(change.changed_fields().is_empty());
            assert!(!change.changed_field("title"));
        }
    }

    #[test]
    fn a_field_list_from_dirty_tracking_is_borrowed_rather_than_copied() {
        // `Tracked::changed()` hands out exactly this.
        let changed: Vec<&'static str> = vec!["title"];
        let change = RowChange::updated("posts", 42_i64, changed.as_slice());

        // The proxy for "publishing allocates nothing": every part of the
        // change still points at memory the publisher already had.
        assert!(matches!(change.collection, Cow::Borrowed(_)));
        assert!(matches!(
            change.kind,
            ChangeKind::Updated {
                fields: ChangedFields::Borrowed(_)
            }
        ));
    }

    #[test]
    fn a_sink_can_be_stored_as_a_boxed_trait_object() {
        // The guarantee is that this compiles: the model layer holds one of
        // these and is not generic over the sink.
        let count = Arc::new(AtomicUsize::new(0));
        let sinks: Vec<Box<dyn ChangeSink>> = vec![
            Box::new(NoopSink),
            Box::new(Recorder::new(Arc::clone(&count))),
            Box::new(FanOut::new().with(Recorder::new(Arc::clone(&count)))),
        ];

        for sink in &sinks {
            sink.publish(&update());
        }
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_boxed_sink_is_itself_a_sink_so_a_fan_out_can_hold_one() {
        let count = Arc::new(AtomicUsize::new(0));
        let boxed: Box<dyn ChangeSink> = Box::new(Recorder::new(Arc::clone(&count)));
        let shared: Arc<dyn ChangeSink> = Arc::new(Recorder::new(Arc::clone(&count)));

        let fan = FanOut::new().with(boxed).with(shared);
        fan.publish(&update());
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn an_owned_change_carries_everything_the_borrowed_one_did() {
        let count = Arc::new(AtomicUsize::new(0));
        let recorder = Recorder::new(Arc::clone(&count));

        {
            // A collection name and field names that do not outlive this block.
            let collection = String::from("posts");
            let fields = vec!["title".to_string(), "body".to_string()];
            let change = RowChange::updated(
                collection.as_str(),
                "slug-key",
                ChangedFields::Owned(fields),
            );
            recorder.publish(&change);
        }

        let kept = recorder.last.lock().unwrap().take().unwrap();
        assert_eq!(kept.collection, "posts");
        assert_eq!(kept.row, RowId::Text(Cow::Borrowed("slug-key")));
        assert!(kept.changed_field("body"));
    }

    #[test]
    fn a_row_id_keeps_integer_and_uuid_keys_without_a_string() {
        assert_eq!(RowId::from(42_i64).to_string(), "42");
        assert_eq!(RowId::from(7_u128).to_string(), "7");
        assert_eq!(RowId::from("slug").to_string(), "slug");

        // Two different columns, not two spellings of one key.
        assert_ne!(RowId::I64(1), RowId::Text(Cow::Borrowed("1")));

        // Detaching an inline id allocates nothing to detach.
        assert_eq!(RowId::U128(9).into_owned(), RowId::U128(9));
    }

    #[test]
    fn two_field_lists_are_equal_whichever_side_of_the_wire_built_them() {
        let borrowed = ChangedFields::Borrowed(&["title", "body"]);
        let owned = ChangedFields::Owned(vec!["title".into(), "body".into()]);
        assert_eq!(borrowed, owned);
        assert_eq!(borrowed.clone().into_owned(), owned);
        assert_eq!(format!("{borrowed:?}"), format!("{owned:?}"));
    }

    #[cfg(feature = "serde")]
    #[test]
    fn every_type_can_be_put_on_a_notify_channel() {
        // The guarantee is that this compiles. Which format a change is
        // written in belongs to the crate that owns the channel, so this crate
        // keeps no format dependency, not even to test with.
        fn carried<T: serde::Serialize + serde::de::DeserializeOwned>() {}
        carried::<RowChange<'static>>();
        carried::<RowId<'static>>();
        carried::<ChangeKind<'static>>();
        carried::<ChangedFields<'static>>();
    }

    // -- the contract under the parts nothing exercised ---------------------

    /// A sink that overrides nothing, to read the defaults off the trait.
    struct Plain(Arc<AtomicUsize>);

    impl ChangeSink for Plain {
        fn publish(&self, _change: &RowChange<'_>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// A sink that panics, which `ChangeSink` tells an implementor not to be.
    struct Panics;

    impl ChangeSink for Panics {
        fn publish(&self, _change: &RowChange<'_>) {
            panic!("this sink does what the trait says not to");
        }
    }

    #[test]
    fn a_sink_that_overrides_nothing_is_always_asked() {
        // The documented default: `wants` is `true`, so a sink that does not
        // think about the question still gets every change. Answering `false`
        // wrongly drops events silently, which is why this is the default and
        // not the other one.
        let count = Arc::new(AtomicUsize::new(0));
        let sink = Plain(Arc::clone(&count));
        assert!(sink.wants("posts"));
        assert!(sink.wants("anything at all"));
        assert!(sink.wants(""));

        // And through the fan-out, which asks each sink in turn.
        let fan = FanOut::new().with(Plain(Arc::clone(&count)));
        assert!(fan.wants("posts"));
        fan.publish(&update());
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_borrowed_sink_is_itself_a_sink() {
        // `&T`, `Box<T>` and `Arc<T>` all forward. The boxed and shared forms
        // are covered above; this is the one a caller reaches for when it
        // holds a sink it does not own.
        let count = Arc::new(AtomicUsize::new(0));
        let owned = Recorder::new(Arc::clone(&count));
        let borrowed: &dyn ChangeSink = &owned;
        assert!(borrowed.wants("posts"));
        borrowed.publish(&update());

        fn takes_a_sink(sink: impl ChangeSink) {
            sink.publish(&update());
        }
        takes_a_sink(&owned);
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    /// What a panicking sink costs, measured rather than asserted in prose.
    ///
    /// `ChangeSink::publish` says "must not panic" and `FanOut` says a sink
    /// that panics "takes the writer with it". Both are true, and the second
    /// sentence understates it: the fan-out publishes in registration order
    /// with no guard, so a panic in the second of three sinks also means the
    /// third never hears about the change. The unwind reaches the writer,
    /// and the fan-out is left half delivered.
    ///
    /// This is a deliberate design -- a guard would need `catch_unwind`,
    /// which needs `UnwindSafe` bounds on a trait that is meant to be
    /// implementable by anybody -- so the test pins the behaviour rather than
    /// asking for a different one. What it is here to stop is the behaviour
    /// changing quietly, in either direction.
    #[test]
    fn a_sink_that_panics_takes_the_writer_and_the_sinks_after_it() {
        let before = Arc::new(AtomicUsize::new(0));
        let after = Arc::new(AtomicUsize::new(0));

        let fan = FanOut::new()
            .with(Recorder::new(Arc::clone(&before)))
            .with(Panics)
            .with(Recorder::new(Arc::clone(&after)));
        assert_eq!(fan.len(), 3);

        // `wants` does not touch `publish`, so asking is still safe.
        assert!(fan.wants("posts"));

        let hushed = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            fan.publish(&update());
        }));
        std::panic::set_hook(hushed);

        assert!(outcome.is_err(), "the panic reached the writer");
        assert_eq!(
            before.load(Ordering::SeqCst),
            1,
            "the sink before it saw it"
        );
        assert_eq!(
            after.load(Ordering::SeqCst),
            0,
            "and the sink after it never did"
        );

        // The fan-out is not poisoned: a second write delivers to the same
        // two of three. The hole is per publish, not permanent.
        let hushed = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            fan.publish(&update());
        }));
        std::panic::set_hook(hushed);
        assert_eq!(before.load(Ordering::SeqCst), 2);
        assert_eq!(after.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn nothing_listens_and_the_publisher_is_told_so_before_it_builds_anything() {
        // "What happens with no sink", at the level the publisher sees. The
        // model layer holds a sink unconditionally, so "no sink" is one of
        // these two values and never an `Option`.
        for sink in [
            Box::new(NoopSink) as Box<dyn ChangeSink>,
            Box::new(FanOut::new()),
        ] {
            assert!(!sink.wants("posts"));
            // Publishing anyway is still defined, because a publisher that
            // ignores `wants` must not be a crash.
            sink.publish(&update());
            sink.publish(&RowChange::created("posts", 1_i64));
            sink.publish(&RowChange::deleted("posts", 1_i64));
        }
    }

    #[cfg(feature = "serde")]
    #[test]
    fn a_field_list_off_a_channel_owns_its_names() {
        // `ChangedFields` is the one `Deserialize` written by hand, and the
        // claim it carries is that a change arriving from a channel is always
        // the owned representation -- nothing can deserialise into
        // `&'static str`. Checked through serde's own value deserialiser, so
        // this crate still links no format.
        use serde::de::value::{Error as ValueError, SeqDeserializer};
        use serde::Deserialize;

        let wire = SeqDeserializer::<_, ValueError>::new(["title", "body"].into_iter());
        let fields = ChangedFields::deserialize(wire).expect("a sequence of names");
        assert!(
            matches!(fields, ChangedFields::Owned(_)),
            "a change off a channel borrowed something"
        );
        // And it is the same value a publisher would have borrowed, which is
        // what lets one sink serve both sides of the wire.
        assert_eq!(fields, ChangedFields::Borrowed(&["title", "body"]));
        assert!(fields.contains("body"));

        let empty = SeqDeserializer::<_, ValueError>::new(std::iter::empty::<&str>());
        let fields = ChangedFields::deserialize(empty).expect("an empty sequence");
        assert!(fields.is_empty());
        assert_eq!(fields, ChangedFields::NONE);
    }
}
