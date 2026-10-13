//! The claims of PLAN.md §10, as assertions.
//!
//! Every count here comes from [`Delivery`], not from a clock, so none of it
//! is flaky and all of it fails loudly when the index stops being an index.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use brocanth_events::{ChangeSink, RowChange};
use brocanth_policy::{Ident, Op, Pred, Value};
use brocanth_realtime::{
    Broker, Config, Event, Gating, Group, Row, Subscription, Topic, UnknownGroups,
};

use common::{not_draft, PostColumn, PostPolicy, Visitor};

fn posts() -> Topic {
    Topic::collection("posts").unwrap()
}

fn row(author: i64, team: i64, status: &str) -> Row {
    Row::new()
        .with_column(PostColumn::AuthorId, author)
        .with_column(PostColumn::TeamId, team)
        .with_column(PostColumn::Status, status)
}

fn frames(client: &brocanth_realtime::Client) -> Vec<String> {
    client
        .mailbox()
        .drain()
        .into_iter()
        .filter_map(|event| match event {
            Event::Frame(frame) => Some(String::from_utf8(frame.bytes().to_vec()).unwrap()),
            Event::Lagged { .. } => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The audience
// ---------------------------------------------------------------------------

#[test]
fn a_write_reaches_the_matching_audience_and_nobody_else() {
    let broker = Broker::new();

    let author = broker.connect().unwrap();
    author
        .subscribe(Subscription::gated_by(
            posts(),
            Pred::eq(PostColumn::AuthorId, 7i64),
        ))
        .unwrap();

    let other_author = broker.connect().unwrap();
    other_author
        .subscribe(Subscription::gated_by(
            posts(),
            Pred::eq(PostColumn::AuthorId, 8i64),
        ))
        .unwrap();

    let team = broker.connect().unwrap();
    team.subscribe(Subscription::gated_by(
        posts(),
        Pred::eq(PostColumn::TeamId, 3i64),
    ))
    .unwrap();

    let everyone = broker.connect().unwrap();
    everyone
        .subscribe(Subscription::gated_by(posts(), Pred::<PostColumn>::Always))
        .unwrap();

    let nobody = broker.connect().unwrap();
    assert_eq!(
        nobody
            .subscribe(Subscription::gated_by(posts(), Pred::<PostColumn>::Never))
            .unwrap()
            .gating,
        Gating::Never
    );

    let delivery =
        broker.publish_with_row(&RowChange::created("posts", 42i64), &row(7, 9, "published"));

    assert_eq!(
        delivery.delivered, 2,
        "the author and the everyone audience"
    );
    assert_eq!(delivery.keyed, 1);
    assert_eq!(delivery.broadcast, 1);
    assert_eq!(delivery.scanned, 0, "every policy here inverted");
    assert_eq!(delivery.undecidable, 0);

    assert_eq!(frames(&author).len(), 1);
    assert_eq!(frames(&everyone).len(), 1);
    assert!(frames(&other_author).is_empty());
    assert!(frames(&team).is_empty(), "team 3, and the row is team 9");
    assert!(frames(&nobody).is_empty());
}

#[test]
fn a_union_keyed_on_two_columns_delivers_one_frame_not_two() {
    let broker = Broker::new();
    let client = broker.connect().unwrap();
    // "mine, or my team's": two keys, and a row that hits both.
    client
        .subscribe(Subscription::gated_by(
            posts(),
            Pred::any([
                Pred::eq(PostColumn::AuthorId, 7i64),
                Pred::eq(PostColumn::TeamId, 3i64),
            ]),
        ))
        .unwrap();

    let delivery =
        broker.publish_with_row(&RowChange::created("posts", 1i64), &row(7, 3, "published"));
    assert_eq!(delivery.delivered, 1);
    assert_eq!(delivery.examined, 1, "looked at once, not once per key");
    assert_eq!(frames(&client).len(), 1);
}

#[test]
fn a_conjunction_keys_on_one_column_and_confirms_the_other() {
    let broker = Broker::new();
    let client = broker.connect().unwrap();
    let filed = client
        .subscribe(Subscription::gated_by(
            posts(),
            Pred::all([
                Pred::eq(PostColumn::TeamId, 3i64),
                Pred::eq(PostColumn::Status, "published"),
            ]),
        ))
        .unwrap();
    assert_eq!(filed.gating, Gating::Prefiltered { keys: 1 });

    // The key hits, the confirmation fails: no frame, and no scan either.
    let miss = broker.publish_with_row(&RowChange::created("posts", 1i64), &row(1, 3, "draft"));
    assert_eq!(miss.delivered, 0);
    assert_eq!(miss.scanned, 0);

    let hit = broker.publish_with_row(&RowChange::created("posts", 2i64), &row(1, 3, "published"));
    assert_eq!(hit.delivered, 1);

    // The key misses: the subscriber is never even looked at.
    let elsewhere =
        broker.publish_with_row(&RowChange::created("posts", 3i64), &row(1, 4, "published"));
    assert_eq!(elsewhere.examined, 0);
}

#[test]
fn a_topic_is_gated_on_the_rule_that_matches_its_kind() {
    // PocketBase gates `collection/*` on the list rule and `collection/id` on
    // the view rule. The distinction is what stops a wildcard subscriber
    // seeing a row the list hides.
    let broker = Broker::new();
    let who = Visitor::user(7);

    let wildcard = broker.connect().unwrap();
    wildcard
        .subscribe(Subscription::from_policy(posts(), &PostPolicy, &who))
        .unwrap();

    let one_row = broker.connect().unwrap();
    one_row
        .subscribe(Subscription::from_policy(
            Topic::row("posts", 42i64).unwrap(),
            &PostPolicy,
            &who,
        ))
        .unwrap();

    // Somebody else's published post: the list rule admits it, the view rule
    // does not.
    let delivery = broker.publish_with_row(
        &RowChange::created("posts", 42i64),
        &row(99, 1, "published"),
    );
    assert_eq!(delivery.delivered, 1);
    let sent = frames(&wildcard);
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains("event:posts/*"), "{sent:?}");
    assert!(frames(&one_row).is_empty(), "the view rule is narrower");

    // The visitor's own draft: both rules admit it, and the frames carry
    // different event names so a client demultiplexes.
    broker.publish_with_row(&RowChange::created("posts", 42i64), &row(7, 1, "draft"));
    assert!(frames(&wildcard)[0].contains("event:posts/*"));
    assert!(frames(&one_row)[0].contains("event:posts/42"));
}

#[test]
fn a_row_topic_hears_only_about_its_own_row() {
    let broker = Broker::new();
    let client = broker.connect().unwrap();
    client
        .subscribe(Subscription::new(
            Topic::row("posts", 42i64).unwrap(),
            brocanth_realtime::Gate::everyone(),
        ))
        .unwrap();

    assert_eq!(
        broker
            .publish_change(&RowChange::deleted("posts", 43i64))
            .examined,
        0
    );
    assert_eq!(
        broker
            .publish_change(&RowChange::deleted("posts", 42i64))
            .delivered,
        1
    );
    // A text id and an integer id are different rows; `brocanth-events` says so.
    assert_eq!(
        broker
            .publish_change(&RowChange::deleted("posts", "42".to_owned()))
            .examined,
        0
    );
}

// ---------------------------------------------------------------------------
// The fallback
// ---------------------------------------------------------------------------

#[test]
fn a_policy_that_does_not_invert_falls_back_and_still_reaches_the_right_set() {
    let broker = Broker::new();

    let scanning = broker.connect().unwrap();
    let filed = scanning
        .subscribe(Subscription::gated_by(posts(), not_draft()))
        .unwrap();
    let Gating::Scanned { reason } = filed.gating else {
        panic!("a negation does not invert, and the caller is told so: {filed:?}")
    };
    assert!(reason.contains("complement"), "{reason}");

    let indexed = broker.connect().unwrap();
    indexed
        .subscribe(Subscription::gated_by(
            posts(),
            Pred::eq(PostColumn::AuthorId, 7i64),
        ))
        .unwrap();

    let published =
        broker.publish_with_row(&RowChange::created("posts", 1i64), &row(7, 1, "published"));
    assert_eq!(published.delivered, 2);
    assert_eq!(published.scanned, 1, "exactly the un-inverted subscriber");
    assert_eq!(published.keyed, 1);
    assert_eq!(frames(&scanning).len(), 1);
    assert_eq!(frames(&indexed).len(), 1);

    let draft = broker.publish_with_row(&RowChange::created("posts", 2i64), &row(7, 1, "draft"));
    assert_eq!(draft.scanned, 1, "it is examined whatever the answer");
    assert_eq!(draft.delivered, 1, "and correctly denied");
    assert!(frames(&scanning).is_empty());
    assert_eq!(frames(&indexed).len(), 1);
}

#[test]
fn the_slow_path_is_visible_without_running_a_publish() {
    let broker = Broker::new();
    let client = broker.connect().unwrap();
    client
        .subscribe(Subscription::gated_by(posts(), not_draft()))
        .unwrap();
    client
        .subscribe(Subscription::gated_by(
            Topic::collection("comments").unwrap(),
            not_draft(),
        ))
        .unwrap();
    client
        .subscribe(Subscription::gated_by(
            posts(),
            Pred::eq(PostColumn::AuthorId, 1i64),
        ))
        .unwrap();

    let audit = broker.audit();
    assert_eq!(audit.len(), 2, "{audit:?}");
    assert_eq!(audit[0].topic.to_string(), "comments/*");
    assert_eq!(audit[1].topic.to_string(), "posts/*");
    assert!(audit.iter().all(|entry| entry.subscribers == 1));
    assert!(audit[0].reason.contains("complement"));

    let stats = broker.stats();
    assert_eq!(stats.subscriptions, 3);
    assert_eq!(stats.scanned, 2);
    assert_eq!(stats.indexed, 1);
    assert_eq!(stats.topics, 2);
}

#[test]
fn a_predicate_about_another_table_is_denied_rather_than_queried() {
    let broker = Broker::new();
    let client = broker.connect().unwrap();
    // `NOT EXISTS` does not invert, and `eval` cannot answer it either.
    let membership = brocanth_policy::Membership::new(
        PostColumn::TeamId,
        Ident::must("team_members"),
        Ident::must("team_id"),
    )
    .filter_by(Ident::must("user_id"), Op::Eq, 7i64)
    .unwrap()
    .negate();
    client
        .subscribe(Subscription::gated_by(posts(), membership.into_pred()))
        .unwrap();

    let delivery =
        broker.publish_with_row(&RowChange::created("posts", 1i64), &row(1, 3, "published"));
    assert_eq!(delivery.scanned, 1);
    assert_eq!(delivery.undecidable, 1);
    assert_eq!(delivery.delivered, 0, "undecidable collapses to a denial");
}

#[test]
fn a_membership_audience_is_answered_by_a_group_index() {
    let membership = brocanth_policy::Membership::new(
        PostColumn::TeamId,
        Ident::must("team_members"),
        Ident::must("team_id"),
    )
    .filter_by(Ident::must("user_id"), Op::Eq, 7i64)
    .unwrap();

    let broker = Broker::builder()
        .groups(|group: &Group, member: &Value| {
            assert_eq!(group.table, "team_members");
            assert_eq!(group.filter, vec![("user_id", Value::Int(7))]);
            Some(*member == Value::Int(3))
        })
        .build();
    let client = broker.connect().unwrap();
    client
        .subscribe(Subscription::gated_by(posts(), membership.into_pred()))
        .unwrap();

    assert_eq!(
        broker
            .publish_with_row(&RowChange::created("posts", 1i64), &row(1, 3, "x"))
            .delivered,
        1
    );
    assert_eq!(
        broker
            .publish_with_row(&RowChange::created("posts", 2i64), &row(1, 4, "x"))
            .delivered,
        0
    );

    // And with no group index, the same subscription is undecidable rather
    // than silently admitted or silently denied.
    let blind = Broker::builder().groups(UnknownGroups).build();
    let client = blind.connect().unwrap();
    let membership = brocanth_policy::Membership::new(
        PostColumn::TeamId,
        Ident::must("team_members"),
        Ident::must("team_id"),
    )
    .filter_by(Ident::must("user_id"), Op::Eq, 7i64)
    .unwrap();
    client
        .subscribe(Subscription::gated_by(posts(), membership.into_pred()))
        .unwrap();
    let delivery = blind.publish_with_row(&RowChange::created("posts", 1i64), &row(1, 3, "x"));
    assert_eq!(delivery.undecidable, 1);
    assert_eq!(delivery.delivered, 0);
}

// ---------------------------------------------------------------------------
// The index is an index
// ---------------------------------------------------------------------------

#[test]
fn the_examined_count_does_not_grow_with_subscribers_elsewhere() {
    // §10's first correction, asserted on work rather than on a clock: the
    // same publish examines the same number of subscriptions whether the
    // broker holds a hundred others or ten thousand.
    fn examined(
        collections: usize,
        per_collection: usize,
    ) -> (usize, Vec<brocanth_realtime::Client>) {
        let broker = Broker::builder()
            .config(Config {
                max_clients: 100_000,
                ..Config::default()
            })
            .build();
        let mut held = Vec::new();
        for collection in 0..collections {
            let topic = Topic::collection(format!("table_{collection}")).unwrap();
            for subscriber in 0..per_collection {
                let client = broker.connect().unwrap();
                client
                    .subscribe(Subscription::gated_by(
                        topic.clone(),
                        Pred::eq(PostColumn::AuthorId, subscriber as i64),
                    ))
                    .unwrap();
                held.push(client);
            }
        }
        let delivery = broker.publish_with_row(
            &RowChange::created("table_0", 1i64),
            &Row::new().with_column(PostColumn::AuthorId, 3i64),
        );
        assert_eq!(delivery.delivered, 1);
        (delivery.examined, held)
    }

    let (small, _held) = examined(2, 50);
    let (large, _held) = examined(200, 50);
    assert_eq!(small, 1, "one bucket hit, one subscription looked at");
    assert_eq!(
        small, large,
        "a hundredfold more subscribers must cost the same publish"
    );
}

#[test]
fn a_collection_nobody_subscribed_to_costs_one_failed_lookup() {
    let broker = Broker::new();
    let client = broker.connect().unwrap();
    client
        .subscribe(Subscription::gated_by(posts(), Pred::<PostColumn>::Always))
        .unwrap();

    assert!(ChangeSink::wants(broker.as_ref(), "posts"));
    assert!(!ChangeSink::wants(broker.as_ref(), "sessions"));

    let delivery = broker.publish_change(&RowChange::created("sessions", 1i64));
    assert_eq!(delivery, Default::default());
    assert!(!delivery.rows_read);
}

#[test]
fn the_row_is_read_once_per_change_not_once_per_subscriber() {
    // The difference from PocketBase in one number. Fifty subscribers, one
    // read -- and PocketBase would run fifty `SELECT (1) …` here.
    let reads = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&reads);
    let broker = Broker::builder()
        .rows(move |_change: &RowChange<'_>, columns: &[&str]| {
            counter.fetch_add(1, Ordering::SeqCst);
            let mut row = Row::new();
            for column in columns {
                row.set(*column, 7i64);
            }
            Some(row)
        })
        .config(Config {
            max_clients: 100,
            ..Config::default()
        })
        .build();

    let mut held = Vec::new();
    for _ in 0..50 {
        let client = broker.connect().unwrap();
        client
            .subscribe(Subscription::gated_by(
                posts(),
                Pred::eq(PostColumn::AuthorId, 7i64),
            ))
            .unwrap();
        held.push(client);
    }

    let delivery = broker.publish_change(&RowChange::created("posts", 1i64));
    assert_eq!(delivery.delivered, 50);
    assert!(delivery.rows_read);
    assert_eq!(reads.load(Ordering::SeqCst), 1);
}

