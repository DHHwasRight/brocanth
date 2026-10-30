//! Readiness: each plugin says how to check that it can serve, and shutdown
//! turns the answer to "no" before anything drains.
//!
//! Two questions, and the core owns only the second.
//!
//! **Is the process alive?** A liveness probe asks nothing of a plugin. If the
//! process can answer an HTTP request at all, it is alive. A probe that pings
//! the database here restarts every pod when the database blinks, which turns
//! one outage into two. So liveness is not a seam, and this module has no
//! liveness type.
//!
//! **Can the process take traffic?** Only the plugins know. The pool knows
//! whether it can reach the database; the queue knows whether its table is
//! there. A plugin registers one [`ReadinessCheck`] with
//! [`Lifecycle::ready_when`](crate::plugin_api::Lifecycle::ready_when), and
//! whatever serves the probe runs every check and reports each one.
//!
//! # Why the core, and why a future
//!
//! The seam has to be in the core for the reason every other seam is: a jobs
//! plugin cannot name an observability crate, and an observability crate
//! cannot name every plugin. The core names neither. It keeps a list of
//! checks and one bit, and it never runs a check, because it has no runtime
//! (§8). A check returns a [`BoxFuture`], the same type an async route
//! returns, and the thing that serves the probe awaits it under a timeout it
//! chooses.
//!
//! # Shutdown flips it first (ADR 2)
//!
//! A load balancer routes to a pod until its readiness probe fails. If the
//! pod stops accepting before the probe fails, the requests that the balancer
//! still sends are refused. So [`Readiness::drain`] runs **before**
//! `begin_shutdown`: [`App::shutdown`](crate::plugin_app::App::shutdown) calls
//! it first, and a transport that wants a delay between "not ready" and "stop
//! accepting" calls it earlier and waits. After `drain`, every probe answers
//! "not ready" without running a check: a check that passes on a draining
//! process is a lie.

use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use crate::plugin_routes::BoxFuture;

/// One way to ask a plugin whether it can serve.
///
/// `Ok(())` means ready. `Err` carries a short reason for the probe body:
/// "connection refused", "relation brocanth_jobs does not exist". The reason
/// goes to an operator, so it says what failed and not a stack.
///
/// A closure that returns a `'static` future is a check:
///
/// ```
/// # use brocanth::plugin_health::Readiness;
/// let readiness = Readiness::new();
/// readiness.add("pool", || async { Ok::<(), String>(()) });
/// assert_eq!(readiness.checks().len(), 1);
/// ```
pub trait ReadinessCheck: Send + Sync + 'static {
    /// Runs the check once.
    ///
    /// Keep it cheap. A probe runs it every few seconds on every replica. A
    /// primary-key lookup or `SELECT 1` is right; a `COUNT(*)` is not.
    fn check(&self) -> BoxFuture<'_, Result<(), String>>;
}

impl<F, Fut> ReadinessCheck for F
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    fn check(&self) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(self())
    }
}

/// One registered check, with the name it reports under.
#[derive(Clone)]
pub struct Check {
    name: String,
    probe: Arc<dyn ReadinessCheck>,
}

impl Check {
    /// The name in the probe body. A plugin's check carries its instance name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The check itself, shareable, so a caller can move it into a task.
    pub fn probe(&self) -> Arc<dyn ReadinessCheck> {
        Arc::clone(&self.probe)
    }
}

impl fmt::Debug for Check {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Check").field("name", &self.name).finish()
    }
}

#[derive(Default)]
struct Inner {
    checks: RwLock<Vec<Check>>,
    draining: AtomicBool,
}

/// The application's readiness checks, and the bit that shutdown flips.
///
/// Cloneable, and every clone is the same list: the application holds one,
/// the probe holds one, and a plugin that wants to read the bit holds one.
/// It belongs to one application. Two applications in one process have two.
#[derive(Clone, Default)]
pub struct Readiness {
    inner: Arc<Inner>,
}

impl Readiness {
    /// An empty list that is not draining.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a check under `name`.
    ///
    /// Two checks with one name are both kept and both reported. A plugin
    /// uses [`Lifecycle::ready_when`](crate::plugin_api::Lifecycle::ready_when),
    /// which names the check after its instance; the application's own
    /// checks come through here.
    pub fn add(&self, name: impl Into<String>, check: impl ReadinessCheck) {
        let check = Check {
            name: name.into(),
            probe: Arc::new(check),
        };
        self.inner
            .checks
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(check);
    }

    /// Every check, in the order it was added.
    ///
    /// A copy, so a caller awaits the checks without holding a lock across
    /// an `.await`.
    pub fn checks(&self) -> Vec<Check> {
        self.inner
            .checks
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// From now on, the application is not ready, whatever the checks say.
    ///
    /// Idempotent, and there is no way back: a process that began to stop
    /// does not start again. [`App::shutdown`](crate::plugin_app::App::shutdown)
    /// calls it before `begin_shutdown`.
    pub fn drain(&self) {
        self.inner.draining.store(true, Ordering::SeqCst);
    }

    /// Whether [`Readiness::drain`] was called.
    pub fn is_draining(&self) -> bool {
        self.inner.draining.load(Ordering::SeqCst)
    }
}

impl fmt::Debug for Readiness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Readiness")
            .field("checks", &self.checks())
            .field("draining", &self.is_draining())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    fn ready_now<T>(fut: BoxFuture<'_, T>) -> T {
        let mut fut = pin!(fut);
        let mut cx = Context::from_waker(Waker::noop());
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("the test future was not ready"),
        }
    }

    #[test]
    fn a_closure_is_a_check_and_its_answer_comes_back() {
        let readiness = Readiness::new();
        readiness.add("ok", || async { Ok(()) });
        readiness.add("down", || async { Err("connection refused".to_string()) });

        let checks = readiness.checks();
        assert_eq!(checks.len(), 2);
        assert_eq!(checks[0].name(), "ok");
        assert_eq!(ready_now(checks[0].probe().check()), Ok(()));
        assert_eq!(
            ready_now(checks[1].probe().check()),
            Err("connection refused".to_string())
        );
    }

    #[test]
    fn drain_is_shared_by_every_clone_and_does_not_reset() {
        let readiness = Readiness::new();
        let probe_side = readiness.clone();
        assert!(!probe_side.is_draining());
        readiness.drain();
        readiness.drain();
        assert!(probe_side.is_draining());
    }
}
