//! The bounded queue, and §10's second correction.
//!
//! > Bound the per-client queue with an explicit overflow policy, because an
//! > unbuffered channel lets one slow reader block the broadcaster.
//!
//! # The property, stated as a rule the code obeys
//!
//! **[`Mailbox::push`] never waits for the reader.** It takes a mutex, moves
//! one [`Frame`] (an `Arc`) into or out of a `VecDeque`, and returns. The
//! critical section contains no I/O, no allocation of frame bytes, no user
//! code and no second lock, so the only thing it can ever wait for is another
//! `push` or a `drain` on the *same* mailbox — never on a subscriber reading,
//! never on a socket flushing.
//!
//! A reader that goes away entirely cannot stall anybody: the queue fills, the
//! overflow policy fires, and the broadcaster keeps going. The test
//! `a_stalled_subscriber_does_not_stall_the_broadcaster` proves it by ordering
//! rather than by a clock: the reader is parked and provably has not run when
//! the broadcast finishes.
//!
//! # The overflow policy, and why the default is what it is
//!
//! [`Overflow::DropOldest`] is the default. Each of the three is defensible
//! and each is wrong for someone, so here is the reasoning rather than the
//! verdict alone:
//!
//! * **`DropOldest`** — a realtime stream of row changes is a stream of
//!   *state notifications*. The newest frame is the closest to the truth and
//!   the oldest is the most stale, so dropping from the front keeps a client
//!   that is catching up as close to current as the queue allows, and a client
//!   that re-renders from the newest fragment self-heals. This is the default.
//! * **`DropNewest`** — keeps the oldest, which pins a stalled client to an
//!   ancient view it can never catch up from. Correct when the queue is a
//!   *prefix* of a log that the client will replay from a cursor, which this
//!   crate's frames are not. Available, not the default.
//! * **`Disconnect`** — correct when a gap is a correctness bug and a resync
//!   is the only honest repair: a client that cannot miss an event should be
//!   told it has, loudly. It is not the default because it converts a laptop
//!   lid, a garbage-collection pause or a slow mobile uplink into a
//!   reconnection, and the reconnection costs more than the frame did.
//!
//! **A drop is never silent.** Under either dropping policy the mailbox
//! records the gap, and the next read yields [`Event::Lagged`] before the next
//! frame. A client that cannot tolerate a gap reacts to that marker by
//! refetching; one that can, ignores it. The precedent is `tokio::sync::
//! broadcast`'s `RecvError::Lagged`, which is the same decision for the same
//! reason: a silently lossy stream is worse than a visibly lossy one, because
//! the application cannot tell it is diverging.
//!
//! # Capacity, and the arithmetic to do before raising it
//!
//! The default is [`Config::DEFAULT_QUEUE`](crate::Config::DEFAULT_QUEUE) = 32
//! frames. A frame is an `Arc`, shared across every subscriber in its audience
//! class, so the bytes are counted once; the queue costs one pointer per slot
//! plus the frames it is the last holder of. At the default client cap of
//! 1000, 32 frames of a 2 KiB fragment is 64 MiB of worst-case backlog — the
//! case where every client stalls at once holding entirely distinct frames.
//! Raise the queue and multiply that number by the same factor.
//!
//! # No runtime
//!
//! A mailbox is readable three ways and none of them is a dependency:
//! [`Mailbox::try_recv`] (never blocks), [`Mailbox::recv_blocking`] /
//! [`Mailbox::recv_timeout`] (a `Condvar`, for a thread per connection), and
//! [`Mailbox::recv`] (a hand-written `Future` holding a `Waker`, for any
//! executor at all). The same mailbox serves all three.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Condvar, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use crate::frame::Frame;

/// What happens when a subscriber's queue is full.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Overflow {
    /// Drop the front of the queue to make room. The default; see the module
    /// documentation for why.
    #[default]
    DropOldest,
    /// Refuse the new frame and keep the backlog.
    DropNewest,
    /// Close the mailbox. The broker then removes the client.
    Disconnect,
}

/// What a reader gets out of a mailbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A frame, encoded and ready to write.
    Frame(Frame),
    /// The queue overflowed and this many frames were dropped since the last
    /// time the reader was told. Yielded **before** the frame that follows the
    /// gap, so a client knows its view is stale at the moment it matters.
    Lagged {
        /// How many frames went missing.
        skipped: u64,
    },
}

/// What [`Mailbox::push`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pushed {
    /// Queued.
    Queued,
    /// The queue was full and a frame was dropped, per the overflow policy.
    Dropped,
    /// The mailbox is closed — the client disconnected, or
    /// [`Overflow::Disconnect`] just fired. The broker removes it.
    Closed,
}

