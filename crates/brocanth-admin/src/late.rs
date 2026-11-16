//! A handle a tab is given when the admin is built, and that arrives when
//! the application starts.
//!
//! A generated project builds its admin with the route table, before any
//! plugin starts. A plugin opens its cache, its queue or its store later,
//! at `before_start`. So a tab cannot be handed the handle when it is
//! built. It is handed a [`Late`] cell instead, and the cell is filled from
//! the first request that carries the handle on its context.
//!
//! # Why not a process-wide global
//!
//! The first version of every tab read a `static INSTALLED: OnceLock<_>`
//! that the plugin set at start. Two applications in one process (two
//! tests, say) then shared the first one's cache. Worse, the tab could not
//! move: the backoffice is meant to become a second service
//! (`docs/module-capabilities.md` §5), and a tab that reads a global of the
//! application's process cannot run in another process. A cell belongs to
//! one admin. What fills it is a function of the request, which a second
//! service replaces with a client of the application.
//!
//! ```
//! use brocanth_admin::late::LateCells;
//!
//! struct Ctx { queue: Option<&'static str> }
//!
//! let mut late = LateCells::<Ctx>::new();
//! let queue = late.cell(|ctx: &Ctx| ctx.queue);
//! assert!(queue.get().is_none());
//!
//! late.fill(&Ctx { queue: None });
//! assert!(queue.get().is_none(), "nothing on the context yet");
//! late.fill(&Ctx { queue: Some("the queue") });
//! assert_eq!(queue.get(), Some(&"the queue"));
//! ```

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use crate::admin::AdminError;

/// A value a tab is given now and that is set once, later.
///
/// Cloning shares the cell. Set once; a second value is dropped, because
/// one admin serves one application.
pub struct Late<T> {
    cell: Arc<OnceLock<T>>,
}

impl<T> Late<T> {
    /// An empty cell.
    pub fn new() -> Self {
        Self {
            cell: Arc::new(OnceLock::new()),
        }
    }

    /// A cell that holds `value` already: for an admin built after start,
    /// or a test.
    pub fn ready(value: T) -> Self {
        let late = Self::new();
        let _ = late.cell.set(value);
        late
    }

    /// Sets the value, unless one is set. `true` when this call set it.
    pub fn set(&self, value: T) -> bool {
        self.cell.set(value).is_ok()
    }

    /// The value, once it is set.
    pub fn get(&self) -> Option<&T> {
        self.cell.get()
    }

    /// The value, or the error a tab shows before the application started.
    /// `what` names the handle: "the cache".
    ///
    /// # Errors
    /// [`AdminError::Config`] with a `help:` line while the cell is empty.
    pub fn require(&self, what: &str) -> Result<&T, AdminError> {
        self.get().ok_or_else(|| {
            AdminError::Config(format!(
                "this tab has no {what} yet: no request with it on its context has come\n\
                 help: register the plugin that provides it; the tab reads it from the \
                 request context of the running application"
            ))
        })
    }
}

impl<T> Default for Late<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Clone for Late<T> {
    fn clone(&self) -> Self {
        Self {
            cell: Arc::clone(&self.cell),
        }
    }
}

impl<T> fmt::Debug for Late<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Late")
            .field("set", &self.cell.get().is_some())
            .finish()
    }
}

/// One way to fill one cell from a request context.
type Filler<C> = Box<dyn Fn(&C) -> bool + Send + Sync>;

/// The [`Late`] cells of one admin, and how each one is filled from a
/// request context `C`.
///
/// The generated `plugins::admin_panels` makes one cell per tab with
/// [`LateCells::cell`], and the admin's handler calls [`LateCells::fill`]
/// with each request's context before it dispatches. Once every cell is
/// set, a fill is one atomic load.
pub struct LateCells<C: ?Sized> {
    fillers: Vec<Filler<C>>,
    done: AtomicBool,
}

impl<C: ?Sized> LateCells<C> {
    /// No cells.
    pub fn new() -> Self {
        Self {
            fillers: Vec::new(),
            done: AtomicBool::new(false),
        }
    }

    /// A new cell, filled by `read` from the first request context on which
    /// it answers `Some`.
    ///
    /// `read` takes owned data off the context: a handle such as a queue or
    /// a store, which a split backoffice replaces with a client.
    pub fn cell<T, F>(&mut self, read: F) -> Late<T>
    where
        T: Send + Sync + 'static,
        F: Fn(&C) -> Option<T> + Send + Sync + 'static,
    {
        let late = Late::new();
        let cell = late.clone();
        self.fillers.push(Box::new(move |cx: &C| {
            if cell.get().is_some() {
                return true;
            }
            match read(cx) {
                Some(value) => {
                    let _ = cell.set(value);
                    true
                }
                None => false,
            }
        }));
        self.done.store(false, Ordering::Release);
        late
    }

    /// Fills every empty cell that `cx` carries a value for.
    pub fn fill(&self, cx: &C) {
        if self.done.load(Ordering::Acquire) {
            return;
        }
        let mut all = true;
        for filler in &self.fillers {
            all &= filler(cx);
        }
        if all {
            self.done.store(true, Ordering::Release);
        }
    }

    /// How many cells there are.
    pub fn len(&self) -> usize {
        self.fillers.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.fillers.is_empty()
    }
}

impl<C: ?Sized> Default for LateCells<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C: ?Sized> fmt::Debug for LateCells<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LateCells")
            .field("cells", &self.fillers.len())
            .field("done", &self.done.load(Ordering::Relaxed))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_admins_have_two_cells() {
        let mut one = LateCells::<Option<u32>>::new();
        let mut two = LateCells::<Option<u32>>::new();
        let a = one.cell(|cx: &Option<u32>| *cx);
        let b = two.cell(|cx: &Option<u32>| *cx);
        one.fill(&Some(1));
        two.fill(&Some(2));
        assert_eq!((a.get(), b.get()), (Some(&1), Some(&2)));
    }

    #[test]
    fn the_first_value_stays() {
        let mut cells = LateCells::<u32>::new();
        let cell = cells.cell(|cx: &u32| Some(*cx));
        cells.fill(&1);
        cells.fill(&2);
        assert_eq!(cell.get(), Some(&1));
    }

    #[test]
    fn an_empty_cell_says_what_to_do() {
        let late = Late::<u32>::new();
        let message = late.require("the cache").unwrap_err().to_string();
        assert!(message.contains("help:"), "{message}");
        assert_eq!(Late::ready(3).require("x").ok(), Some(&3));
    }
}
