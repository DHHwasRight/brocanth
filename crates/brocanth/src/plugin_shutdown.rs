//! Shutdown, in two phases, so a pool can drain and a job runner can finish
//! what it started.
//!
//! One phase is not enough, and the reason is the job runner. Told "stop", it
//! has to do two different things: stop *taking* work, and then finish the
//! work it already has. If every plugin is told "stop" once, a runner that
//! blocks until its queue is empty blocks while the queue is still being
//! filled by the request that is in flight, and a pool that closes
//! immediately takes the connection that in-flight job is holding.
//!
//! So:
//!
//! 1. **[`ShutdownPhase::Begin`]** runs for every instance, in reverse
//!    registration order. Close the gate: stop accepting connections, stop
//!    polling the queue, stop handing out pool entries to new callers. Return
//!    quickly — nothing is draining yet.
//! 2. **[`ShutdownPhase::Drain`]** runs for every instance, in reverse
//!    registration order. Finish: wait for in-flight jobs, flush the log,
//!    close the pool. Each instance gets [`ShutdownCx::remaining`], which is
//!    what is left of the application's grace period.
//!
//! **Reverse registration order**, because registration order is dependency
//! order: the ORM is registered after the pool because it needs the pool, so
//! the ORM must be finished with the pool before the pool closes.
//!
//! **One failure does not cancel the rest.** A plugin whose drain fails is
//! recorded in the [`ShutdownReport`] and every other plugin is still asked.
//! A shutdown that stops at the first error leaks exactly the resources it was
//! called to release.

use std::time::{Duration, Instant};

use crate::plugin_api::{PluginError, Services};
use crate::plugin_body::StreamGate;

/// Why the application is stopping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShutdownReason {
    /// Somebody called [`App::shutdown`](crate::plugin_app::App::shutdown).
    Requested,
    /// A signal arrived. The server plugin decides which ones count.
    Signal,
    /// Something failed badly enough to stop the application.
    Failed,
}

impl ShutdownReason {
    pub fn as_str(self) -> &'static str {
        match self {
            ShutdownReason::Requested => "requested",
            ShutdownReason::Signal => "signal",
            ShutdownReason::Failed => "failed",
        }
    }
}

impl std::fmt::Display for ShutdownReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which half of the shutdown is running.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ShutdownPhase {
    /// Stop accepting new work. Return quickly.
    Begin,
    /// Finish the work in hand, inside the remaining grace period.
    Drain,
}

impl ShutdownPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            ShutdownPhase::Begin => "begin_shutdown",
            ShutdownPhase::Drain => "shutdown",
        }
    }
}

impl std::fmt::Display for ShutdownPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a plugin can reach while it is shutting down.
pub struct ShutdownCx<'a> {
    reason: ShutdownReason,
    phase: ShutdownPhase,
    grace: Option<Duration>,
    started: Instant,
    instance: &'a str,
    services: &'a mut Services,
    streams: &'a StreamGate,
}

impl<'a> ShutdownCx<'a> {
    pub(crate) fn new(
        reason: ShutdownReason,
        phase: ShutdownPhase,
        grace: Option<Duration>,
        started: Instant,
        instance: &'a str,
        services: &'a mut Services,
        streams: &'a StreamGate,
    ) -> Self {
        Self {
            reason,
            phase,
            grace,
            started,
            instance,
            services,
            streams,
        }
    }

    pub fn reason(&self) -> ShutdownReason {
        self.reason
    }

    pub fn phase(&self) -> ShutdownPhase {
        self.phase
    }

    /// The instance shutting down.
    pub fn instance(&self) -> &str {
        self.instance
    }

    /// The whole grace period the application allowed, or [`None`] for "take
    /// as long as you need".
    pub fn grace(&self) -> Option<Duration> {
        self.grace
    }

    /// What is left of it, counted from the start of the shutdown and shared
    /// by every plugin — so a job runner that waits does not spend the pool's
    /// budget as well as its own.
    ///
    /// [`None`] means unlimited; `Some(Duration::ZERO)` means the budget is
    /// gone and a plugin should drop what it is holding rather than wait.
    pub fn remaining(&self) -> Option<Duration> {
        self.grace.map(|g| g.saturating_sub(self.started.elapsed()))
    }

    /// Whether the grace period has run out.
    pub fn is_expired(&self) -> bool {
        matches!(self.remaining(), Some(left) if left.is_zero())
    }

