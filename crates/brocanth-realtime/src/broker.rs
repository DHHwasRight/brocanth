//! The broker: §10's three corrections, in one type.
//!
//! 1. **Index by topic, not by client.** [`Registry`] is a map from collection
//!    to a wildcard index and a map of row indexes. A change to `posts/42`
//!    reaches two hash lookups and then only the subscribers on those two
//!    topics. PocketBase walks every connected client in chunks, and its own
//!    source carries a `@todo` about it.
//! 2. **Bound the per-client queue with an explicit overflow policy.** See
//!    [`crate::mailbox`].
//! 3. **Cap the client count.** [`Config::max_clients`], and
//!    [`ConnectError::AtCapacity`] at the limit.
//!
//! # The loop, as §10 asks for it
//!
//! ```text
//! a row changed, collection = "posts", row = 42
//!   -> the index for "posts/*", and the index for "posts/42"
//!   -> union the columns those indexes key on         ["author_id", "team_id"]
//!   -> read the row once, for those columns           (zero queries when the
//!                                                      publisher hands it over)
//!   -> for each column, one map lookup on its value   -> the subscribers
//!   -> render one frame per audience class            -> push, never block
//! ```
//!
//! Six hundred subscribers, one row read, two hash lookups, one render per
//! class. PocketBase: six hundred point queries.
//!
//! # The locking, stated
//!
//! The registry is an `RwLock`. A publish takes it for **reading**, so
//! publishes do not serialise against each other; subscribing and
//! disconnecting take it for writing. Per-subscription delivery bookkeeping is
//! an `AtomicU64`, which is why the fast path needs no write lock.
//!
//! Two things run under the read lock and both are the caller's code: the
//! [`RowReader`] and the [`FrameEmitter`]. Neither may block for long, and
//! both say so. The way to avoid the reader entirely is
//! [`Broker::publish_with_row`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use brocanth_events::{ChangeSink, RowChange, RowId};

use crate::frame::{AudienceClass, ChangeLine, Emit, Frame, FrameEmitter};
use crate::gate::{Gate, Gating, Subscription};
use crate::mailbox::{Mailbox, Overflow, Pushed};
use crate::reach::{GroupIndex, Plan, UnknownGroups, ValueKey, Verdict};
use crate::row::{NoRows, Row, RowReader};
use crate::topic::Topic;

// ---------------------------------------------------------------------------
// Identifiers
// ---------------------------------------------------------------------------

/// One connection. PocketBase's `clientId`, which it hands to the client in
/// its `PB_CONNECT` frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClientId(u64);

impl std::fmt::Display for ClientId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One topic a client subscribed to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SubscriptionId(u64);

impl std::fmt::Display for SubscriptionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// The limits, all of them explicit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// How many connections the broker will hold. §10's third correction:
    /// *"cap the client count, which PocketBase does not do at all."*
    ///
    /// At the limit [`Broker::connect`] returns
    /// [`ConnectError::AtCapacity`] and the new connection is **refused**.
    /// Refused rather than traded for an existing one: evicting an established
    /// subscriber to admit a new one lets anyone holding a socket knock
    /// everybody else off, which turns a capacity limit into a denial of
    /// service with extra steps. A transport should answer `503` with a
    /// `Retry-After`.
    pub max_clients: usize,

    /// How many topics one client may hold. PocketBase has no limit here
    /// either, and a client that posts a million topics costs the server
    /// memory it never agreed to spend.
    pub max_subscriptions_per_client: usize,

    /// How many frames a client's mailbox holds before
    /// [`Config::overflow`] fires.
    pub queue: usize,

    /// What happens when a mailbox is full. See [`crate::mailbox`] for the
    /// argument behind the default.
    pub overflow: Overflow,
}

impl Config {
    /// 1000 connections — §10's own benchmark number, *"a thousand connections
    /// with six hundred subscribers"*. It is a starting point, not a law:
    /// raise it once you have measured the memory, which is
    /// `max_clients × queue × the size of a frame`.
    pub const DEFAULT_MAX_CLIENTS: usize = 1000;