#[derive(Debug)]
struct State {
    queue: VecDeque<Frame>,
    skipped: u64,
    closed: bool,
    waker: Option<Waker>,
    delivered: u64,
    dropped: u64,
}

/// One client's bounded queue of encoded frames.
#[derive(Debug)]
pub struct Mailbox {
    capacity: usize,
    overflow: Overflow,
    state: Mutex<State>,
    ready: Condvar,
}

impl Mailbox {
    /// A mailbox holding at most `capacity` frames.
    ///
    /// A capacity of zero is raised to one: a queue that cannot hold a single
    /// frame is an unbuffered channel, which is the thing §10 names as the
    /// bug.
    pub fn new(capacity: usize, overflow: Overflow) -> Mailbox {
        Mailbox {
            capacity: capacity.max(1),
            overflow,
            state: Mutex::new(State {
                queue: VecDeque::new(),
                skipped: 0,
                closed: false,
                waker: None,
                delivered: 0,
                dropped: 0,
            }),
            ready: Condvar::new(),
        }
    }

    /// How many frames this mailbox will hold.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// The overflow policy in force.
    pub fn overflow(&self) -> Overflow {
        self.overflow
    }

    /// Queue a frame. **Never waits for the reader.**
    ///
    /// The whole of §10's second correction is in this method's contract: the
    /// broadcaster calls it once per subscriber per change, and it returns in
    /// bounded time whatever the subscriber is doing.
    pub fn push(&self, frame: Frame) -> Pushed {
        let mut state = self.lock();
        if state.closed {
            return Pushed::Closed;
        }
        let outcome = if state.queue.len() < self.capacity {
            state.queue.push_back(frame);
            state.delivered += 1;
            Pushed::Queued
        } else {
            match self.overflow {
                Overflow::DropOldest => {
                    state.queue.pop_front();
                    state.queue.push_back(frame);
                    state.skipped += 1;
                    state.dropped += 1;
                    state.delivered += 1;
                    Pushed::Dropped
                }
                Overflow::DropNewest => {
                    state.skipped += 1;
                    state.dropped += 1;
                    Pushed::Dropped
                }
                Overflow::Disconnect => {
                    state.closed = true;
                    state.queue.clear();
                    state.dropped += 1;
                    Pushed::Closed
                }
            }
        };
        let waker = state.waker.take();
        drop(state);
        self.ready.notify_all();
        if let Some(waker) = waker {
            waker.wake();
        }
        outcome
    }

    /// Take one event, or `None` when the mailbox is empty or closed.
    pub fn try_recv(&self) -> Option<Event> {
        let mut state = self.lock();
        Self::take(&mut state)
    }

    /// Take every event waiting, in order.
    ///
    /// What a transport calls when its socket became writable: one lock, one
    /// move, and the broadcaster is never held up for the length of a write.
    pub fn drain(&self) -> Vec<Event> {
        let mut state = self.lock();
        let mut out = Vec::with_capacity(state.queue.len() + 1);
        while let Some(event) = Self::take(&mut state) {
            out.push(event);
        }
        out
    }