#[test]
fn an_everyone_audience_reads_no_row_at_all() {
    let reads = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&reads);
    let broker = Broker::builder()
        .rows(move |_change: &RowChange<'_>, _columns: &[&str]| {
            counter.fetch_add(1, Ordering::SeqCst);
            None
        })
        .build();
    let client = broker.connect().unwrap();
    client
        .subscribe(Subscription::gated_by(posts(), Pred::<PostColumn>::Always))
        .unwrap();

    let delivery = broker.publish_change(&RowChange::created("posts", 1i64));
    assert_eq!(delivery.delivered, 1);
    assert!(!delivery.rows_read, "no column, no query");
    assert_eq!(reads.load(Ordering::SeqCst), 0);
}

#[test]
fn a_row_that_cannot_be_read_denies_the_keyed_and_keeps_the_broadcast() {
    let broker = Broker::new(); // the default reader reads nothing
    let keyed = broker.connect().unwrap();
    keyed
        .subscribe(Subscription::gated_by(
            posts(),
            Pred::eq(PostColumn::AuthorId, 7i64),
        ))
        .unwrap();
    let everyone = broker.connect().unwrap();
    everyone
        .subscribe(Subscription::gated_by(posts(), Pred::<PostColumn>::Always))
        .unwrap();

    let delivery = broker.publish_change(&RowChange::deleted("posts", 1i64));
    assert!(delivery.row_unavailable);
    assert_eq!(delivery.delivered, 1);
    assert!(frames(&keyed).is_empty());
    assert_eq!(frames(&everyone).len(), 1);
}