    /// 64 topics per client. A browser page subscribing to more than this is
    /// almost certainly subscribing to a collection it should have used the
    /// wildcard for.
    pub const DEFAULT_MAX_SUBSCRIPTIONS: usize = 64;

    /// 32 frames. See [`crate::mailbox`] for the arithmetic.
    pub const DEFAULT_QUEUE: usize = 32;
}

impl Default for Config {
    fn default() -> Config {
        Config {
            max_clients: Config::DEFAULT_MAX_CLIENTS,
            max_subscriptions_per_client: Config::DEFAULT_MAX_SUBSCRIPTIONS,
            queue: Config::DEFAULT_QUEUE,
            overflow: Overflow::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Why a connection was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ConnectError {
    /// The client cap is full. Refuse the connection; do not evict anybody.
    #[error("the broker holds {connected} of {cap} connections")]
    AtCapacity {
        /// How many are connected.
        connected: usize,
        /// The cap.
        cap: usize,
    },
}

/// Why a subscription was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SubscribeError {
    /// This client already holds its maximum number of topics.
    #[error("this client holds {held} of {cap} subscriptions")]
    TooManySubscriptions {
        /// How many it holds.
        held: usize,
        /// The cap.
        cap: usize,
    },
    /// The client is gone — it disconnected, or its mailbox overflowed under
    /// [`Overflow::Disconnect`].
    #[error("this client is no longer connected")]
    Disconnected,
}

// ---------------------------------------------------------------------------
// What one publish did
// ---------------------------------------------------------------------------

/// What one published change cost and where it went.
///
/// Returned by every publish, so §10's cost model is a number a test can
/// assert on rather than a claim in a comment. The one to watch is
/// [`Delivery::scanned`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Delivery {
    /// Subscriptions the broker looked at **at all**.
    ///
    /// The proof that the index is an index. With ten thousand subscriptions
    /// spread over a hundred collections, a change to one collection examines
    /// the subscriptions on that collection's topics and nothing else, and
    /// this number says so without a clock.
    pub examined: usize,
    /// Frames handed to a mailbox.
    pub delivered: usize,
    /// Of those, reached by a key lookup. The fast path.
    pub keyed: usize,
    /// Of those, reached because the audience is everyone. The free path.
    pub broadcast: usize,
    /// Subscriptions checked against the row one at a time, because their
    /// policy did not invert. **The cost §10 is about.** In memory, against a
    /// row read once — not a query each, as PocketBase does — but still linear
    /// in the subscribers on the topic.
    pub scanned: usize,
    /// Subscriptions nobody could decide: a membership with no group index, or
    /// an opaque fragment with no evaluator. Denied.
    pub undecidable: usize,
    /// Frames destroyed by an overflow policy.
    pub dropped: usize,
    /// Clients removed because their mailbox closed, under
    /// [`Overflow::Disconnect`] or because they had already gone.
    pub disconnected: usize,
    /// Whether a row was read for this change. At most one, ever.
    pub rows_read: bool,
    /// The row was needed and could not be read, so every subscription that
    /// needed a column was denied. See [`crate::row`].
    pub row_unavailable: bool,
}

/// A topic with subscribers on the slow path.
///
/// What [`Broker::audit`] returns, so an accidentally opaque policy is a thing
/// somebody can notice and fix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fallback {
    /// Where.
    pub topic: Topic,
    /// Why there is no key, verbatim from `brocanth_policy::NotInvertible` or
    /// from this crate's own indexing limits.
    pub reason: &'static str,
    /// How many subscriptions are paying for it. This many in-memory
    /// evaluations per change of this topic.
    pub subscribers: usize,
}

/// A snapshot of the broker.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Connections held.
    pub clients: usize,
    /// Subscriptions held.
    pub subscriptions: usize,
    /// Distinct topics with at least one subscription.
    pub topics: usize,
    /// Subscriptions on a key, or on everyone. The fast path.
    pub indexed: usize,
    /// Subscriptions checked per change. The slow path.
    pub scanned: usize,
    /// Subscriptions whose policy denied the subject outright. They receive
    /// nothing, and knowing that is often the bug.
    pub never: usize,
}

