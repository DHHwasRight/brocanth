//! The model seam: where an ORM registers hooks into a core that has never
//! heard of a database.
//!
//! PLAN.md §8.4 — *"The ORM is recommended and absent."* So the core cannot
//! name a connection, a transaction, an entity or a column, and a seam shaped
//! like `before_save(&Model, &Connection)` is out of the question before it is
//! written.
//!
//! What the core *can* do is carry an event it does not understand to the
//! hooks that do. The ORM declares an event **kind** — a `'static` marker type
//! such as `BeforeSave` — and says, through [`ModelEvent::Payload`], what the
//! event actually looks like at a given lifetime. The payload is free to
//! borrow, which every real `before_save` does: it holds the model being
//! written and the connection to write it with. The kind is what the core
//! keys on; the payload is what the core never looks inside.
//!
//! Any plugin can then register a [`ModelHook`] for a kind, and the ORM emits
//! the payload from inside its own write path. The core stores the hooks,
//! sorts them with everything else ([`plugin_order`](crate::plugin_order)),
//! and hands them back when the kind comes past.
//!
//! So the dependency arrow still points from the ORM to the core: the ORM
//! names the core's [`ModelEvent`], and the core names nothing of the ORM's.
//! A different ORM is a different set of event types against the same seam,
//! and an application that stores nothing registers no model hook and pays
//! [`ModelDispatch::emit`] exactly nothing — see
//! [`crate::plugin_hooks::HookFamilies::MODEL`].
//!
//! # From the ORM's side
//!
//! ```
//! use brocanth::plugin_api::PluginError;
//! use brocanth::plugin_model::{ModelEvent, ModelHook, ModelHooks};
//! use brocanth::plugin_order::{HookId, Order};
//!
//! // The ORM's event kind, and the payload it carries. The payload borrows,
//! // because a real `before_save` holds the model and the connection.
//! struct BeforeSave;
//! struct Writing<'a> {
//!     table: &'static str,
//!     audit: &'a mut Vec<&'static str>,
//! }
//! impl ModelEvent for BeforeSave {
//!     type Payload<'a> = Writing<'a>;
//! }
//!
//! // Somebody else's hook on it.
//! struct Audit;
//! impl ModelHook<BeforeSave> for Audit {
//!     fn on_event(&self, event: &mut Writing<'_>) -> Result<(), PluginError> {
//!         event.audit.push(event.table);
//!         Ok(())
//!     }
//! }
//!
//! let mut hooks = ModelHooks::new();
//! hooks.register::<BeforeSave>(HookId::new("audit", "trail"), Order::new(), Audit);
//! let hooks = hooks.finish().unwrap();
//!
//! let mut audit = Vec::new();
//! hooks.emit::<BeforeSave>(&mut Writing { table: "posts", audit: &mut audit }).unwrap();
//! assert_eq!(audit, vec!["posts"]);
//! ```

use std::any::{type_name, Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use crate::plugin_api::PluginError;
use crate::plugin_hooks::HookFamilies;
use crate::plugin_order::{sort, HookId, Order, OrderError, Ordered};

/// A kind of thing that happens to a model, named by whoever owns the model
/// layer.
///
/// The kind itself is a `'static` marker — `struct BeforeSave;` — because it
/// is the key the core stores hooks under, and a key has to outlive the
/// request. The event *payload* is [`ModelEvent::Payload`], which is free to
/// borrow: a real `before_save` carries the model being written and the
/// connection to write it with, and neither of those is `'static`.
///
/// Splitting the two is what lets the core hold hooks for an event it cannot
/// name, without forcing the ORM to own or clone the row.
pub trait ModelEvent: 'static {
    /// What the event carries, at lifetime `'a`.
    type Payload<'a>;
}

/// A hook on one kind of model event.
pub trait ModelHook<Ev: ModelEvent>: Send + Sync + 'static {
    /// Returning `Err` stops the chain: no later hook for this event runs, and
    /// the ORM gets the error back to abort the write with.
    fn on_event(&self, event: &mut Ev::Payload<'_>) -> Result<(), PluginError>;
}

