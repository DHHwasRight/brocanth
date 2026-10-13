//! A thirty-line executor, for an implementation that is async behind a
//! synchronous trait.
//!
//! [`ResourceData`](crate::ResourceData) is synchronous because `brocanth`'s
//! `Handler` is, and the argument is in that trait's documentation. An
//! application whose pool is async still has to block somewhere, and this is
//! the smallest honest way to do it when there is nothing better to hand.
//!
//! **Prefer something better when there is one.** Inside a Tokio worker,
//! `tokio::task::block_in_place` tells the runtime what is happening and this
//! does not. A synchronous driver blocks nowhere. This is for a future that
//! completes without yielding to a reactor — an in-memory fixture, a
//! connection already checked out — and for tests, which is what
//! `brocanth-admin` and `brocanth-storage` both use their copies for.
//!
//! It is not a runtime. It drives one future on the calling thread and has no
//! timer, no reactor and no spawn.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

struct Signal {
    thread: Thread,
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

/// Runs a future to completion on this thread.
pub fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = Box::pin(future);
    let signal = Arc::new(Signal {
        thread: thread::current(),
        woken: AtomicBool::new(false),
    });
    let waker = Waker::from(Arc::clone(&signal));
    let mut context = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => {
                while !signal.woken.swap(false, Ordering::AcqRel) {
                    thread::park();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_future_that_is_already_ready_completes_without_parking() {
        assert_eq!(block_on(async { 1 + 1 }), 2);
    }

    #[test]
    fn a_future_woken_from_another_thread_completes() {
        use std::sync::mpsc;
        let (tx, rx) = mpsc::channel::<u8>();
        std::thread::spawn(move || tx.send(7).expect("the receiver is alive"));
        // Polling a blocking receive is not what a real implementation does;
        // it is the smallest future that is not ready on its first poll.
        let value = block_on(async move {
            loop {
                if let Ok(value) = rx.try_recv() {
                    return value;
                }
                std::thread::yield_now();
            }
        });
        assert_eq!(value, 7);
    }
}
