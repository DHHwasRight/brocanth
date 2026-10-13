//! Multi-process realtime over Postgres `LISTEN/NOTIFY`.
//!
//! PLAN.md §10:
//!
//! > The same trait over `LISTEN/NOTIFY` buys multi-process realtime — which
//! > PocketBase cannot have at any price, its broker being in-memory beside an
//! > embedded database.
//!
//! "The same trait" is `brocanth_events::ChangeSink`. An application running
//! four web processes installs two sinks: the local [`Broker`](crate::Broker),
//! which serves the connections this process holds, and a [`NotifySink`],
//! which puts the change on a Postgres channel so the other three serve
//! theirs. A listener in each process decodes with [`decode`] and publishes
//! into its own broker. Nothing else changes, and no subscriber moves.
//!
//! Behind the `listen-notify` feature, off by default: §8.2 says a feature
//! that is off must mean the code is absent, and a single-process deployment
//! has no business linking a JSON codec to broadcast in memory.
//!
//! # No driver here
//!
//! [`Notifier`] has one method and the application implements it in about ten
//! lines against whatever it already opened:
//!
//! ```ignore
//! impl Notifier for MyPool {
//!     fn notify(&self, channel: &str, payload: &str) -> Result<(), NotifyError> {
//!         // `pg_notify` rather than `NOTIFY`, because the channel name is a
//!         // value there and an identifier in the statement form.
//!         sqlx::query("SELECT pg_notify($1, $2)")
//!             .bind(channel)
//!             .bind(payload)
//!             .execute(&self.0)
//!             .map_err(|e| NotifyError::new(e.to_string()))?;
//!         Ok(())
//!     }
//! }
//! ```
//!
//! A driver in this crate would also re-open the `links = "sqlite3"` conflict
//! that `brocanth-policy`'s manifest documents, for nothing.
//!
//! # The 8000-byte limit, and the pointer
//!
//! PostgreSQL's `NOTIFY_PAYLOAD_MAX_LENGTH` is `BLCKSZ - NAMEDATALEN - 128`,
//! which is 8192 − 64 − 128 = **8000**, and the check rejects a payload whose
//! length is *greater than or equal to* that. So the real limit is
//! [`MAX_PAYLOAD`] = 7999 bytes, and a payload that reaches it does not get
//! truncated — the transaction **errors**. A broadcaster that can fail a write
//! is worse than one that loses a field list.
//!
//! So: **a payload that does not fit becomes a pointer to the row rather than
//! the row.** The only unbounded part of a change is the list of changed field
//! names — a create and a delete carry none, and a collection name and a row
//! id are bounded by the schema. [`encode`] therefore drops the field list and
//! sets a truncation flag, which is exactly a pointer: *this row, in this
//! collection, changed; go and read it.*
//!
//! Nothing about audience resolution is lost by that. The broker computes an
//! audience from the columns **its index keys on**, read through
//! [`RowReader`](crate::RowReader) on the receiving side — never from the
//! changed field names, which are advisory. What a truncated change costs is
//! that an emitter cannot say *which* fields moved.
//! [`Inbound::truncated`] says so rather than letting an emitter believe
//! nothing changed.
//!
//! If even the pointer does not fit — a row id of several kilobytes — there is
//! nothing left to elide and [`encode`] returns [`PayloadTooLarge`]. Stated,
//! rather than silently dropped.
//!
//! # Not sending a process its own changes back
//!
//! A process that both notifies and listens receives its own notification and
//! would deliver every frame twice. Every envelope carries an [`Origin`], and
//! [`Inbound::is_local`] is the one line a listener needs:
//!
//! ```ignore
//! let inbound = decode(&channel, &payload)?;
//! if !inbound.is_local() {
//!     broker.publish_change(&inbound.change);
//! }
//! ```
//!
//! # Channel names
//!
//! One channel per collection, so a process wakes only for collections it has
//! subscribers on — §10's "index by topic", extended across processes. A
//! channel name is an identifier, capped at `NAMEDATALEN - 1` = 63 bytes, so
//! [`channel_for`] uses `bc_<collection>` when that fits and is a bare
//! identifier, and `bc_h_<16 hex>` of an FNV-1a hash when it does not.
//!
//! The hash is written out here rather than taken from `std::hash::
//! DefaultHasher`, and that is not an optimisation: `DefaultHasher`'s output
//! is explicitly not stable across releases, so two processes built with
//! different compilers would `LISTEN` on different channels and silently never
//! hear each other. A wire format needs a hash that is fixed forever.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use brocanth_events::{ChangeKind, ChangeSink, ChangedFields, RowChange, RowId};
use serde::{Deserialize, Serialize};