// ---------------------------------------------------------------------------
// The registry
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct ClientRec {
    mailbox: Arc<Mailbox>,
    subscriptions: Vec<(SubscriptionId, Topic)>,
}

#[derive(Debug)]
enum Placement {
    Everyone,
    Never,
    Keys(Vec<(&'static str, ValueKey)>),
    Unindexed,
}

#[derive(Debug)]
struct SubRec {
    client: ClientId,
    gate: Gate,
    placement: Placement,
    confirm: bool,
    columns: Vec<&'static str>,
    /// The publish this subscription was last delivered a frame for. Dedup for
    /// a union keyed on two columns that both hit, without allocating a set
    /// per publish.
    stamp: AtomicU64,
}

#[derive(Debug)]
struct TopicIndex {
    topic: Topic,
    subs: HashMap<SubscriptionId, SubRec>,
    everyone: Vec<SubscriptionId>,
    by_column: HashMap<&'static str, HashMap<ValueKey, Vec<SubscriptionId>>>,
    unindexed: Vec<SubscriptionId>,
    /// Reference counts, so a column stops being read the moment the last
    /// subscription that needed it goes away.
    columns: HashMap<&'static str, usize>,
}

impl TopicIndex {
    fn new(topic: Topic) -> TopicIndex {
        TopicIndex {
            topic,
            subs: HashMap::new(),
            everyone: Vec::new(),
            by_column: HashMap::new(),
            unindexed: Vec::new(),
            columns: HashMap::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.subs.is_empty()
    }

    fn insert(&mut self, id: SubscriptionId, client: ClientId, gate: Gate) -> Gating {
        let plan = gate.plan();
        let gating = Gating::of(&plan);
        let columns = gate.columns();
        for column in &columns {
            *self.columns.entry(column).or_insert(0) += 1;
        }
        let (placement, confirm) = match plan {
            Plan::Everyone => {
                self.everyone.push(id);
                (Placement::Everyone, false)
            }
            Plan::Never => (Placement::Never, false),
            Plan::Keys { keys, confirm } => {
                for (column, key) in &keys {
                    self.by_column
                        .entry(column)
                        .or_default()
                        .entry(key.clone())
                        .or_default()
                        .push(id);
                }
                (Placement::Keys(keys), confirm)
            }
            Plan::Unindexed { .. } => {
                self.unindexed.push(id);
                (Placement::Unindexed, false)
            }
        };
        self.subs.insert(
            id,
            SubRec {
                client,
                gate,
                placement,
                confirm,
                columns,
                stamp: AtomicU64::new(0),
            },
        );
        gating
    }

    fn remove(&mut self, id: SubscriptionId) {
        let Some(rec) = self.subs.remove(&id) else {
            return;
        };
        for column in &rec.columns {
            if let Some(count) = self.columns.get_mut(column) {
                *count -= 1;
                if *count == 0 {
                    self.columns.remove(column);
                }
            }
        }
        match rec.placement {
            Placement::Everyone => self.everyone.retain(|held| *held != id),
            Placement::Never => {}
            Placement::Unindexed => self.unindexed.retain(|held| *held != id),
            Placement::Keys(keys) => {
                for (column, key) in keys {
                    let Some(buckets) = self.by_column.get_mut(column) else {
                        continue;
                    };
                    if let Some(bucket) = buckets.get_mut(&key) {
                        bucket.retain(|held| *held != id);
                        if bucket.is_empty() {
                            buckets.remove(&key);
                        }
                    }
                    if buckets.is_empty() {
                        self.by_column.remove(column);
                    }
                }
            }
        }
    }
}

#[derive(Debug, Default)]
struct CollectionSlot {
    wildcard: Option<TopicIndex>,
    rows: HashMap<RowId<'static>, TopicIndex>,
}

impl CollectionSlot {
    fn is_empty(&self) -> bool {
        self.wildcard.as_ref().is_none_or(TopicIndex::is_empty) && self.rows.is_empty()
    }
}

#[derive(Debug, Default)]
struct Registry {
    clients: HashMap<ClientId, ClientRec>,
    collections: HashMap<String, CollectionSlot>,
    next_client: u64,
    next_subscription: u64,
}

impl Registry {
    fn index_mut(&mut self, topic: &Topic) -> &mut TopicIndex {
        let slot = self
            .collections
            .entry(topic.collection_name().to_owned())
            .or_default();
        match topic {
            Topic::Collection(_) => slot
                .wildcard
                .get_or_insert_with(|| TopicIndex::new(topic.clone())),
            Topic::Row { id, .. } => slot
                .rows
                .entry(id.clone())
                .or_insert_with(|| TopicIndex::new(topic.clone())),
        }
    }

    fn drop_subscription(&mut self, topic: &Topic, id: SubscriptionId) {
        let Some(slot) = self.collections.get_mut(topic.collection_name()) else {
            return;
        };
        match topic {
            Topic::Collection(_) => {
                if let Some(index) = slot.wildcard.as_mut() {
                    index.remove(id);
                    if index.is_empty() {
                        slot.wildcard = None;
                    }
                }
            }
            Topic::Row { id: row, .. } => {
                if let Some(index) = slot.rows.get_mut(row) {
                    index.remove(id);
                    if index.is_empty() {
                        slot.rows.remove(row);
                    }
                }
            }
        }
        if slot.is_empty() {
            self.collections.remove(topic.collection_name());
        }
    }

    fn remove_client(&mut self, id: ClientId) -> bool {
        let Some(client) = self.clients.remove(&id) else {
            return false;
        };
        client.mailbox.close();
        for (subscription, topic) in client.subscriptions {
            self.drop_subscription(&topic, subscription);
        }
        true
    }
}

// ---------------------------------------------------------------------------
// The broker
// ---------------------------------------------------------------------------

/// The topic index, the audience index and the fan-out.
///
/// Built with [`Broker::builder`], held in an `Arc`, shared by every thread
/// that writes a row and every connection that reads one.
pub struct Broker {
    config: Config,
    registry: RwLock<Registry>,
    rows: Box<dyn RowReader>,
    groups: Box<dyn GroupIndex>,
    emitter: Box<dyn FrameEmitter>,
    seq: AtomicU64,
}

impl std::fmt::Debug for Broker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let stats = self.stats();
        f.debug_struct("Broker")
            .field("config", &self.config)
            .field("stats", &stats)
            .finish_non_exhaustive()
    }
}

/// Assembles a [`Broker`].
pub struct BrokerBuilder {
    config: Config,
    rows: Box<dyn RowReader>,
    groups: Box<dyn GroupIndex>,
    emitter: Box<dyn FrameEmitter>,
}

impl Default for BrokerBuilder {
    fn default() -> BrokerBuilder {
        BrokerBuilder {
            config: Config::default(),
            rows: Box::new(NoRows),
            groups: Box::new(UnknownGroups),
            emitter: Box::new(ChangeLine),
        }
    }
}

impl BrokerBuilder {
    /// The limits.
    #[must_use]
    pub fn config(mut self, config: Config) -> BrokerBuilder {
        self.config = config;
        self
    }