    /// Block this thread until an event arrives, or the mailbox closes.
    pub fn recv_blocking(&self) -> Option<Event> {
        let mut state = self.lock();
        loop {
            if let Some(event) = Self::take(&mut state) {
                return Some(event);
            }
            if state.closed {
                return None;
            }
            state = self.ready.wait(state).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Block this thread for at most `timeout`.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<Event> {
        let deadline = std::time::Instant::now() + timeout;
        let mut state = self.lock();
        loop {
            if let Some(event) = Self::take(&mut state) {
                return Some(event);
            }
            if state.closed {
                return None;
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return None;
            }
            let (next, _) = self
                .ready
                .wait_timeout(state, left)
                .unwrap_or_else(|e| e.into_inner());
            state = next;
        }
    }

    /// Await an event, on any executor, with no runtime dependency.
    pub fn recv(&self) -> Recv<'_> {
        Recv { mailbox: self }
    }

    /// Poll for an event, registering `context`'s waker when there is none.
    ///
    /// The primitive [`Mailbox::recv`] is written on; public because a
    /// transport implementing `Stream` wants exactly this signature and should
    /// not have to go through a temporary future to get it.
    pub fn poll_recv(&self, context: &mut Context<'_>) -> Poll<Option<Event>> {
        let mut state = self.lock();
        if let Some(event) = Self::take(&mut state) {
            return Poll::Ready(Some(event));
        }
        if state.closed {
            return Poll::Ready(None);
        }
        state.waker = Some(context.waker().clone());
        Poll::Pending
    }

    /// Close the mailbox. A reader drains what is queued and then sees the
    /// end; a `push` after this reports [`Pushed::Closed`].
    pub fn close(&self) {
        let mut state = self.lock();
        state.closed = true;
        let waker = state.waker.take();
        drop(state);
        self.ready.notify_all();
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    /// `true` once the mailbox is closed.
    pub fn is_closed(&self) -> bool {
        self.lock().closed
    }

    /// How many frames are waiting.
    pub fn len(&self) -> usize {
        self.lock().queue.len()
    }

    /// `true` when nothing is waiting.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How many frames this mailbox has accepted since it was created.
    pub fn delivered(&self) -> u64 {
        self.lock().delivered
    }

    /// How many frames the overflow policy has destroyed since this mailbox
    /// was created. The number an operator watches.
    pub fn dropped(&self) -> u64 {
        self.lock().dropped
    }

    fn take(state: &mut State) -> Option<Event> {
        if state.skipped > 0 {
            let skipped = std::mem::take(&mut state.skipped);
            return Some(Event::Lagged { skipped });
        }
        state.queue.pop_front().map(Event::Frame)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        // A panic in a reader must not take the broadcaster down with it: the
        // queue has no invariant a panic could have broken halfway, so the
        // poison is noise and clearing it is the honest reading.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The future [`Mailbox::recv`] returns.
#[derive(Debug)]
pub struct Recv<'a> {
    mailbox: &'a Mailbox,
}

impl Future for Recv<'_> {
    type Output = Option<Event>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        self.mailbox.poll_recv(context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Topic;

    fn frame(n: u64) -> Frame {
        Frame::sse(n, &Topic::collection("posts").unwrap(), format!("n={n}"))
    }

    fn data(event: Event) -> String {
        match event {
            Event::Frame(frame) => String::from_utf8(frame.bytes().to_vec()).unwrap(),
            Event::Lagged { skipped } => panic!("lagged {skipped}"),
        }
    }

    #[test]
    fn drop_oldest_keeps_the_newest_and_reports_the_gap() {
        let mailbox = Mailbox::new(2, Overflow::DropOldest);
        assert_eq!(mailbox.push(frame(1)), Pushed::Queued);
        assert_eq!(mailbox.push(frame(2)), Pushed::Queued);
        assert_eq!(mailbox.push(frame(3)), Pushed::Dropped);

        assert_eq!(mailbox.try_recv(), Some(Event::Lagged { skipped: 1 }));
        assert!(data(mailbox.try_recv().unwrap()).contains("n=2"));
        assert!(data(mailbox.try_recv().unwrap()).contains("n=3"));
        assert_eq!(mailbox.try_recv(), None);
        assert_eq!(mailbox.dropped(), 1);
    }

    #[test]
    fn drop_newest_keeps_the_backlog_and_reports_the_gap() {
        let mailbox = Mailbox::new(2, Overflow::DropNewest);
        mailbox.push(frame(1));
        mailbox.push(frame(2));
        assert_eq!(mailbox.push(frame(3)), Pushed::Dropped);

        assert_eq!(mailbox.try_recv(), Some(Event::Lagged { skipped: 1 }));
        assert!(data(mailbox.try_recv().unwrap()).contains("n=1"));
        assert!(data(mailbox.try_recv().unwrap()).contains("n=2"));
    }

    #[test]
    fn disconnect_closes_and_stays_closed() {
        let mailbox = Mailbox::new(1, Overflow::Disconnect);
        assert_eq!(mailbox.push(frame(1)), Pushed::Queued);
        assert_eq!(mailbox.push(frame(2)), Pushed::Closed);
        assert!(mailbox.is_closed());
        assert_eq!(mailbox.push(frame(3)), Pushed::Closed);
        assert_eq!(mailbox.recv_blocking(), None);
    }

    #[test]
    fn a_zero_capacity_mailbox_is_not_an_unbuffered_channel() {
        let mailbox = Mailbox::new(0, Overflow::DropOldest);
        assert_eq!(mailbox.capacity(), 1);
        assert_eq!(mailbox.push(frame(1)), Pushed::Queued);
    }

    #[test]
    fn closing_wakes_a_blocked_reader() {
        let mailbox = std::sync::Arc::new(Mailbox::new(4, Overflow::DropOldest));
        let reader = std::sync::Arc::clone(&mailbox);
        let handle = std::thread::spawn(move || reader.recv_blocking());
        mailbox.close();
        assert_eq!(handle.join().unwrap(), None);
    }
}
