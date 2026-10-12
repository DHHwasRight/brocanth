//! A thirty-line executor, so the admin's tests need no runtime.
//!
//! [`crate::data::AdminData`] is async because an ORM is. That would normally
//! drag `tokio` into this crate's dev-dependencies, and PLAN §8.2 is clear
//! that a dependency which appears only to make a test compile is still a
//! dependency in the tree. A future that never yields — an in-memory fixture,
//! a synchronous server — completes on the first poll, and parking the thread
//! handles the rest.
//!
//! This is not a runtime. It drives one future on the calling thread and has
//! no timer, no reactor and no spawn. Inside tokio, use tokio.

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