/// The largest payload `NOTIFY` accepts, in bytes.
///
/// `BLCKSZ - NAMEDATALEN - 128` is 8000 and the check is `>=`, so 7999 is the
/// last length that works. Over it, the transaction fails.
pub const MAX_PAYLOAD: usize = 7999;

/// The longest a channel name may be, in bytes: `NAMEDATALEN - 1`.
pub const MAX_CHANNEL: usize = 63;

/// The wire format version this crate writes and accepts.
pub const VERSION: u8 = 1;

// ---------------------------------------------------------------------------
// Origin
// ---------------------------------------------------------------------------

/// Which process sent a notification.
///
/// Not an identity and not a secret — just enough for a process to recognise
/// its own echo. Derived once from the process id and the clock, so two
/// processes on one machine differ and a restart differs from what it replaced.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Origin(u64);

impl Origin {
    /// This process's origin, computed once.
    pub fn process() -> Origin {
        static ORIGIN: OnceLock<Origin> = OnceLock::new();
        *ORIGIN.get_or_init(|| {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);
            let mut hash = fnv1a(&std::process::id().to_le_bytes());
            hash ^= fnv1a(&nanos.to_le_bytes());
            Origin(hash)
        })
    }

    /// A chosen origin, for a test that wants two processes in one.
    pub fn new(id: u64) -> Origin {
        Origin(id)
    }

    /// The raw value.
    pub fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

/// FNV-1a, 64 bit. Fixed forever, because a wire format cannot change its
/// hash.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

// ---------------------------------------------------------------------------
// Channels
// ---------------------------------------------------------------------------

/// The channel a collection's changes travel on.
///
/// `bc_<collection>` when the name is a bare identifier that fits in
/// [`MAX_CHANNEL`], and `bc_h_<16 hex>` otherwise. Both forms are stable
/// across processes and releases, which is the only property that matters:
/// the sender and the listener must agree without talking to each other.
pub fn channel_for(collection: &str) -> String {
    let plain = !collection.is_empty()
        && collection
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        && !collection.as_bytes()[0].is_ascii_digit();
    if plain && collection.len() + 3 <= MAX_CHANNEL {
        format!("bc_{collection}")
    } else {
        format!("bc_h_{:016x}", fnv1a(collection.as_bytes()))
    }
}

// ---------------------------------------------------------------------------
// The wire format
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
enum WireId {
    /// An integer key.
    #[serde(rename = "n")]
    Int(i64),
    /// A 128-bit key, as decimal text. Text rather than a JSON number because
    /// a 128-bit integer is outside what every JSON reader agrees on, and a
    /// row id that silently loses its low bits is a row id that addresses the
    /// wrong row.
    #[serde(rename = "u")]
    Big(String),
    /// A textual key.
    #[serde(rename = "s")]
    Text(String),
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    v: u8,
    o: Origin,
    c: String,
    i: WireId,
    k: Kind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    f: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    t: bool,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
enum Kind {
    #[serde(rename = "c")]
    Created,
    #[serde(rename = "u")]
    Updated,
    #[serde(rename = "d")]
    Deleted,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// One encoded notification: where to send it and what to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notification {
    /// The channel, from [`channel_for`].
    pub channel: String,
    /// The payload, at most [`MAX_PAYLOAD`] bytes.
    pub payload: String,
    /// `true` when the field list was dropped to make it fit, so the change is
    /// a pointer to the row rather than a description of it.
    pub truncated: bool,
}

/// The change did not fit even without its field list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "a change needs {needed} bytes and NOTIFY accepts {MAX_PAYLOAD}, with nothing left to elide"
)]
pub struct PayloadTooLarge {
    /// How many bytes the pointer form still needed.
    pub needed: usize,
}

