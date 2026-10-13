//! Two "processes" in one test binary, talking over the `LISTEN/NOTIFY` codec.
//!
//! PLAN.md §10: *"The same trait over `LISTEN/NOTIFY` buys multi-process
//! realtime — which PocketBase cannot have at any price, its broker being
//! in-memory beside an embedded database."* The channel here is a `Vec`
//! instead of a Postgres connection, which is the whole point of
//! [`Notifier`](brocanth_realtime::notify::Notifier) being a trait: the thing
//! under test is the encoding, the echo suppression and the audience, not the
//! driver.

#![cfg(feature = "listen-notify")]

mod common;

use std::sync::{Arc, Mutex};

use brocanth_events::{ChangeSink, RowChange};
use brocanth_policy::Pred;
use brocanth_realtime::notify::{channel_for, decode, NotifyError, NotifySink, Origin};
use brocanth_realtime::{Broker, Row, Subscription, Topic};

use common::PostColumn;

/// Everything on the wire, in order.
type Wire = Arc<Mutex<Vec<(String, String)>>>;

fn sink(wire: &Wire, origin: Origin) -> NotifySink<impl Fn(&str, &str) -> Result<(), NotifyError>> {
    let wire = Arc::clone(wire);
    NotifySink::new(move |channel: &str, payload: &str| {
        wire.lock().unwrap().push((channel.into(), payload.into()));
        Ok(())
    })
    .origin(origin)
    .only(["posts"])
}

#[test]
fn a_write_in_one_process_reaches_a_subscriber_in_another() {
    let wire: Wire = Arc::default();

    // Process A writes rows and holds no subscribers.
    let writer = sink(&wire, Origin::new(0xaaaa));

    // Process B holds the subscriber, and reads the row itself -- which is
    // what the pointer in a notification is for.
    let reader = Broker::builder()
        .rows(|_change: &RowChange<'_>, _columns: &[&str]| {
            Some(Row::new().with_column(PostColumn::AuthorId, 7i64))
        })
        .build();
    let client = reader.connect().unwrap();
    client
        .subscribe(Subscription::gated_by(
            Topic::collection("posts").unwrap(),
            Pred::eq(PostColumn::AuthorId, 7i64),
        ))
        .unwrap();

    // A collection nobody broadcasts never reaches the wire at all.
    assert!(!writer.wants("sessions"));
    assert!(writer.wants("posts"));

    let fields: &[&str] = &["title"];
    writer.publish(&RowChange::updated("posts", 42i64, fields));

    let sent = wire.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, channel_for("posts"));

    let inbound = decode(&sent[0].0, &sent[0].1).unwrap();
    assert!(inbound.is_remote(), "process B did not send this");
    assert!(!inbound.truncated);
    assert!(inbound.change.changed_field("title"));

    let delivery = reader.publish_change(&inbound.change);
    assert_eq!(delivery.keyed, 1);
    assert!(delivery.rows_read, "the pointer was resolved on arrival");
    assert_eq!(client.mailbox().len(), 1);
}

#[test]
fn a_process_does_not_deliver_its_own_echo_twice() {
    let wire: Wire = Arc::default();
    let local = sink(&wire, Origin::process());
    local.publish(&RowChange::created("posts", 1i64));

    let sent = wire.lock().unwrap().clone();
    let inbound = decode(&sent[0].0, &sent[0].1).unwrap();
    assert!(
        inbound.is_local(),
        "a listener skips this one, or every frame goes out twice"
    );
}

#[test]
fn an_oversized_change_still_reaches_the_right_audience() {
    // The field list is dropped to fit, so the receiver cannot say which
    // columns moved -- but the audience is computed from the row, not from the
    // field list, so delivery is unaffected. That is why a pointer is enough.
    let wire: Wire = Arc::default();
    let writer = sink(&wire, Origin::new(1));
    let names: Vec<String> = (0..600).map(|n| format!("column_number_{n:04}")).collect();
    writer.publish(&RowChange::updated("posts", 42i64, names));
    assert_eq!(writer.truncated(), 1);
    assert_eq!(writer.oversized(), 0);

    let reader = Broker::builder()
        .rows(|_change: &RowChange<'_>, _columns: &[&str]| {
            Some(Row::new().with_column(PostColumn::AuthorId, 7i64))
        })
        .build();
    let client = reader.connect().unwrap();
    client
        .subscribe(Subscription::gated_by(
            Topic::collection("posts").unwrap(),
            Pred::eq(PostColumn::AuthorId, 7i64),
        ))
        .unwrap();

    let sent = wire.lock().unwrap().clone();
    let inbound = decode(&sent[0].0, &sent[0].1).unwrap();
    assert!(inbound.truncated);
    assert!(inbound.change.changed_fields().is_empty());
    assert_eq!(reader.publish_change(&inbound.change).delivered, 1);
}
