//! Waking a worker the moment there is work, instead of asking twice a
//! second whether there is.
//!
//! # Why a listener at all
//!
//! A poll is a latency floor and a load floor at the same time, and the two
//! pull in opposite directions. Poll every 500 ms and an empty queue costs
//! two index scans per worker per second for ever; poll every 5 s and a job
//! enqueued by a web request waits five seconds to start. Neither number is
//! right, because the question itself is wrong: the database already knows
//! when a job arrives.
//!
//! Postgres can say so. `pg_notify` runs inside the insert, so the
//! announcement is delivered when the insert commits -- never before the row
//! is visible, and never at all if the enqueue rolls back. That last part is
//! what makes this safe to fold into a transaction the application owns: a
//! job enqueued beside a business write is announced exactly when that write
//! lands.
//!
//! # What it does not replace
//!
//! The poll stays, slowed down, and it is not redundant:
//!
//! * **A job scheduled for later** is announced when it is *inserted*, not
//!   when it becomes due. Nothing fires at 09:00.
//! * **A notification sent while nobody was listening is gone.** Postgres
//!   does not queue them for an absent listener, so a worker that was
//!   reconnecting missed whatever was said.
//! * **The connection can drop** and the first symptom of a listener that
//!   quietly died would otherwise be a queue that stops draining.
//!
//! So the worker sleeps until the soonest job is due, capped at
//! [`WorkerConfig::safety_poll`](crate::WorkerConfig::safety_poll), and a
//! notification only ever shortens that sleep. Correctness never depends on
//! a notification arriving. Losing every one of them costs latency and
//! nothing else, which is the property worth having: this is an
//! optimisation that cannot become a bug.
//!
//! # SQLite
//!
//! There is no equivalent, so there is no listener, and a SQLite worker
//! polls at [`WorkerConfig::poll_interval`](crate::WorkerConfig::poll_interval).
//! Stated rather than hidden: the two backends have the same guarantees and
//! different idle latency, and pretending otherwise with a file watcher
//! would be a second mechanism to get wrong.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;

use crate::events::JobEvents;
#[cfg(feature = "postgres")]
use crate::events::JobEvent;

/// The shared "something happened" bell a worker waits on.
///
/// A [`Notify`] and not a channel: a worker does not care how many jobs
/// arrived or on which queue, only that it should look again. Collapsing a
/// burst of a thousand enqueues into one wake-up is the behaviour wanted,
/// and it is what `notify_waiters` does for free.
#[derive(Clone, Default)]
pub(crate) struct Bell(Arc<Notify>);

impl Bell {
    /// Waits for the next ring. A ring that happened before this call is not
    /// remembered, which is safe: the caller re-reads the table either way.
    pub(crate) async fn rung(&self) {
        self.0.notified().await;
    }

    /// Wakes everybody waiting. Only the Postgres listener rings it, so in
    /// a build without one the method is absent rather than unused.
    #[cfg(feature = "postgres")]
    pub(crate) fn ring(&self) {
        self.0.notify_waiters();
    }
}

/// Starts a listener where there is something to listen to.
///
/// One function with two bodies rather than a `cfg` at the call site: the
/// worker asks for a listener and finds out whether it got one, and a build
/// with no Postgres in it contains none of the code below rather than a
/// branch that declines.
#[cfg(feature = "postgres")]
pub(crate) fn spawn(
    store: &crate::store::Store,
    bell: &Bell,
    events: Arc<dyn JobEvents>,
    backoff: Duration,
    enabled: bool,
) -> Option<tokio::task::JoinHandle<()>> {
    if !enabled {
        return None;
    }
    let pool = store.listen_pool()?;
    Some(tokio::spawn(listen(pool, bell.clone(), events, backoff)))
}

/// The same, for a build with no Postgres. There is nothing to listen to,
/// so the worker polls and the code above is not linked.
#[cfg(not(feature = "postgres"))]
pub(crate) fn spawn(
    _store: &crate::store::Store,
    _bell: &Bell,
    _events: Arc<dyn JobEvents>,
    _backoff: Duration,
    _enabled: bool,
) -> Option<tokio::task::JoinHandle<()>> {
    None
}

/// Listens on the Postgres channel and rings the bell, for ever.
///
/// Returns only when the handle is dropped. Every error is a reconnect: the
/// listener is an optimisation, so it must never be a reason the worker
/// stops. It rings the bell after a reconnect as well, because whatever was
/// said while it was away is lost and the table is the only way to find out.
#[cfg(feature = "postgres")]
pub(crate) async fn listen(
    pool: sqlx::PgPool,
    bell: Bell,
    events: Arc<dyn JobEvents>,
    backoff: Duration,
) {
    use sqlx::postgres::PgListener;

    loop {
        match PgListener::connect_with(&pool).await {
            Ok(mut listener) => {
                if let Err(e) = listener.listen(crate::store::sql::CHANNEL).await {
                    events.handle(JobEvent::ListenerFailed {
                        error: e.to_string(),
                    });
                    tokio::time::sleep(backoff).await;
                    continue;
                }
                events.handle(JobEvent::ListenerConnected);
                // Whatever arrived while this listener was being set up was
                // said to nobody. One ring covers all of it.
                bell.ring();
                loop {
                    match listener.recv().await {
                        Ok(_) => bell.ring(),
                        Err(e) => {
                            events.handle(JobEvent::ListenerFailed {
                                error: e.to_string(),
                            });
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                events.handle(JobEvent::ListenerFailed {
                    error: e.to_string(),
                });
            }
        }
        tokio::time::sleep(backoff).await;
    }
}