    /// How many response bodies are still being produced.
    ///
    /// Already non-zero in phase [`ShutdownPhase::Begin`] and falling through
    /// [`ShutdownPhase::Drain`] as each one ends. A plugin that owns what a
    /// stream reads from -- a broker, a connection pool, a file handle --
    /// reads this before it closes: the gate has already told every stream to
    /// end, so a number that will not fall is a stream that ignored
    /// [`BodyStream::finish`](crate::plugin_body::BodyStream::finish).
    ///
    /// It is a number and not a wait. Only whatever polls a body can make it
    /// fall, and the core polls nothing; the transport awaits
    /// [`StreamGate::drained`].
    pub fn open_streams(&self) -> usize {
        self.streams.open_streams()
    }

    /// The application's gate, for a plugin that wants to wait on it itself.
    pub fn streams(&self) -> &StreamGate {
        self.streams
    }

    pub fn services(&self) -> &Services {
        self.services
    }

    pub fn services_mut(&mut self) -> &mut Services {
        self.services
    }
}

impl std::fmt::Debug for ShutdownCx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShutdownCx")
            .field("reason", &self.reason)
            .field("phase", &self.phase)
            .field("instance", &self.instance)
            .field("remaining", &self.remaining())
            .field("open_streams", &self.open_streams())
            .finish()
    }
}

/// One plugin's failure during shutdown.
#[derive(Debug)]
pub struct ShutdownFailure {
    pub instance: String,
    pub phase: ShutdownPhase,
    pub error: PluginError,
}

impl std::fmt::Display for ShutdownFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "`{}` failed during `{}`: {}",
            self.instance, self.phase, self.error
        )
    }
}

/// What happened when the application stopped.
///
/// Returned rather than thrown, because a shutdown is a report and not a
/// branch: the caller wants every line of it, including the ones after the
/// first failure.
#[derive(Debug)]
pub struct ShutdownReport {
    reason: ShutdownReason,
    order: Vec<String>,
    failures: Vec<ShutdownFailure>,
    elapsed: Duration,
    grace: Option<Duration>,
    streams_left: usize,
}

impl ShutdownReport {
    pub(crate) fn new(reason: ShutdownReason, grace: Option<Duration>) -> Self {
        Self {
            reason,
            order: Vec::new(),
            failures: Vec::new(),
            elapsed: Duration::ZERO,
            grace,
            streams_left: 0,
        }
    }

    pub(crate) fn record(&mut self, instance: &str) {
        self.order.push(instance.to_string());
    }

    pub(crate) fn fail(&mut self, instance: &str, phase: ShutdownPhase, error: PluginError) {
        self.failures.push(ShutdownFailure {
            instance: instance.to_string(),
            phase,
            error,
        });
    }

    pub(crate) fn finish(&mut self, elapsed: Duration, streams_left: usize) {
        self.elapsed = elapsed;
        self.streams_left = streams_left;
    }

    pub fn reason(&self) -> ShutdownReason {
        self.reason
    }

    /// Every instance that was asked, in the order it was asked, `Begin` pass
    /// then `Drain` pass.
    pub fn order(&self) -> &[String] {
        &self.order
    }

    pub fn failures(&self) -> &[ShutdownFailure] {
        &self.failures
    }

    /// Response bodies that were still being produced when the last plugin
    /// had stopped.
    ///
    /// Every one of them was told to end at the start of the shutdown, so a
    /// number above zero means either that the transport had not finished
    /// draining when this ran, or that a stream ignored
    /// [`BodyStream::finish`](crate::plugin_body::BodyStream::finish). Those
    /// clients see a truncated response.
    pub fn streams_left(&self) -> usize {
        self.streams_left
    }

    /// Nothing failed, and no response was cut off.
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty() && self.streams_left == 0
    }

    /// How long the whole shutdown took.
    pub fn elapsed(&self) -> Duration {
        self.elapsed
    }

    /// The shutdown took longer than the grace period allowed.
    pub fn overran(&self) -> bool {
        matches!(self.grace, Some(g) if self.elapsed > g)
    }
}

impl std::fmt::Display for ShutdownReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "shutdown ({}) finished in {:?} after asking {} instance(s)",
            self.reason,
            self.elapsed,
            self.order.len()
        )?;
        if self.streams_left > 0 {
            write!(
                f,
                "\n  {} response stream(s) were still open and were cut off",
                self.streams_left
            )?;
        }
        for failure in &self.failures {
            write!(f, "\n  {failure}")?;
        }
        Ok(())
    }
}