    /// Where the columns a change does not carry come from. The default reads
    /// nothing, which is right for a publisher that calls
    /// [`Broker::publish_with_row`].
    #[must_use]
    pub fn rows(mut self, rows: impl RowReader + 'static) -> BrokerBuilder {
        self.rows = Box::new(rows);
        self
    }

    /// Who can answer a membership audience. The default answers nothing, and
    /// a membership audience is then undecidable and denied.
    #[must_use]
    pub fn groups(mut self, groups: impl GroupIndex + 'static) -> BrokerBuilder {
        self.groups = Box::new(groups);
        self
    }

    /// What a change looks like on the wire. The default is
    /// [`ChangeLine`]; a fragment emitter goes here.
    #[must_use]
    pub fn emitter(mut self, emitter: impl FrameEmitter + 'static) -> BrokerBuilder {
        self.emitter = Box::new(emitter);
        self
    }

    /// Build it.
    pub fn build(self) -> Arc<Broker> {
        Arc::new(Broker {
            config: self.config,
            registry: RwLock::new(Registry::default()),
            rows: self.rows,
            groups: self.groups,
            emitter: self.emitter,
            // Starts at one so that a subscription's zero stamp cannot look
            // like "already delivered in publish zero".
            seq: AtomicU64::new(1),
        })
    }
}

impl Broker {
    /// A broker with the default configuration, no row reader, no group index
    /// and the [`ChangeLine`] emitter.
    pub fn new() -> Arc<Broker> {
        Broker::builder().build()
    }

