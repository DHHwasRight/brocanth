//! §10's second and third corrections: the bounded queue and the client cap.
//!
//! The headline claim is that **a client that falls behind can never slow the
//! broadcaster**, and it is proved by ordering rather than by a clock. The
//! reader is held at a gate that only opens after the broadcast has finished,
//! so if the broadcaster needed the reader to make progress the test would
//! deadlock instead of occasionally passing. `brocanth-server`'s
//! `fifty_waiting_handlers_do_not_cost_fifty_threads` measures the same kind
//! of claim; this one measures work done rather than elapsed time, because a
//! count cannot be flaky on a loaded machine.

mod common;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Duration;

use brocanth_events::RowChange;
use brocanth_policy::Pred;
use brocanth_realtime::{Broker, Config, ConnectError, Event, Overflow, SubscribeError};
use brocanth_realtime::{Subscription, Topic};

use common::PostColumn;

const CHANGES: usize = 500;

fn posts() -> Topic {
    Topic::collection("posts").unwrap()
}

fn everyone() -> Subscription {
    Subscription::gated_by(posts(), Pred::<PostColumn>::Always)
}

fn broker_with(queue: usize, overflow: Overflow) -> Arc<Broker> {
    Broker::builder()
        .config(Config {
            queue,
            overflow,
            ..Config::default()
        })
        .build()
}

// ---------------------------------------------------------------------------
// The property
// ---------------------------------------------------------------------------

#[test]
fn a_stalled_subscriber_does_not_stall_the_broadcaster() {
    let broker = broker_with(4, Overflow::DropOldest);
    let client = broker.connect().unwrap();
    client.subscribe(everyone()).unwrap();

    // The reader reaches the gate, announces it, and waits. Nothing it does
    // from here can release the broadcaster, because the only thing that
    // releases *it* is the main thread, after the broadcast.
    let gate = Arc::new(Barrier::new(2));
    let parked = Arc::new(AtomicBool::new(false));
    let consumed = Arc::new(AtomicUsize::new(0));

    let mailbox = Arc::clone(client.mailbox());
    let reader_gate = Arc::clone(&gate);
    let reader_parked = Arc::clone(&parked);
    let reader_consumed = Arc::clone(&consumed);
    let reader = std::thread::spawn(move || {
        reader_parked.store(true, Ordering::SeqCst);
        reader_gate.wait();
        let events = mailbox.drain();
        reader_consumed.store(events.len(), Ordering::SeqCst);
        events
    });

    while !parked.load(Ordering::SeqCst) {
        std::thread::yield_now();
    }

    let mut delivered = 0;
    let mut dropped = 0;
    for id in 0..CHANGES {
        let delivery = broker.publish_change(&RowChange::created("posts", id as i64));
        delivered += delivery.delivered;
        dropped += delivery.dropped;
    }

    // The whole claim: every publish completed while the reader had provably
    // consumed nothing.
    assert_eq!(delivered, CHANGES, "the broadcaster was never refused");
    assert_eq!(
        consumed.load(Ordering::SeqCst),
        0,
        "the reader had not run, so it cannot have been what let the broadcaster through"
    );
    assert_eq!(
        dropped,
        CHANGES - 4,
        "everything past the queue was dropped rather than queued or waited on"
    );
    assert!(
        !client.mailbox().is_closed(),
        "and the client is still here"
    );

    gate.wait();
    let events = reader.join().unwrap();
    assert_eq!(events.len(), 5, "one gap marker and a full queue");
    assert!(matches!(events[0], Event::Lagged { skipped } if skipped as usize == CHANGES - 4));
}

