//! Context extension: what a plugin adds to the per-request context, and how a
//! handler asks for it.
//!
//! # Why the literal port was rejected
//!
//! The Crystal original extended the framework's base context class with a
//! macro. A middleware called `Brocanth::Context.property(:request_id, String)`
//! and from then on `ctx.request_id` existed — but only if that middleware was
//! actually registered, and the compiler could not tell. Handlers therefore
//! guarded with `ctx.responds_to?(:request_id)`, or read the field and crashed
//! at request time when someone removed the middleware from the pipeline.
//! Every consumer paid for a decision made in the wiring.
//!
//! Porting that literally into Rust would mean `ctx.get("request_id")`
//! returning `Option<&dyn Any>`: the same guard, now with a downcast. The goal
//! was never the guard, it was the extension. So the Rust version keeps the
//! goal and drops the guard.
//!
//! # What replaces it
//!
//! The set of installed context providers is carried in the *type* of the
//! application, as a type-level list. [`Context<E>`] inherits that list.
//! Extraction is bounded by [`Has`], so:
//!
//! * if the provider was installed, `cx.get::<RequestId, _>()` compiles and
//!   cannot fail at run time;
//! * if it was not, the program does not build, and the error names the type
//!   that is missing.
//!
//! Same goal as the macro, enforced instead of guarded.
//!
//! ```
//! use brocanth::plugin_context::{Base, ContextBuilder, ContextItem, With};
//!
//! #[derive(Debug, PartialEq)]
//! struct RequestId(String);
//! impl ContextItem for RequestId {}
//!
//! let mut b = ContextBuilder::new();
//! b.insert(RequestId("abc".into()));
//! let cx = b.build::<With<RequestId, Base>>();
//!
//! assert_eq!(cx.get::<RequestId, _>().0, "abc");
//! let id: &RequestId = cx.extract();
//! assert_eq!(id.0, "abc");
//! ```
//!
//! Ask for something nobody installed and there is nothing to review, because
//! it does not compile:
//!
//! ```compile_fail
//! use brocanth::plugin_context::{Base, ContextBuilder, ContextItem};
//!
//! struct RequestId(String);
//! impl ContextItem for RequestId {}
//!
//! // `Base` is the empty provider list: no plugin installed `RequestId`.
//! let cx = ContextBuilder::new().build::<Base>();
//! let _ = cx.get::<RequestId, _>();
//! ```