/// Encode a change for a channel.
///
/// Drops the changed field list and sets [`Notification::truncated`] when the
/// full form does not fit. See the module documentation for why that is a
/// pointer rather than a loss.
pub fn encode(origin: Origin, change: &RowChange<'_>) -> Result<Notification, PayloadTooLarge> {
    let channel = channel_for(&change.collection);
    let envelope = Envelope {
        v: VERSION,
        o: origin,
        c: change.collection.clone().into_owned(),
        i: wire_id(&change.row),
        k: match change.kind {
            ChangeKind::Created => Kind::Created,
            ChangeKind::Updated { .. } => Kind::Updated,
            ChangeKind::Deleted => Kind::Deleted,
        },
        f: change.changed_fields().iter().map(str::to_owned).collect(),
        t: false,
    };
    // `to_string` on a struct of owned primitives cannot fail.
    let payload = serde_json::to_string(&envelope).unwrap_or_default();
    if payload.len() <= MAX_PAYLOAD {
        return Ok(Notification {
            channel,
            payload,
            truncated: false,
        });
    }

    let pointer = Envelope {
        f: Vec::new(),
        t: true,
        ..envelope
    };
    let payload = serde_json::to_string(&pointer).unwrap_or_default();
    if payload.len() > MAX_PAYLOAD {
        return Err(PayloadTooLarge {
            needed: payload.len(),
        });
    }
    Ok(Notification {
        channel,
        payload,
        truncated: true,
    })
}

fn wire_id(id: &RowId<'_>) -> WireId {
    match id {
        RowId::I64(n) => WireId::Int(*n),
        RowId::U128(n) => WireId::Big(n.to_string()),
        RowId::Text(s) => WireId::Text(s.clone().into_owned()),
    }
}

/// A change that arrived from another process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inbound {
    /// The change, owned, ready to publish into the local broker.
    pub change: RowChange<'static>,
    /// Which process sent it.
    pub origin: Origin,
    /// `true` when the field list was dropped to fit. The change still names
    /// the row exactly; what is unknown is which columns moved.
    pub truncated: bool,
}

impl Inbound {
    /// `true` when this process sent it. A listener skips those, or it
    /// delivers every frame twice.
    pub fn is_local(&self) -> bool {
        self.origin == Origin::process()
    }

    /// `true` when it came from somewhere else.
    pub fn is_remote(&self) -> bool {
        !self.is_local()
    }
}

/// Why a payload could not be read.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    /// Not the JSON this crate writes.
    #[error("the notification payload is not a change envelope: {0}")]
    Malformed(#[from] serde_json::Error),
    /// A version this build does not know. A newer process is on the channel,
    /// and guessing at its format is how a rolling deploy corrupts a stream.
    #[error("the notification is version {found}, and this build speaks {VERSION}")]
    Version {
        /// What arrived.
        found: u8,
    },
    /// A 128-bit id that is not a number.
    #[error("the notification carries a 128-bit row id that is not a number")]
    BadRowId,
}

/// Read a payload off a channel.
///
/// The `channel` argument is accepted and not trusted: the collection is in
/// the payload, because a channel name may be a hash and a hash does not
/// invert.
pub fn decode(channel: &str, payload: &str) -> Result<Inbound, DecodeError> {
    let _ = channel;
    let envelope: Envelope = serde_json::from_str(payload)?;
    if envelope.v != VERSION {
        return Err(DecodeError::Version { found: envelope.v });
    }
    let row = match envelope.i {
        WireId::Int(n) => RowId::I64(n),
        WireId::Big(text) => RowId::U128(text.parse().map_err(|_| DecodeError::BadRowId)?),
        WireId::Text(text) => RowId::Text(std::borrow::Cow::Owned(text)),
    };
    let kind = match envelope.k {
        Kind::Created => ChangeKind::Created,
        Kind::Deleted => ChangeKind::Deleted,
        Kind::Updated => ChangeKind::Updated {
            fields: ChangedFields::Owned(envelope.f),
        },
    };
    Ok(Inbound {
        change: RowChange::new(envelope.c, row, kind),
        origin: envelope.o,
        truncated: envelope.t,
    })
}

// ---------------------------------------------------------------------------
// The sink
// ---------------------------------------------------------------------------

/// Whatever can run `pg_notify`.
///
/// One method, so the application's own pool implements it without this crate
/// taking a driver. See the module documentation for the ten lines.
pub trait Notifier: Send + Sync {
    /// Send a payload on a channel.
    fn notify(&self, channel: &str, payload: &str) -> Result<(), NotifyError>;
}

/// The notifier failed. Carries a message because this crate cannot name the
/// driver's error type.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct NotifyError(String);

