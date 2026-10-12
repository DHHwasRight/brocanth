//! Backtraces for panicking jobs, opt-in.
//!
//! The dead letter is supposed to hold "the payload, the error, the attempt
//! count and the backtrace where available", and *where available* is doing
//! real work in that sentence. There are two cases and they are not alike.
//!
//! **A returned error.** Pulling a backtrace out of an arbitrary error needs
//! `std::error::request_ref::<Backtrace>`, which is still unstable, and
//! requiring `Job::Error: std::error::Error` to get a field we could only
//! sometimes fill is a worse trade than not having the field. So: no backtrace,
//! and the error's `Display` chain instead.
//!
//! **A panic.** This is the case where a backtrace is worth most — a panic
//! message is usually `index out of bounds` and tells you nothing — and it is
//! the case std actually supports, through the panic hook. So
//! [`capture_backtraces`] installs a hook that captures one, and the worker
//! reads it back on the same thread immediately after catching the panic.
//!
//! It is opt-in because installing a global panic hook is exactly the kind of
//! thing a library must not do behind an application's back: there is one hook
//! per process, and a queue that takes it has taken it from whatever the
//! application installed. [`capture_backtraces`] chains to the previous hook
//! rather than replacing it, and it is still the application's call to make.

use std::backtrace::Backtrace;
use std::cell::RefCell;
use std::sync::Once;

thread_local! {
    static LAST: RefCell<Option<String>> = const { RefCell::new(None) };
}

static INSTALLED: Once = Once::new();

/// Installs a panic hook that records a backtrace for the dead letter.
///
/// Call it once, early, from the application's `main`. Calling it again does
/// nothing. The previously installed hook still runs, so this does not silence
/// whatever else was printing panics.
///
/// A backtrace is captured according to the usual environment variables:
/// `RUST_BACKTRACE=1` for frames, `RUST_BACKTRACE=full` for all of them, and
/// nothing at all when they are unset — which is `std`'s policy, not this
/// crate's, and means the field is empty on a production binary that has not
/// asked for backtraces.
pub fn capture_backtraces() {
    INSTALLED.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let trace = Backtrace::capture();
            if trace.status() == std::backtrace::BacktraceStatus::Captured {
                let text = trace.to_string();
                LAST.with(|slot| *slot.borrow_mut() = Some(text));
            }
            previous(info);
        }));
    });
}

/// Takes the backtrace recorded by the most recent panic **on this thread**.
///
/// Correct because of where it is called: the worker catches the panic with
/// `catch_unwind`, which returns on the thread that panicked, and reads this
/// immediately. Returns `None` when [`capture_backtraces`] was never called or
/// backtraces are switched off.
pub(crate) fn take_backtrace() -> Option<String> {
    LAST.with(|slot| slot.borrow_mut().take())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_the_hook_there_is_nothing_to_take() {
        // Deliberately does not install the hook: other tests in this binary
        // share the process, and a test that asserts "no backtrace" must not be
        // the thing that installs one.
        LAST.with(|slot| *slot.borrow_mut() = None);
        assert!(take_backtrace().is_none());
    }

    #[test]
    fn taking_a_backtrace_clears_it() {
        LAST.with(|slot| *slot.borrow_mut() = Some("frames".into()));
        assert_eq!(take_backtrace().as_deref(), Some("frames"));
        assert!(take_backtrace().is_none());
    }
}