/// Every model hook of one application, keyed by the event type.
///
/// A plain value hanging off the [`App`](crate::plugin_app::App). Two
/// applications in one process have two of these; there is no registry to
/// reset.
#[derive(Default)]
pub struct ModelHooks {
    /// Each bucket holds `Box<dyn ModelHook<Ev>>` for one `Ev`, boxed again
    /// as `Any` because the core cannot name `Ev`.
    pending: HashMap<TypeId, Vec<Ordered<Box<dyn Any + Send + Sync>>>>,
    sorted: HashMap<TypeId, Vec<Box<dyn Any + Send + Sync>>>,
    /// Event type ids to the name of the type, for error messages.
    names: HashMap<TypeId, &'static str>,
}

impl ModelHooks {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a hook for `Ev`. Order is resolved later, by [`ModelHooks::finish`].
    pub fn register<Ev: ModelEvent>(&mut self, id: HookId, order: Order, hook: impl ModelHook<Ev>) {
        let key = TypeId::of::<Ev>();
        self.names.insert(key, type_name::<Ev>());
        let boxed: Box<dyn ModelHook<Ev>> = Box::new(hook);
        self.pending.entry(key).or_default().push(Ordered::new(
            id,
            order,
            Box::new(boxed) as Box<dyn Any + Send + Sync>,
        ));
    }

    /// Sorts each event's hooks into their declared order.
    ///
    /// # Errors
    ///
    /// [`OrderError`] from the event whose declarations do not resolve. Each
    /// event sorts on its own, so "after the session hook" means the session
    /// hook *for this event*.
    pub fn finish(mut self) -> Result<Self, OrderError> {
        let pending = std::mem::take(&mut self.pending);
        for (key, bucket) in pending {
            self.sorted.insert(key, sort(bucket)?);
        }
        Ok(self)
    }

    /// Whether any hook is registered at all.
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty() && self.sorted.is_empty()
    }

    /// How many hooks are registered for `Ev`.
    pub fn len_for<Ev: ModelEvent>(&self) -> usize {
        self.sorted
            .get(&TypeId::of::<Ev>())
            .map(Vec::len)
            .unwrap_or(0)
    }

    /// Runs every hook registered for `Ev`, in declared order.
    ///
    /// The first hook that fails stops the chain and the error goes back to
    /// the caller, which is the ORM, which aborts the write. Hooks that have
    /// already run have already run: the core does not pretend to undo them,
    /// because only the ORM's transaction can.
    ///
    /// # Errors
    ///
    /// Whatever the first failing hook returned.
    pub fn emit<Ev: ModelEvent>(&self, event: &mut Ev::Payload<'_>) -> Result<(), PluginError> {
        let Some(bucket) = self.sorted.get(&TypeId::of::<Ev>()) else {
            return Ok(());
        };
        for entry in bucket {
            let hook = entry
                .downcast_ref::<Box<dyn ModelHook<Ev>>>()
                .expect("a bucket holds one event type, and the key is that type");
            hook.on_event(event)?;
        }
        Ok(())
    }
}

impl std::fmt::Debug for ModelHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut events: Vec<&str> = self
            .names
            .iter()
            .filter(|(k, _)| self.sorted.contains_key(k) || self.pending.contains_key(k))
            .map(|(_, n)| *n)
            .collect();
        events.sort_unstable();
        f.debug_struct("ModelHooks")
            .field("events", &events)
            .finish()
    }
}

/// A cheap handle an ORM keeps, so a write path deep inside the ORM can emit
/// without holding the application.
///
/// Cloning is an `Arc` bump. `H` is the application's hook-family record, so
/// an application with no model hook compiles [`ModelDispatch::emit`] down to
/// `Ok(())` and the ORM's write path pays nothing for a seam it is not using.
pub struct ModelDispatch<H> {
    hooks: Arc<ModelHooks>,
    _families: PhantomData<fn() -> H>,
}

impl<H> ModelDispatch<H> {
    pub(crate) fn new(hooks: Arc<ModelHooks>) -> Self {
        Self {
            hooks,
            _families: PhantomData,
        }
    }

    /// The hooks behind the handle, for a caller that wants to look.
    pub fn hooks(&self) -> &ModelHooks {
        &self.hooks
    }
}

impl<H: HookFamilies> ModelDispatch<H> {
    /// Emits `event` to every hook registered for it.
    #[inline]
    pub fn emit<Ev: ModelEvent>(&self, event: &mut Ev::Payload<'_>) -> Result<(), PluginError> {
        if !H::MODEL {
            return Ok(());
        }
        self.hooks.emit::<Ev>(event)
    }
}