    /// Configure one.
    pub fn builder() -> BrokerBuilder {
        BrokerBuilder::default()
    }

    /// The limits in force.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Open a connection.
    ///
    /// The returned [`Client`] owns its registration: dropping it unsubscribes
    /// everything and closes the mailbox, so a transport that loses its socket
    /// cleans up by letting the handle go.
    pub fn connect(self: &Arc<Self>) -> Result<Client, ConnectError> {
        let mut registry = self.write();
        let connected = registry.clients.len();
        if connected >= self.config.max_clients {
            return Err(ConnectError::AtCapacity {
                connected,
                cap: self.config.max_clients,
            });
        }
        registry.next_client += 1;
        let id = ClientId(registry.next_client);
        let mailbox = Arc::new(Mailbox::new(self.config.queue, self.config.overflow));
        registry.clients.insert(
            id,
            ClientRec {
                mailbox: Arc::clone(&mailbox),
                subscriptions: Vec::new(),
            },
        );
        drop(registry);
        Ok(Client {
            id,
            broker: Arc::clone(self),
            mailbox,
        })
    }

    /// How many connections are open.
    pub fn clients(&self) -> usize {
        self.read().clients.len()
    }

    /// A snapshot.
    pub fn stats(&self) -> Stats {
        let registry = self.read();
        let mut stats = Stats {
            clients: registry.clients.len(),
            ..Stats::default()
        };
        for slot in registry.collections.values() {
            for index in slot.wildcard.iter().chain(slot.rows.values()) {
                stats.topics += 1;
                stats.subscriptions += index.subs.len();
                stats.scanned += index.unindexed.len();
                for rec in index.subs.values() {
                    match rec.placement {
                        Placement::Never => stats.never += 1,
                        Placement::Everyone | Placement::Keys(_) => stats.indexed += 1,
                        Placement::Unindexed => {}
                    }
                }
            }
        }
        stats
    }

    /// Every topic with a subscriber on the slow path, and why.
    ///
    /// The §10 health check. An application whose audit is empty is paying a
    /// map lookup per write; one whose audit is not is paying an evaluation
    /// per subscriber, and the `reason` says what to change in the policy to
    /// stop.
    pub fn audit(&self) -> Vec<Fallback> {
        let registry = self.read();
        let mut out = Vec::new();
        for slot in registry.collections.values() {
            for index in slot.wildcard.iter().chain(slot.rows.values()) {
                let mut by_reason: HashMap<&'static str, usize> = HashMap::new();
                for id in &index.unindexed {
                    if let Some(rec) = index.subs.get(id) {
                        if let Plan::Unindexed { reason } = rec.gate.plan() {
                            *by_reason.entry(reason).or_insert(0) += 1;
                        }
                    }
                }
                for (reason, subscribers) in by_reason {
                    out.push(Fallback {
                        topic: index.topic.clone(),
                        reason,
                        subscribers,
                    });
                }
            }
        }
        out.sort_by(|a, b| {
            a.topic
                .to_string()
                .cmp(&b.topic.to_string())
                .then(a.reason.cmp(b.reason))
        });
        out
    }

    /// Broadcast a change, reading the row through the configured
    /// [`RowReader`] if any subscriber needs a column.
    pub fn publish_change(&self, change: &RowChange<'_>) -> Delivery {
        self.broadcast(change, None)
    }