#[test]
fn a_stalled_subscriber_does_not_slow_a_healthy_one() {
    // Same broadcast, two clients: one parked at the gate, one draining as
    // fast as it can. The healthy one must lose nothing.
    let broker = broker_with(CHANGES, Overflow::DropOldest);
    let stalled = broker.connect().unwrap();
    stalled.subscribe(everyone()).unwrap();
    let healthy = broker.connect().unwrap();
    healthy.subscribe(everyone()).unwrap();

    let gate = Arc::new(Barrier::new(2));
    let parked = Arc::new(AtomicBool::new(false));
    let stalled_mailbox = Arc::clone(stalled.mailbox());
    let reader_gate = Arc::clone(&gate);
    let reader_parked = Arc::clone(&parked);
    let reader = std::thread::spawn(move || {
        reader_parked.store(true, Ordering::SeqCst);
        reader_gate.wait();
        stalled_mailbox.drain().len()
    });

    let healthy_mailbox = Arc::clone(healthy.mailbox());
    let drainer = std::thread::spawn(move || {
        let mut seen = 0;
        while seen < CHANGES {
            match healthy_mailbox.recv_timeout(Duration::from_secs(5)) {
                Some(Event::Frame(_)) => seen += 1,
                Some(Event::Lagged { .. }) => panic!("the healthy client must not lag"),
                None => break,
            }
        }
        seen
    });

    while !parked.load(Ordering::SeqCst) {
        std::thread::yield_now();
    }
    for id in 0..CHANGES {
        let delivery = broker.publish_change(&RowChange::created("posts", id as i64));
        assert_eq!(delivery.delivered, 2);
        assert_eq!(delivery.dropped, 0);
    }

    assert_eq!(drainer.join().unwrap(), CHANGES, "nothing lost");
    gate.wait();
    assert_eq!(reader.join().unwrap(), CHANGES);
}

#[test]
fn several_publishers_broadcast_at_once() {
    // The registry is an `RwLock` and a publish takes it for reading, so
    // publishes do not serialise against each other. Four threads, one
    // stalled subscriber, no lost work.
    const THREADS: usize = 4;
    const EACH: usize = 200;

    let broker = broker_with(8, Overflow::DropOldest);
    let client = broker.connect().unwrap();
    client.subscribe(everyone()).unwrap();

    let start = Arc::new(Barrier::new(THREADS));
    let mut handles = Vec::new();
    for thread in 0..THREADS {
        let broker = Arc::clone(&broker);
        let start = Arc::clone(&start);
        handles.push(std::thread::spawn(move || {
            start.wait();
            let mut delivered = 0;
            for id in 0..EACH {
                delivered += broker
                    .publish_change(&RowChange::created("posts", (thread * EACH + id) as i64))
                    .delivered;
            }
            delivered
        }));
    }
    let delivered: usize = handles.into_iter().map(|h| h.join().unwrap()).sum();
    assert_eq!(delivered, THREADS * EACH);
    assert_eq!(client.mailbox().len(), 8, "bounded, whoever was publishing");
}

// ---------------------------------------------------------------------------
// The overflow policy does what it says
// ---------------------------------------------------------------------------

fn ids(events: Vec<Event>) -> (u64, Vec<String>) {
    let mut skipped = 0;
    let mut bodies = Vec::new();
    for event in events {
        match event {
            Event::Lagged { skipped: n } => skipped += n,
            Event::Frame(frame) => {
                let text = String::from_utf8(frame.bytes().to_vec()).unwrap();
                let line = text
                    .lines()
                    .find(|line| line.starts_with("data:id="))
                    .unwrap()
                    .to_owned();
                bodies.push(line);
            }
        }
    }
    (skipped, bodies)
}

#[test]
fn drop_oldest_keeps_the_newest_state() {
    let broker = broker_with(3, Overflow::DropOldest);
    let client = broker.connect().unwrap();
    client.subscribe(everyone()).unwrap();
    for id in 0..6i64 {
        broker.publish_change(&RowChange::created("posts", id));
    }
    let (skipped, bodies) = ids(client.mailbox().drain());
    assert_eq!(skipped, 3);
    assert_eq!(bodies, ["data:id=3", "data:id=4", "data:id=5"]);
    assert!(!client.mailbox().is_closed());
}

#[test]
fn drop_newest_keeps_the_oldest_backlog() {
    let broker = broker_with(3, Overflow::DropNewest);
    let client = broker.connect().unwrap();
    client.subscribe(everyone()).unwrap();
    for id in 0..6i64 {
        broker.publish_change(&RowChange::created("posts", id));
    }
    let (skipped, bodies) = ids(client.mailbox().drain());
    assert_eq!(skipped, 3);
    assert_eq!(bodies, ["data:id=0", "data:id=1", "data:id=2"]);
}