impl NotifyError {
    /// Wrap a driver's message.
    pub fn new(message: impl Into<String>) -> NotifyError {
        NotifyError(message.into())
    }
}

impl<F> Notifier for F
where
    F: Fn(&str, &str) -> Result<(), NotifyError> + Send + Sync,
{
    fn notify(&self, channel: &str, payload: &str) -> Result<(), NotifyError> {
        self(channel, payload)
    }
}

/// A [`ChangeSink`] that puts changes on a Postgres channel.
///
/// Install it beside the local broker, not instead of it: the broker serves
/// this process's connections and the sink serves everybody else's.
///
/// # Failure is counted, never raised
///
/// `ChangeSink::publish` returns nothing, and `brocanth-events` says why:
/// *"a subscriber that has gone away, a full queue or a broken socket must not
/// fail the write that caused the change — the row is already committed and
/// there is nobody left to apologise to."* A notifier that errors increments
/// [`NotifySink::failures`]; a change that cannot fit increments
/// [`NotifySink::oversized`]. Both are numbers an operator watches, and
/// neither takes a write down with it.
pub struct NotifySink<N> {
    notifier: N,
    origin: Origin,
    collections: Option<Vec<String>>,
    sent: AtomicU64,
    truncated: AtomicU64,
    oversized: AtomicU64,
    failures: AtomicU64,
}

impl<N: Notifier> NotifySink<N> {
    /// Notify for every collection.
    pub fn new(notifier: N) -> NotifySink<N> {
        NotifySink {
            notifier,
            origin: Origin::process(),
            collections: None,
            sent: AtomicU64::new(0),
            truncated: AtomicU64::new(0),
            oversized: AtomicU64::new(0),
            failures: AtomicU64::new(0),
        }
    }

    /// Notify only for these collections.
    ///
    /// What makes [`ChangeSink::wants`] answer `false`, so the publisher never
    /// builds a change nobody is going to send. An application knows which of
    /// its tables are broadcast and which are a write-ahead log nobody watches.
    #[must_use]
    pub fn only(mut self, collections: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.collections = Some(collections.into_iter().map(Into::into).collect());
        self
    }

    /// Send under a chosen origin, for a test running two "processes" in one.
    #[must_use]
    pub fn origin(mut self, origin: Origin) -> Self {
        self.origin = origin;
        self
    }

    /// How many notifications went out.
    pub fn sent(&self) -> u64 {
        self.sent.load(Ordering::Relaxed)
    }

    /// How many went out as a pointer because the full form did not fit.
    pub fn truncated(&self) -> u64 {
        self.truncated.load(Ordering::Relaxed)
    }

    /// How many could not be sent at all, pointer or not.
    pub fn oversized(&self) -> u64 {
        self.oversized.load(Ordering::Relaxed)
    }

    /// How many the notifier refused.
    pub fn failures(&self) -> u64 {
        self.failures.load(Ordering::Relaxed)
    }
}