#[test]
fn a_half_read_row_is_treated_as_unreadable_rather_than_as_nulls() {
    let broker = Broker::builder()
        .rows(|_change: &RowChange<'_>, _columns: &[&str]| {
            // Answers about `author_id` and forgets `team_id`.
            Some(Row::new().with("author_id", 7i64))
        })
        .build();
    let client = broker.connect().unwrap();
    client
        .subscribe(Subscription::gated_by(
            posts(),
            Pred::eq(PostColumn::AuthorId, 7i64),
        ))
        .unwrap();
    let other = broker.connect().unwrap();
    other
        .subscribe(Subscription::gated_by(
            posts(),
            Pred::eq(PostColumn::TeamId, 3i64),
        ))
        .unwrap();

    let delivery = broker.publish_change(&RowChange::created("posts", 1i64));
    assert!(delivery.row_unavailable);
    assert_eq!(delivery.delivered, 0, "deny, rather than guess at a NULL");
}

// ---------------------------------------------------------------------------
// The payload
// ---------------------------------------------------------------------------

#[test]
fn one_render_serves_a_whole_audience_class() {
    // §10: "the fragment emitter caches one render per audience class". Thirty
    // subscribers keyed on the same value, one call to the emitter.
    let renders = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&renders);
    let broker = Broker::builder()
        .emitter(move |emit: &brocanth_realtime::Emit<'_>| {
            counter.fetch_add(1, Ordering::SeqCst);
            Some(brocanth_realtime::Frame::sse(
                emit.id,
                emit.topic,
                "<li>a post</li>",
            ))
        })
        .build();

    let mut held = Vec::new();
    for _ in 0..30 {
        let client = broker.connect().unwrap();
        client
            .subscribe(Subscription::gated_by(
                posts(),
                Pred::eq(PostColumn::AuthorId, 7i64),
            ))
            .unwrap();
        held.push(client);
    }
    // And one in a different class.
    let everyone = broker.connect().unwrap();
    everyone
        .subscribe(Subscription::gated_by(posts(), Pred::<PostColumn>::Always))
        .unwrap();

    let delivery = broker.publish_with_row(&RowChange::created("posts", 1i64), &row(7, 1, "x"));
    assert_eq!(delivery.delivered, 31);
    assert_eq!(renders.load(Ordering::SeqCst), 2, "one per audience class");
    assert!(frames(&held[0])[0].contains("data:<li>a post</li>"));
}