impl<H> Clone for ModelDispatch<H> {
    fn clone(&self) -> Self {
        Self {
            hooks: Arc::clone(&self.hooks),
            _families: PhantomData,
        }
    }
}

impl<H> std::fmt::Debug for ModelDispatch<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelDispatch")
            .field("hooks", &*self.hooks)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct BeforeSave;
    struct Writing<'a> {
        log: &'a Mutex<Vec<&'static str>>,
        abort: bool,
    }
    impl ModelEvent for BeforeSave {
        type Payload<'a> = Writing<'a>;
    }

    struct AfterSave;
    impl ModelEvent for AfterSave {
        type Payload<'a> = Mutex<Vec<&'static str>>;
    }

    struct Note(&'static str);
    impl ModelHook<BeforeSave> for Note {
        fn on_event(&self, event: &mut Writing<'_>) -> Result<(), PluginError> {
            event.log.lock().unwrap().push(self.0);
            if event.abort {
                return Err(PluginError::new(format!("{} refused", self.0)));
            }
            Ok(())
        }
    }

    struct Counter(&'static str);
    impl ModelHook<AfterSave> for Counter {
        fn on_event(&self, event: &mut Mutex<Vec<&'static str>>) -> Result<(), PluginError> {
            event.get_mut().unwrap().push(self.0);
            Ok(())
        }
    }

    #[test]
    fn hooks_fire_in_declared_order_not_registration_order() {
        let mut hooks = ModelHooks::new();
        hooks.register::<BeforeSave>(
            HookId::new("audit", "trail"),
            Order::new().after("validation"),
            Note("audit"),
        );
        hooks.register::<BeforeSave>(
            HookId::new("validation", "check"),
            Order::new(),
            Note("validation"),
        );
        let hooks = hooks.finish().unwrap();

        let log = Mutex::new(Vec::new());
        hooks
            .emit::<BeforeSave>(&mut Writing {
                log: &log,
                abort: false,
            })
            .unwrap();
        assert_eq!(*log.lock().unwrap(), vec!["validation", "audit"]);
    }

    #[test]
    fn a_failing_hook_stops_the_chain_and_nothing_else() {
        let mut hooks = ModelHooks::new();
        hooks.register::<BeforeSave>(HookId::new("a", "h"), Order::new(), Note("a"));
        hooks.register::<BeforeSave>(HookId::new("b", "h"), Order::new(), Note("b"));
        hooks.register::<AfterSave>(HookId::new("c", "h"), Order::new(), Counter("c"));
        let hooks = hooks.finish().unwrap();

        let log = Mutex::new(Vec::new());
        let err = hooks
            .emit::<BeforeSave>(&mut Writing {
                log: &log,
                abort: true,
            })
            .unwrap_err();
        assert_eq!(err.to_string(), "a refused");
        // `b` never ran: the chain stopped.
        assert_eq!(*log.lock().unwrap(), vec!["a"]);

        // And the other kind is untouched, because a bucket is per kind.
        let mut after = Mutex::new(Vec::new());
        hooks.emit::<AfterSave>(&mut after).unwrap();
        assert_eq!(*after.lock().unwrap(), vec!["c"]);
    }

    #[test]
    fn an_event_nobody_hooked_is_free_and_silent() {
        let hooks = ModelHooks::new().finish().unwrap();
        let mut after = Mutex::new(Vec::new());
        assert!(hooks.emit::<AfterSave>(&mut after).is_ok());
        assert_eq!(hooks.len_for::<AfterSave>(), 0);
    }

    #[test]
    fn the_dispatch_handle_with_no_model_family_never_reaches_the_map() {
        use crate::plugin_hooks::NoHooks;
        let mut hooks = ModelHooks::new();
        hooks.register::<AfterSave>(HookId::new("a", "h"), Order::new(), Counter("a"));
        let dispatch: ModelDispatch<NoHooks> =
            ModelDispatch::new(Arc::new(hooks.finish().unwrap()));

        let mut after = Mutex::new(Vec::new());
        dispatch.emit::<AfterSave>(&mut after).unwrap();

        // The hook is registered, and `NoHooks::MODEL` is false, so `emit` is
        // a constant `Ok(())` and the hook did not run.
        assert_eq!(dispatch.hooks().len_for::<AfterSave>(), 1);
        assert!(after.lock().unwrap().is_empty());
    }
}