    /// Broadcast a change whose row the publisher already holds.
    ///
    /// **The zero-query path, and the one the model layer should take.** The
    /// row was just written, so nothing has to go and find it; the broker uses
    /// what it is given and the [`RowReader`] never runs. A row carrying more
    /// columns than any audience keys on costs nothing — the extra columns are
    /// simply not looked at.
    pub fn publish_with_row(&self, change: &RowChange<'_>, row: &Row) -> Delivery {
        self.broadcast(change, Some(row))
    }

    fn broadcast(&self, change: &RowChange<'_>, given: Option<&Row>) -> Delivery {
        let registry = self.read();
        let Some(slot) = registry.collections.get(change.collection.as_ref()) else {
            // The whole of the "index by topic" claim: a collection nobody
            // subscribed to is one failed hash lookup, whatever the broker
            // holds elsewhere.
            return Delivery::default();
        };

        let indexes: Vec<&TopicIndex> = slot
            .wildcard
            .iter()
            .chain(slot.rows.get(&change.row))
            .collect();
        if indexes.is_empty() {
            return Delivery::default();
        }

        let mut needed: Vec<&str> = Vec::new();
        let mut wants_row = false;
        for index in &indexes {
            wants_row |= !index.unindexed.is_empty();
            for column in index.columns.keys() {
                if !needed.contains(column) {
                    needed.push(column);
                }
            }
        }
        wants_row |= !needed.is_empty();

        let mut delivery = Delivery::default();
        let read;
        let row = match given {
            Some(row) => Some(row),
            None if wants_row => {
                delivery.rows_read = true;
                read = self.rows.read(change, &needed);
                read.as_ref()
            }
            None => None,
        };
        // A reader that answered with half the columns would otherwise look
        // like a row whose other half is NULL, and silently stop delivering.
        let row = row.filter(|row| row.has_all(&needed));
        delivery.row_unavailable = wants_row && row.is_none();

        let publish = self.seq.fetch_add(1, Ordering::Relaxed);
        let mut walk = Walk {
            emitter: self.emitter.as_ref(),
            groups: self.groups.as_ref(),
            change,
            row,
            id: publish,
            delivery,
            closed: Vec::new(),
        };

        for index in indexes {
            walk.topic_index(&registry, index, publish);
        }

        let Walk {
            mut delivery,
            closed,
            ..
        } = walk;
        drop(registry);

        if !closed.is_empty() {
            let mut registry = self.write();
            for client in closed {
                if registry.remove_client(client) {
                    delivery.disconnected += 1;
                }
            }
        }
        delivery
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Registry> {
        self.registry.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Registry> {
        self.registry.write().unwrap_or_else(|e| e.into_inner())
    }
}

/// Reports changes into the broker.
///
/// What makes the model layer's `Box<dyn ChangeSink>` a realtime broadcast
/// without the model layer knowing realtime exists.
///
/// [`ChangeSink::wants`] is one hash lookup against the topic index, so a
/// collection nobody subscribed to costs the publisher nothing — it never
/// builds the change at all. That is §8's "a hook nobody registered costs
/// nothing", at the one seam where it is cheap to honour.
///
/// A publisher that holds the row it just wrote should call
/// [`Broker::publish_with_row`] instead: the sink trait carries identity, not
/// values, so going through it gives up the zero-query path.
impl ChangeSink for Broker {
    fn publish(&self, change: &RowChange<'_>) {
        let _ = self.publish_change(change);
    }

    fn wants(&self, collection: &str) -> bool {
        self.read().collections.contains_key(collection)
    }
}

// ---------------------------------------------------------------------------
// One publish
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, Eq, Hash)]
enum ClassKey {
    Everyone,
    Keyed(&'static str, ValueKey),
    Unindexed,
}

struct Walk<'a> {
    emitter: &'a dyn FrameEmitter,
    groups: &'a dyn GroupIndex,
    change: &'a RowChange<'a>,
    row: Option<&'a Row>,
    id: u64,
    delivery: Delivery,
    closed: Vec<ClientId>,
}

impl<'a> Walk<'a> {
    fn topic_index(&mut self, registry: &Registry, index: &'a TopicIndex, publish: u64) {
        // One render per audience class, per §10, reused by every subscriber in
        // the class. Reset per topic, because the topic is the SSE event name
        // and therefore part of the bytes.
        let mut cache: Vec<(ClassKey, Option<Frame>)> = Vec::new();

        for id in &index.everyone {
            self.delivery.examined += 1;
            if self.send(
                registry,
                index,
                *id,
                AudienceClass::Everyone,
                ClassKey::Everyone,
                &mut cache,
            ) {
                self.delivery.broadcast += 1;
            }
        }

        if let Some(row) = self.row {
            for (column, buckets) in &index.by_column {
                let Some(value) = row.get(column) else {
                    continue;
                };
                let Some(key) = ValueKey::of(value) else {
                    continue;
                };
                let Some(bucket) = buckets.get(&key) else {
                    continue;
                };
                for id in bucket {
                    let Some(rec) = index.subs.get(id) else {
                        continue;
                    };
                    // A union keyed on two columns can hit twice. One frame.
                    if rec.stamp.swap(publish, Ordering::Relaxed) == publish {
                        continue;
                    }
                    self.delivery.examined += 1;
                    if rec.confirm {
                        match rec.gate.admits(row, self.groups) {
                            Verdict::Admit => {}
                            Verdict::Deny => continue,
                            Verdict::Undecidable { .. } => {
                                self.delivery.undecidable += 1;
                                continue;
                            }
                        }
                    }
                    if self.send(
                        registry,
                        index,
                        *id,
                        AudienceClass::Keyed { column, value },
                        ClassKey::Keyed(column, key.clone()),
                        &mut cache,
                    ) {
                        self.delivery.keyed += 1;
                    }
                }
            }
        }

        for id in &index.unindexed {
            self.delivery.examined += 1;
            self.delivery.scanned += 1;
            let Some(rec) = index.subs.get(id) else {
                continue;
            };
            let Some(row) = self.row else {
                self.delivery.undecidable += 1;
                continue;
            };
            match rec.gate.admits(row, self.groups) {
                Verdict::Admit => {}
                Verdict::Deny => continue,
                Verdict::Undecidable { .. } => {
                    self.delivery.undecidable += 1;
                    continue;
                }
            }
            self.send(
                registry,
                index,
                *id,
                AudienceClass::Unindexed,
                ClassKey::Unindexed,
                &mut cache,
            );
        }
    }