#[test]
fn an_emitter_that_declines_sends_nothing_and_breaks_nothing() {
    let broker = Broker::builder()
        .emitter(|_emit: &brocanth_realtime::Emit<'_>| None)
        .build();
    let client = broker.connect().unwrap();
    client
        .subscribe(Subscription::gated_by(posts(), Pred::<PostColumn>::Always))
        .unwrap();
    let delivery = broker.publish_change(&RowChange::created("posts", 1i64));
    assert_eq!(delivery.examined, 1);
    assert_eq!(delivery.delivered, 0);
    assert!(client.mailbox().is_empty());
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

#[test]
fn unsubscribing_empties_the_index_rather_than_leaking_it() {
    let broker = Broker::new();
    let client = broker.connect().unwrap();
    let filed = client
        .subscribe(Subscription::gated_by(
            posts(),
            Pred::eq(PostColumn::AuthorId, 7i64),
        ))
        .unwrap();
    assert_eq!(broker.stats().topics, 1);

    assert!(client.unsubscribe(filed.id));
    assert!(!client.unsubscribe(filed.id));
    assert_eq!(broker.stats().topics, 0);
    assert_eq!(broker.stats().subscriptions, 0);
    assert!(!ChangeSink::wants(broker.as_ref(), "posts"));

    // And the whole set at once, which is PocketBase's empty-subscriptions POST.
    client
        .subscribe(Subscription::gated_by(posts(), Pred::<PostColumn>::Always))
        .unwrap();
    client
        .subscribe(Subscription::gated_by(
            Topic::row("posts", 1i64).unwrap(),
            Pred::<PostColumn>::Always,
        ))
        .unwrap();
    assert_eq!(client.subscriptions(), 2);
    client.unsubscribe_all();
    assert_eq!(client.subscriptions(), 0);
    assert_eq!(broker.stats().topics, 0);
}

#[test]
fn dropping_a_client_disconnects_it() {
    let broker = Broker::new();
    let client = broker.connect().unwrap();
    client
        .subscribe(Subscription::gated_by(posts(), Pred::<PostColumn>::Always))
        .unwrap();
    assert_eq!(broker.clients(), 1);
    let mailbox = Arc::clone(client.mailbox());

    drop(client);
    assert_eq!(broker.clients(), 0);
    assert_eq!(broker.stats().subscriptions, 0);
    assert!(mailbox.is_closed(), "a reader learns the stream ended");
    assert_eq!(
        broker
            .publish_change(&RowChange::created("posts", 1i64))
            .examined,
        0
    );
}

#[test]
fn replacing_the_subscription_set_is_the_whole_set() {
    let broker = Broker::new();
    let client = broker.connect().unwrap();
    client
        .subscribe(Subscription::gated_by(posts(), Pred::<PostColumn>::Always))
        .unwrap();
    let filed = client
        .replace_subscriptions([Subscription::gated_by(
            Topic::collection("comments").unwrap(),
            Pred::<PostColumn>::Always,
        )])
        .unwrap();
    assert_eq!(filed.len(), 1);
    assert_eq!(client.subscriptions(), 1);
    assert_eq!(
        broker
            .publish_change(&RowChange::created("posts", 1i64))
            .examined,
        0
    );
    assert_eq!(
        broker
            .publish_change(&RowChange::created("comments", 1i64))
            .delivered,
        1
    );
}
