//! The model layer's end of `brocanth::plugin_model`.
//!
//! Behind the `plugin` feature, which is the only thing in this crate that
//! names the core. An application that stores rows and runs no core pays
//! neither the dependency nor the code.
//!
//! The core cannot name a connection, an entity or a column -- PLAN.md §8.4,
//! *"the ORM is recommended and absent"* -- so the ORM declares the event kind
//! and the core only keys on it. [`RowWritten`] is this crate's kind, and its
//! payload is a [`RowChange`]: the row that was written, named but not read.
//!
//! The arrow still points from here to the core. This module names
//! `brocanth::plugin_model`; nothing in the core names this crate.
//!
//! # Why a sink rather than a second write path
//!
//! [`crate::changes`] already reports every write to a [`ChangeSink`], and
//! `brocanth-realtime`'s broker is one. A model hook is another reader of the
//! same change, so it arrives the same way: [`HookSink`] is a sink that hands
//! the change to the application's hooks.
//!
//! That keeps one publish point in the write path, and it keeps the cost
//! argument intact. [`ChangeSink::wants`] is asked before a change is built,
//! and [`HookSink::wants`] is `H::MODEL`, a constant -- so an application
//! whose plugins installed no model hook answers `false` at compile time and
//! the write path builds nothing. It is the same
//! [`HookFamilies::MODEL`] that
//! already compiles `ModelDispatch::emit` down to `Ok(())`, doing the same job
//! one step earlier.
//!
//! # Installing it
//!
//! ```ignore
//! use brocanth_events::FanOut;
//! use brocanth_model::plugin::HookSink;
//!
//! // One sink for the write path, two readers behind it.
//! let sink = FanOut::new()
//!     .with(broker)
//!     .with(HookSink::new(app.model_dispatch()));
//!
//! post.insert_published(db, &sink).await?;
//! ```

use brocanth::plugin_hooks::HookFamilies;
use brocanth::plugin_model::{ModelDispatch, ModelEvent};
use brocanth_events::{ChangeSink, RowChange};

/// The event kind: a row was written.
///
/// A `'static` marker, because it is the key the core files hooks under. What
/// it carries is [`RowChange`], which borrows -- the table name and the
/// changed column names belong to the write that produced them, and a hook
/// that wants to keep one calls [`RowChange::into_owned`].
///
/// One kind for all three of create, update and delete, because
/// [`brocanth_events::ChangeKind`] already says which and a hook that cares
/// matches on it. Three kinds would make every hook register three times to
/// answer one question.
pub struct RowWritten;

impl ModelEvent for RowWritten {
    type Payload<'a> = RowChange<'a>;
}

/// A [`ChangeSink`] that hands every change to the application's model hooks.
///
/// Built from `App::model_dispatch`, which is an `Arc` bump to clone and
/// carries the application's hook-family record `H`.
pub struct HookSink<H> {
    dispatch: ModelDispatch<H>,
}

impl<H> HookSink<H> {
    /// Report changes to this application's model hooks.
    pub fn new(dispatch: ModelDispatch<H>) -> HookSink<H> {
        HookSink { dispatch }
    }

    /// The dispatch handle behind the sink.
    pub fn dispatch(&self) -> &ModelDispatch<H> {
        &self.dispatch
    }
}

impl<H: HookFamilies> ChangeSink for HookSink<H> {
    fn publish(&self, change: &RowChange<'_>) {
        // `emit` takes `&mut`, so a hook may correct the change on its way
        // past. Cloning a borrowed change copies three borrows and allocates
        // nothing.
        let mut event = change.clone();
        // A hook that refuses stops the chain and goes no further. The row is
        // already written and there is nobody left to apologise to, which is
        // the same reason `ChangeSink::publish` returns nothing.
        let _ = self.dispatch.emit::<RowWritten>(&mut event);
    }

    /// `H::MODEL` first, which is a constant: an application that installed no
    /// model hook answers `false` without reading anything, and the write path
    /// never builds the change. Then the hook count for this kind, so an
    /// application that hooks some *other* model event pays nothing here
    /// either.
    fn wants(&self, _collection: &str) -> bool {
        H::MODEL && self.dispatch.hooks().len_for::<RowWritten>() > 0
    }
}

impl<H> std::fmt::Debug for HookSink<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HookSink")
            .field("dispatch", &self.dispatch)
            .finish()
    }
}