#[test]
fn disconnect_removes_the_client_from_the_index() {
    let broker = broker_with(2, Overflow::Disconnect);
    let client = broker.connect().unwrap();
    client.subscribe(everyone()).unwrap();
    let survivor = broker.connect().unwrap();
    survivor.subscribe(everyone()).unwrap();
    // The survivor keeps draining, so only the first client ever overflows.
    assert_eq!(broker.clients(), 2);

    let mut disconnected = 0;
    for id in 0..6i64 {
        let delivery = broker.publish_change(&RowChange::created("posts", id));
        disconnected += delivery.disconnected;
        survivor.mailbox().drain();
    }
    assert_eq!(disconnected, 1);
    assert_eq!(broker.clients(), 1, "the slow one is gone");
    assert!(client.mailbox().is_closed());
    assert_eq!(
        broker.stats().subscriptions,
        1,
        "and so is its subscription"
    );

    // The handle outlived the registration, and dropping it is still safe.
    drop(client);
    assert_eq!(broker.clients(), 1);
}

// ---------------------------------------------------------------------------
// The caps
// ---------------------------------------------------------------------------

#[test]
fn the_client_cap_refuses_rather_than_evicts() {
    let broker = Broker::builder()
        .config(Config {
            max_clients: 3,
            ..Config::default()
        })
        .build();

    let held: Vec<_> = (0..3).map(|_| broker.connect().unwrap()).collect();
    assert_eq!(
        broker.connect().unwrap_err(),
        ConnectError::AtCapacity {
            connected: 3,
            cap: 3
        }
    );
    // Nobody was thrown off to make room, which is the point: otherwise anyone
    // with a socket can knock everybody else off.
    assert_eq!(broker.clients(), 3);
    assert!(held.iter().all(|client| !client.mailbox().is_closed()));

    drop(held);
    assert_eq!(broker.clients(), 0);
    assert!(broker.connect().is_ok());
}

#[test]
fn the_subscription_cap_holds_per_client() {
    let broker = Broker::builder()
        .config(Config {
            max_subscriptions_per_client: 2,
            ..Config::default()
        })
        .build();
    let client = broker.connect().unwrap();
    client.subscribe(everyone()).unwrap();
    client
        .subscribe(Subscription::gated_by(
            Topic::collection("comments").unwrap(),
            Pred::<PostColumn>::Always,
        ))
        .unwrap();
    assert_eq!(
        client.subscribe(everyone()).unwrap_err(),
        SubscribeError::TooManySubscriptions { held: 2, cap: 2 }
    );

    // Another client has its own budget.
    let other = broker.connect().unwrap();
    assert!(other.subscribe(everyone()).is_ok());
}

#[test]
fn an_awaited_mailbox_wakes_without_a_runtime() {
    // The future is hand written, so any executor drives it -- here, the
    // twenty-line one `brocanth-storage` and `brocanth-admin` both carry.
    let broker = Broker::new();
    let client = broker.connect().unwrap();
    client.subscribe(everyone()).unwrap();

    let mailbox = Arc::clone(client.mailbox());
    let publisher = std::thread::spawn(move || {
        std::thread::yield_now();
        broker.publish_change(&RowChange::created("posts", 1i64));
    });
    let event = block_on(async { mailbox.recv().await });
    assert!(matches!(event, Some(Event::Frame(_))));
    publisher.join().unwrap();
}

/// The same executor `brocanth-storage/src/rt.rs` carries, so this test needs
/// no runtime either.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::sync::atomic::AtomicBool;
    use std::task::{Context, Poll, Wake, Waker};

    struct Signal {
        thread: std::thread::Thread,
        woken: AtomicBool,
    }

    impl Wake for Signal {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.woken.store(true, Ordering::Release);
            self.thread.unpark();
        }
    }

    let mut future = Box::pin(future);
    let signal = Arc::new(Signal {
        thread: std::thread::current(),
        woken: AtomicBool::new(false),
    });
    let waker = Waker::from(Arc::clone(&signal));
    let mut context = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => {
                while !signal.woken.swap(false, Ordering::AcqRel) {
                    std::thread::park();
                }
            }
        }
    }
}