    fn send(
        &mut self,
        registry: &Registry,
        index: &TopicIndex,
        id: SubscriptionId,
        class: AudienceClass<'_>,
        key: ClassKey,
        cache: &mut Vec<(ClassKey, Option<Frame>)>,
    ) -> bool {
        let Some(rec) = index.subs.get(&id) else {
            return false;
        };
        let Some(client) = registry.clients.get(&rec.client) else {
            return false;
        };
        let frame = match cache.iter().find(|(held, _)| *held == key) {
            Some((_, frame)) => frame.clone(),
            None => {
                let frame = self.emitter.emit(&Emit {
                    topic: &index.topic,
                    change: self.change,
                    row: self.row,
                    class,
                    id: self.id,
                });
                cache.push((key, frame.clone()));
                frame
            }
        };
        let Some(frame) = frame else {
            // The emitter declined for this whole class.
            return false;
        };
        match client.mailbox.push(frame) {
            Pushed::Queued => {
                self.delivery.delivered += 1;
                true
            }
            Pushed::Dropped => {
                self.delivery.delivered += 1;
                self.delivery.dropped += 1;
                true
            }
            Pushed::Closed => {
                if !self.closed.contains(&rec.client) {
                    self.closed.push(rec.client);
                }
                false
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The client handle
// ---------------------------------------------------------------------------

/// What a transport holds for one connection.
///
/// Dropping it disconnects: every subscription leaves the index and the
/// mailbox closes. That is the only cleanup path there is, so a socket that
/// dies cleans up by the handle going out of scope.
#[derive(Debug)]
pub struct Client {
    id: ClientId,
    broker: Arc<Broker>,
    mailbox: Arc<Mailbox>,
}

/// What [`Client::subscribe`] filed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Subscribed {
    /// The handle for [`Client::unsubscribe`].
    pub id: SubscriptionId,
    /// How it was filed, and therefore what it costs per change.
    pub gating: Gating,
}

impl Client {
    /// This connection's id. PocketBase hands this to the browser in its
    /// `PB_CONNECT` frame, and a transport that mirrors that behaviour writes
    /// this value.
    pub fn id(&self) -> ClientId {
        self.id
    }

    /// The queue of frames waiting for this connection.
    pub fn mailbox(&self) -> &Arc<Mailbox> {
        &self.mailbox
    }

    /// Add a topic.
    ///
    /// The return value says whether the policy inverted. A
    /// [`Gating::Scanned`] here is the thing §10 is about, and the reason it
    /// carries says what to change.
    pub fn subscribe(&self, subscription: Subscription) -> Result<Subscribed, SubscribeError> {
        let mut registry = self.broker.write();
        let Some(client) = registry.clients.get(&self.id) else {
            return Err(SubscribeError::Disconnected);
        };
        let held = client.subscriptions.len();
        if held >= self.broker.config.max_subscriptions_per_client {
            return Err(SubscribeError::TooManySubscriptions {
                held,
                cap: self.broker.config.max_subscriptions_per_client,
            });
        }
        registry.next_subscription += 1;
        let id = SubscriptionId(registry.next_subscription);
        let (topic, gate) = subscription.into_parts();
        let gating = registry.index_mut(&topic).insert(id, self.id, gate);
        if let Some(client) = registry.clients.get_mut(&self.id) {
            client.subscriptions.push((id, topic));
        }
        Ok(Subscribed { id, gating })
    }

    /// Drop one topic. `true` when it was held.
    pub fn unsubscribe(&self, id: SubscriptionId) -> bool {
        let mut registry = self.broker.write();
        let Some(client) = registry.clients.get_mut(&self.id) else {
            return false;
        };
        let Some(at) = client
            .subscriptions
            .iter()
            .position(|(held, _)| *held == id)
        else {
            return false;
        };
        let (_, topic) = client.subscriptions.remove(at);
        registry.drop_subscription(&topic, id);
        true
    }

    /// Drop every topic, keeping the connection.
    ///
    /// PocketBase's "an empty subscription set means unsubscribe from
    /// everything", which is a real case: a page that navigates away from the
    /// view that wanted the data but keeps its stream open.
    pub fn unsubscribe_all(&self) {
        let mut registry = self.broker.write();
        let Some(client) = registry.clients.get_mut(&self.id) else {
            return;
        };
        for (id, topic) in std::mem::take(&mut client.subscriptions) {
            registry.drop_subscription(&topic, id);
        }
    }

    /// Replace the whole set, which is what PocketBase's `POST /api/realtime`
    /// does: the request carries the complete new set, not a delta.
    ///
    /// Not atomic against a concurrent publish — a change landing between the
    /// clear and the re-add reaches neither set. That is the honest trade for
    /// not holding the write lock across the caller's iterator, and a client
    /// that cannot miss a frame across a resubscribe should subscribe to the
    /// new topics before dropping the old ones.
    pub fn replace_subscriptions(
        &self,
        subscriptions: impl IntoIterator<Item = Subscription>,
    ) -> Result<Vec<Subscribed>, SubscribeError> {
        self.unsubscribe_all();
        subscriptions
            .into_iter()
            .map(|subscription| self.subscribe(subscription))
            .collect()
    }

    /// How many topics this client holds.
    pub fn subscriptions(&self) -> usize {
        self.broker
            .read()
            .clients
            .get(&self.id)
            .map_or(0, |client| client.subscriptions.len())
    }

    /// Close the connection now, rather than at the end of the scope.
    pub fn disconnect(self) {
        // `Drop` does the work; this is the spelling for a caller who wants
        // the intent in the diff.
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.broker.write().remove_client(self.id);
    }
}
