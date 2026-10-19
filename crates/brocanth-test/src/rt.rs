//! Thirty lines of `std`, so a test client needs no runtime.
//!
//! A contributed route may be an
//! [`AsyncHandler`](brocanth::plugin_routes::AsyncHandler), so driving one is
//! part of this crate's job. Taking `tokio` to do it would put a runtime in
//! the tree of every crate that writes a test, and the core's whole claim is
//! that it holds no runtime. A future that never yields -- an in-memory
//! fixture, a handler that only formats a page -- completes on its first poll,
//! and parking the thread handles the rest.
//!
//! This is not a runtime. It drives one future on the calling thread and has
//! no timer, no reactor and no spawn. A route that waits on a socket or a
//! timer needs [`TestClient::send_async`](crate::TestClient::send_async) and
//! the caller's own runtime; see the note there.
//!
//! The same shape as `brocanth-admin`'s `rt.rs` and
//! `brocanth-server`'s `bridge::block_on`, deliberately: three copies of
//! thirty lines, and no fourth crate in anybody's tree.

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
///
/// A spurious unpark polls again, which every future permits.
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