use std::any::{type_name, Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;

/// A value a plugin can put into the per-request context.
///
/// One provider owns one type. That is the whole key: there is no string name
/// to misspell.
pub trait ContextItem: Send + Sync + 'static {}

// ---------------------------------------------------------------------------
// The type-level list of installed providers
// ---------------------------------------------------------------------------

/// The empty provider list. An application starts here.
pub struct Base;

/// `T` installed on top of the list `Rest`.
pub struct With<T, Rest>(PhantomData<fn() -> (T, Rest)>);

/// Position marker: the item is at the head of the list.
pub struct Here;

/// Position marker: the item is further down the list.
pub struct There<I>(PhantomData<fn() -> I>);

/// Proof that the provider list `Self` contains `T`.
///
/// `I` is the position of `T` in the list. It exists only so the two impls do
/// not overlap, and it is always inferred — write `_` for it, or use
/// [`Context::extract`], which infers both.
pub trait Has<T, I> {}

impl<T, Rest> Has<T, Here> for With<T, Rest> {}

impl<T, U, Rest, I> Has<U, There<I>> for With<T, Rest> where Rest: Has<U, I> {}

// ---------------------------------------------------------------------------
// Declaring what a plugin provides
// ---------------------------------------------------------------------------

/// What a plugin contributes to the context, at the type level.
///
/// Implemented by [`NoContext`] and [`ProvidesContext<T>`]; a plugin names one
/// of them as its `Plugin::Provides`.
pub trait Provided {
    /// The provider list `E` after this plugin is installed.
    type Install<E>;
}

/// The plugin puts nothing in the context.
pub struct NoContext;

/// The plugin puts a `T` in the context.
pub struct ProvidesContext<T>(PhantomData<fn() -> T>);

impl Provided for NoContext {
    type Install<E> = E;
}

impl<T: ContextItem> Provided for ProvidesContext<T> {
    type Install<E> = With<T, E>;
}

/// The provider list after `P` is registered on top of `E`.
///
/// `AppBuilder` computes this as it goes and never names it. A generated
/// application has to, because its `App` is a written struct and a struct
/// field needs a type: the chain is `After<Last, After<.., Base>>`, in
/// registration order, innermost first.
///
/// Spelled out it is
/// `<<P as Plugin>::Provides as Provided>::Install<E>`, which is correct,
/// unreadable, and long enough that a generator emitting it has to
/// reimplement rustfmt's wrapping to produce a formatted file.
///
/// A plugin that provides nothing folds away: `NoContext::Install<E>` is
/// `E`, so `After<P, E>` is `E`.
pub type After<P, E> = <<P as crate::plugin_api::Plugin>::Provides as Provided>::Install<E>;

// ---------------------------------------------------------------------------
// The context itself
// ---------------------------------------------------------------------------

/// Fills a context. Plugins write here, through `Plugin::provide`.
#[derive(Default)]
pub struct ContextBuilder {
    items: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl ContextBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Installs a value. A second write for the same type replaces the first.
    pub fn insert<T: ContextItem>(&mut self, value: T) {
        self.items.insert(TypeId::of::<T>(), Box::new(value));
    }

    pub fn contains<T: ContextItem>(&self) -> bool {
        self.items.contains_key(&TypeId::of::<T>())
    }

    /// Seals the context and declares its provider list.
    ///
    /// The application calls this with the list it tracked while plugins were
    /// registered, so the list and the contents always agree. Calling it by
    /// hand with a list wider than what was inserted turns a compile error back
    /// into a panic, which is the thing this module exists to avoid.
    pub fn build<E>(self) -> Context<E> {
        Context {
            items: self.items,
            _providers: PhantomData,
        }
    }
}

/// The per-request context of an application whose provider list is `E`.
pub struct Context<E> {
    items: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    _providers: PhantomData<fn() -> E>,
}

impl<E> Context<E> {
    /// The value a provider installed.
    ///
    /// Compiles only when some registered plugin provides `T`.
    pub fn get<T: ContextItem, I>(&self) -> &T
    where
        E: Has<T, I>,
    {
        self.fetch()
    }

    /// Like [`Context::get`], with both type parameters inferred from the
    /// binding: `let id: &RequestId = cx.extract();`.
    pub fn extract<'a, X, I>(&'a self) -> X
    where
        X: FromContext<'a, E, I>,
    {
        X::from_context(self)
    }

    /// Escape hatch for generic code that cannot name the provider list.
    /// Prefer [`Context::get`]: this one can answer `None`.
    pub fn try_get<T: ContextItem>(&self) -> Option<&T> {
        self.items
            .get(&TypeId::of::<T>())
            .and_then(|b| b.downcast_ref::<T>())
    }

    fn fetch<T: ContextItem>(&self) -> &T {
        self.try_get::<T>().unwrap_or_else(|| {
            panic!(
                "context provider for `{}` is in the provider list but wrote nothing",
                type_name::<T>()
            )
        })
    }
}

/// Anything a handler can pull out of a context.
pub trait FromContext<'a, E, I>: Sized {
    fn from_context(cx: &'a Context<E>) -> Self;
}

impl<'a, E, T, I> FromContext<'a, E, I> for &'a T
where
    T: ContextItem,
    E: Has<T, I>,
{
    fn from_context(cx: &'a Context<E>) -> Self {
        cx.fetch::<T>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct RequestId(String);
    impl ContextItem for RequestId {}

    #[derive(Debug, PartialEq)]
    struct CurrentUser(u64);
    impl ContextItem for CurrentUser {}

    #[test]
    fn extraction_reaches_past_the_head_of_the_list() {
        let mut b = ContextBuilder::new();
        b.insert(RequestId("r-1".into()));
        b.insert(CurrentUser(7));
        let cx = b.build::<With<CurrentUser, With<RequestId, Base>>>();

        assert_eq!(cx.get::<RequestId, _>().0, "r-1");
        assert_eq!(cx.get::<CurrentUser, _>().0, 7);

        let user: &CurrentUser = cx.extract();
        assert_eq!(user.0, 7);
    }

    #[test]
    fn try_get_is_the_only_way_to_ask_about_something_not_installed() {
        let cx = ContextBuilder::new().build::<Base>();
        assert!(cx.try_get::<RequestId>().is_none());
    }
}