impl<N: Notifier> ChangeSink for NotifySink<N> {
    fn publish(&self, change: &RowChange<'_>) {
        match encode(self.origin, change) {
            Ok(notification) => {
                if notification.truncated {
                    self.truncated.fetch_add(1, Ordering::Relaxed);
                }
                match self
                    .notifier
                    .notify(&notification.channel, &notification.payload)
                {
                    Ok(()) => {
                        self.sent.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(_) => {
                        self.failures.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            Err(_) => {
                self.oversized.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn wants(&self, collection: &str) -> bool {
        match &self.collections {
            None => true,
            Some(allowed) => allowed.iter().any(|held| held == collection),
        }
    }
}

impl<N> fmt::Debug for NotifySink<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NotifySink")
            .field("origin", &self.origin)
            .field("collections", &self.collections)
            .field("sent", &self.sent)
            .field("truncated", &self.truncated)
            .field("oversized", &self.oversized)
            .field("failures", &self.failures)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn a_change_round_trips() {
        let fields: &[&str] = &["title", "body"];
        let change = RowChange::updated("posts", 42i64, fields);
        let notification = encode(Origin::new(7), &change).unwrap();
        assert_eq!(notification.channel, "bc_posts");
        assert!(!notification.truncated);

        let inbound = decode(&notification.channel, &notification.payload).unwrap();
        assert_eq!(inbound.origin, Origin::new(7));
        assert!(!inbound.truncated);
        assert_eq!(inbound.change, change.clone().into_owned());
    }

    #[test]
    fn every_row_id_kind_round_trips() {
        for row in [
            RowId::I64(-5),
            RowId::U128(u128::MAX),
            RowId::Text("a slug".into()),
        ] {
            let change = RowChange::created("posts", row.clone());
            let notification = encode(Origin::new(1), &change).unwrap();
            let inbound = decode(&notification.channel, &notification.payload).unwrap();
            assert_eq!(inbound.change.row, row.into_owned());
        }
    }

    #[test]
    fn a_payload_that_does_not_fit_becomes_a_pointer_to_the_row() {
        // A wide table, updated everywhere: the field list is the only part of
        // a change that can grow without bound.
        let names: Vec<String> = (0..600).map(|n| format!("column_number_{n:04}")).collect();
        let change = RowChange::updated("posts", 42i64, names);
        let full = serde_json::to_string(&change.changed_fields().iter().collect::<Vec<_>>())
            .unwrap()
            .len();
        assert!(full > MAX_PAYLOAD, "the fixture has to actually overflow");

        let notification = encode(Origin::new(1), &change).unwrap();
        assert!(notification.truncated);
        assert!(notification.payload.len() <= MAX_PAYLOAD);

        let inbound = decode(&notification.channel, &notification.payload).unwrap();
        assert!(inbound.truncated, "the receiver must know the list is gone");
        assert_eq!(inbound.change.row, RowId::I64(42));
        assert_eq!(inbound.change.collection, "posts");
        assert!(inbound.change.changed_fields().is_empty());
    }

    #[test]
    fn a_row_id_too_large_for_a_pointer_is_an_error_rather_than_a_silent_drop() {
        let id = "x".repeat(MAX_PAYLOAD + 1);
        let change = RowChange::created("posts", id);
        let error = encode(Origin::new(1), &change).unwrap_err();
        assert!(error.needed > MAX_PAYLOAD);
    }

    #[test]
    fn a_channel_name_fits_postgres() {
        assert_eq!(channel_for("posts"), "bc_posts");
        let long = "a".repeat(200);
        let hashed = channel_for(&long);
        assert!(hashed.len() <= MAX_CHANNEL);
        assert_eq!(hashed, channel_for(&long), "stable");
        assert_ne!(channel_for("Posts"), "bc_Posts", "not a bare identifier");
        assert!(channel_for("posts; drop table").starts_with("bc_h_"));
    }

    #[test]
    fn a_version_this_build_does_not_know_is_refused_rather_than_guessed_at() {
        let payload = r#"{"v":99,"o":1,"c":"posts","i":{"n":1},"k":"c"}"#;
        assert!(matches!(
            decode("bc_posts", payload),
            Err(DecodeError::Version { found: 99 })
        ));
    }

    #[test]
    fn a_process_recognises_its_own_echo() {
        let change = RowChange::created("posts", 1i64);
        let mine = encode(Origin::process(), &change).unwrap();
        assert!(decode(&mine.channel, &mine.payload).unwrap().is_local());
        let theirs = encode(Origin::new(Origin::process().get() ^ 1), &change).unwrap();
        assert!(decode(&theirs.channel, &theirs.payload)
            .unwrap()
            .is_remote());
    }

    #[test]
    fn the_sink_counts_rather_than_fails() {
        let sent: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
        let sink = NotifySink::new(|channel: &str, payload: &str| {
            if channel == "bc_secrets" {
                return Err(NotifyError::new("no"));
            }
            sent.lock().unwrap().push((channel.into(), payload.into()));
            Ok(())
        })
        .only(["posts", "secrets"]);

        assert!(sink.wants("posts"));
        assert!(!sink.wants("sessions"));

        sink.publish(&RowChange::created("posts", 1i64));
        sink.publish(&RowChange::deleted("secrets", 1i64));
        assert_eq!(sink.sent(), 1);
        assert_eq!(sink.failures(), 1);
        assert_eq!(sent.lock().unwrap().len(), 1);

        let names: Vec<String> = (0..600).map(|n| format!("column_number_{n:04}")).collect();
        sink.publish(&RowChange::updated("posts", 2i64, names));
        assert_eq!(sink.truncated(), 1);
        assert_eq!(sink.oversized(), 0);
    }
}
