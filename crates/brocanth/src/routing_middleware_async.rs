//! Pattern-scoped middleware whose layers await.
//!
//! The same stack as [`MiddlewareStack`](crate::routing::MiddlewareStack),
//! with the same three rules, all tested here again:
//!
//! 1. Chains run in **registration order**. The first layer registered is
//!    the outermost.
//! 2. A layer that does not call [`AsyncNext::run`] **short-circuits**: no
//!    later layer runs, and neither does the terminal handler.
//! 3. `except` wins over the pattern, for that layer only.
//!
//! It is a second stack and not a second mode of the first, because the
//! first cannot carry a future: its output type is a trait parameter, fixed
//! before any borrow of the chain exists. `routing_middleware`'s header has
//! the compiler's own words for that. Here the output of a layer is a
//! [`BoxFuture`] whose lifetime is a parameter of the *method*, so it can
//! name the borrows it holds.
//!
//! # Why this is simpler than `docs/async-handlers.md` §2.5
//!
//! The design there hands the context back out of `next`: `run` returned
//! `(O, &mut C)`, so that a layer could touch the context after the chain.
//! That was needed only because `run` took the context for the whole of the
//! chain's lifetime `'a`. [`AsyncNext::run`] takes it for any *shorter*
//! lifetime instead, so a layer reborrows — `next.run(&mut *ctx).await` —
//! and has its context back when the await ends, exactly as the sync stack
//! does.
//!
//! The §2.5 split into `chain_for` and `run_chain` is gone for a similar
//! reason. The selected chain is a `Vec` that must outlive the future
//! running it. [`AsyncMiddlewareStack::run`] is an `async fn`, so the `Vec`
//! is a local of that future and lives exactly as long as it.
//!
//! # A layer is a function or a type
//!
//! A plain closure does not infer the higher-ranked signature a layer needs:
//! its return type is fixed at one lifetime and then fails the bound. A
//! `fn` item states its lifetime, so it is a layer:
//!
//! ```
//! use brocanth::plugin_routes::BoxFuture;
//! use brocanth::routing::{AsyncMiddlewareStack, AsyncNext};
//!
//! fn timed<'a>(ctx: &'a mut Vec<&'static str>, next: AsyncNext<'a, Vec<&'static str>, u16>)
//!     -> BoxFuture<'a, u16>
//! {
//!     Box::pin(async move {
//!         ctx.push("in");
//!         let status = next.run(&mut *ctx).await;
//!         ctx.push("out"); // the context is ours again after the await
//!         status
//!     })
//! }
//!
//! let mut stack = AsyncMiddlewareStack::new();
//! stack.use_at("/{*rest}", timed).expect("pattern");
//! assert_eq!(stack.patterns_for("/x"), ["/{*rest}"]);
//! ```
//!
//! The stack holds no runtime and spawns nothing. It hands back a future and
//! the caller awaits it.

use crate::plugin_routes::BoxFuture;
use crate::routing_middleware::{CompiledScope, Scope};
use crate::routing_path::TemplateError;

/// The end of a chain: the handler, as the stack calls it.
///
/// `Send + Sync` because an [`AsyncNext`] holds a reference to it across an
/// `.await`, and a future holding a reference to something that is not
/// `Sync` is not `Send`.
pub type AsyncTerminal<'t, C, O> = dyn for<'c> Fn(&'c mut C) -> BoxFuture<'c, O> + Send + Sync + 't;

/// One step of an async middleware chain.
///
/// Implemented for any `for<'a> Fn(&'a mut C, AsyncNext<'a, C, O>) ->
/// BoxFuture<'a, O>`, which a `fn` item with a named lifetime is.
pub trait AsyncLayer<C, O>: Send + Sync {
    /// Run this layer. Await `next.run(&mut *ctx)` to continue, or return
    /// without calling it to short-circuit.
    fn call<'a>(&'a self, ctx: &'a mut C, next: AsyncNext<'a, C, O>) -> BoxFuture<'a, O>;
}

impl<C, O, F> AsyncLayer<C, O> for F
where
    F: for<'a> Fn(&'a mut C, AsyncNext<'a, C, O>) -> BoxFuture<'a, O> + Send + Sync,
{
    fn call<'a>(&'a self, ctx: &'a mut C, next: AsyncNext<'a, C, O>) -> BoxFuture<'a, O> {
        self(ctx, next)
    }
}

/// The rest of the chain, handed to a layer.
///
/// Dropping it without calling [`AsyncNext::run`] short-circuits the chain.
pub struct AsyncNext<'a, C, O> {
    rest: &'a [&'a dyn AsyncLayer<C, O>],
    terminal: &'a AsyncTerminal<'a, C, O>,
}

impl<'a, C, O> AsyncNext<'a, C, O> {
    /// Run the next layer, or the terminal handler when none is left.
    ///
    /// Takes the context for `'b`, which may be shorter than the chain.
    /// That is what lets a layer write `next.run(&mut *ctx).await` and use
    /// `ctx` again afterwards.
    pub fn run<'b>(self, ctx: &'b mut C) -> BoxFuture<'b, O>
    where
        'a: 'b,
    {
        // Shortened to `'b` first: every borrow the next layer holds must
        // end when this call's future does.
        let this: AsyncNext<'b, C, O> = self;
        match this.rest.split_first() {
            Some((head, tail)) => head.call(
                ctx,
                AsyncNext {
                    rest: tail,
                    terminal: this.terminal,
                },
            ),
            None => (this.terminal)(ctx),
        }
    }

    /// How many layers are still ahead, the terminal handler excluded.
    #[must_use]
    pub const fn remaining(&self) -> usize {
        self.rest.len()
    }
}

struct Entry<C, O> {
    scope: CompiledScope,
    layer: Box<dyn AsyncLayer<C, O>>,
}

/// An ordered set of pattern-scoped async layers.
///
/// A plain value, like [`MiddlewareStack`](crate::routing::MiddlewareStack):
/// build one per application.
pub struct AsyncMiddlewareStack<C, O> {
    entries: Vec<Entry<C, O>>,
}

impl<C, O> Default for AsyncMiddlewareStack<C, O> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C, O> AsyncMiddlewareStack<C, O> {
    /// An empty stack.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// How many layers are registered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no layer is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Register a layer for a pattern, with no exclusions.
    ///
    /// # Errors
    ///
    /// Returns [`TemplateError`] if the pattern does not parse.
    pub fn use_at(
        &mut self,
        pattern: &str,
        layer: impl AsyncLayer<C, O> + 'static,
    ) -> Result<&mut Self, TemplateError> {
        self.use_scoped(Scope::new(pattern), layer)
    }

    /// Register a layer for a scope. Order of registration is order of
    /// execution.
    ///
    /// # Errors
    ///
    /// Returns [`TemplateError`] if the pattern or any exclusion does not
    /// parse.
    pub fn use_scoped(
        &mut self,
        scope: Scope,
        layer: impl AsyncLayer<C, O> + 'static,
    ) -> Result<&mut Self, TemplateError> {
        self.entries.push(Entry {
            scope: scope.compile()?,
            layer: Box::new(layer),
        });
        Ok(self)
    }

    /// The patterns that apply to `path`, in registration order.
    #[must_use]
    pub fn patterns_for(&self, path: &str) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|entry| entry.scope.applies_to(path))
            .map(|entry| entry.scope.pattern())
            .collect()
    }

    /// Run the chain for `path`, ending at `terminal`.
    ///
    /// The chain is chosen on the first poll and kept in this future's own
    /// state, so nothing it borrows can outlive it. When no layer matches,
    /// the chain is an empty `Vec`, which allocates nothing, and `terminal`
    /// runs directly.
    pub async fn run<F>(&self, path: &str, ctx: &mut C, terminal: F) -> O
    where
        F: for<'c> Fn(&'c mut C) -> BoxFuture<'c, O> + Send + Sync,
    {
        let selected: Vec<&dyn AsyncLayer<C, O>> = self
            .entries
            .iter()
            .filter(|entry| entry.scope.applies_to(path))
            .map(|entry| &*entry.layer)
            .collect();
        AsyncNext {
            rest: &selected,
            terminal: &terminal,
        }
        .run(ctx)
        .await
    }
}

impl<C, O> core::fmt::Debug for AsyncMiddlewareStack<C, O> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AsyncMiddlewareStack")
            .field(
                "patterns",
                &self
                    .entries
                    .iter()
                    .map(|e| e.scope.pattern())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};

    /// Polls a future to the end on this thread. The core has no runtime,
    /// and a test that brought one would be testing the wrong thing.
    fn block_on<F: Future>(future: F) -> F::Output {
        struct Unpark(std::thread::Thread);
        impl Wake for Unpark {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
        let mut cx = Context::from_waker(&waker);
        let mut future = Box::pin(future);
        loop {
            match future.as_mut().poll(&mut cx) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::park(),
            }
        }
    }

    /// A future that is pending once, so every layer below really crosses
    /// an `.await` with the context and the chain held.
    struct YieldOnce(bool);
    impl Future for YieldOnce {
        type Output = ();
        fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            if self.0 {
                Poll::Ready(())
            } else {
                self.0 = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }

    type Trace = Vec<String>;

    /// A layer that records its name on the way in and out, and continues.
    struct Pass(&'static str);
    impl AsyncLayer<Trace, ()> for Pass {
        fn call<'a>(
            &'a self,
            ctx: &'a mut Trace,
            next: AsyncNext<'a, Trace, ()>,
        ) -> BoxFuture<'a, ()> {
            Box::pin(async move {
                ctx.push(format!("{}:in", self.0));
                YieldOnce(false).await;
                next.run(&mut *ctx).await;
                ctx.push(format!("{}:out", self.0));
            })
        }
    }

    /// A layer that records its name and does not continue.
    struct Stop(&'static str);
    impl AsyncLayer<Trace, ()> for Stop {
        fn call<'a>(
            &'a self,
            ctx: &'a mut Trace,
            _next: AsyncNext<'a, Trace, ()>,
        ) -> BoxFuture<'a, ()> {
            Box::pin(async move {
                YieldOnce(false).await;
                ctx.push(format!("{}:stop", self.0));
            })
        }
    }

    fn handler(ctx: &mut Trace) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            YieldOnce(false).await;
            ctx.push("handler".to_string());
        })
    }

    fn run(stack: &AsyncMiddlewareStack<Trace, ()>, path: &str) -> Trace {
        let mut trace = Trace::new();
        block_on(stack.run(path, &mut trace, handler));
        trace
    }

    #[test]
    fn chains_run_in_registration_order_and_unwind_in_reverse() {
        // Rule 1, across awaits: the first layer registered is entered
        // first and left last, so a timing layer wraps everything inside.
        let mut stack = AsyncMiddlewareStack::new();
        stack.use_at("/{*rest}", Pass("a")).expect("pattern");
        stack.use_at("/{*rest}", Pass("b")).expect("pattern");

        assert_eq!(
            run(&stack, "/anything"),
            ["a:in", "b:in", "handler", "b:out", "a:out"]
        );
    }

    #[test]
    fn a_layer_that_does_not_continue_stops_everything_after_it() {
        // Rule 2. Dropping `next` is the whole of short-circuiting.
        let mut stack = AsyncMiddlewareStack::new();
        stack.use_at("/{*rest}", Pass("outer")).expect("pattern");
        stack.use_at("/{*rest}", Stop("guard")).expect("pattern");
        stack.use_at("/{*rest}", Pass("never")).expect("pattern");

        assert_eq!(
            run(&stack, "/admin"),
            ["outer:in", "guard:stop", "outer:out"]
        );
    }

    #[test]
    fn except_wins_over_the_pattern_and_only_for_that_layer() {
        // Rule 3. Skipping the whole chain is the obvious wrong answer.
        let mut stack = AsyncMiddlewareStack::new();
        stack
            .use_scoped(Scope::all().except("/health"), Pass("auth"))
            .expect("pattern");
        stack.use_at("/{*rest}", Pass("log")).expect("pattern");

        assert_eq!(
            run(&stack, "/admin"),
            ["auth:in", "log:in", "handler", "log:out", "auth:out"]
        );
        assert_eq!(run(&stack, "/health"), ["log:in", "handler", "log:out"]);
    }

    #[test]
    fn a_path_outside_every_pattern_reaches_the_handler_alone() {
        let mut stack = AsyncMiddlewareStack::new();
        stack
            .use_at("/admin/{*rest}", Pass("auth"))
            .expect("pattern");
        assert_eq!(run(&stack, "/public"), ["handler"]);
        assert_eq!(stack.patterns_for("/admin"), ["/admin/{*rest}"]);
    }

    #[test]
    fn an_empty_stack_still_runs_the_handler() {
        let stack: AsyncMiddlewareStack<Trace, ()> = AsyncMiddlewareStack::new();
        assert!(stack.is_empty());
        assert_eq!(run(&stack, "/"), ["handler"]);
    }

    /// A layer as a plain function: the form a generated project writes.
    fn plus_one<'a>(ctx: &'a mut Vec<u8>, next: AsyncNext<'a, Vec<u8>, u32>) -> BoxFuture<'a, u32> {
        Box::pin(async move {
            ctx.push(1);
            let out = next.run(&mut *ctx).await;
            ctx.push(3);
            out + 1
        })
    }

    #[test]
    fn a_function_layer_changes_the_context_before_and_after_the_handler() {
        // The reason a layer exists: it writes after `next`, which the
        // §2.5 design needed a context handback for.
        let mut stack = AsyncMiddlewareStack::new();
        stack.use_at("/{*rest}", plus_one).expect("pattern");

        let mut ctx = Vec::new();
        let out = block_on(
            stack.run("/x", &mut ctx, |c: &mut Vec<u8>| -> BoxFuture<'_, u32> {
                Box::pin(async move {
                    c.push(2);
                    41
                })
            }),
        );
        assert_eq!(out, 42);
        assert_eq!(ctx, [1, 2, 3]);
    }

    fn forbidden<'a>(_ctx: &'a mut (), _next: AsyncNext<'a, (), u32>) -> BoxFuture<'a, u32> {
        Box::pin(async { 403 })
    }

    #[test]
    fn a_short_circuiting_layer_chooses_the_returned_value() {
        let mut stack = AsyncMiddlewareStack::new();
        stack.use_at("/{*rest}", forbidden).expect("pattern");
        let out = block_on(
            stack.run("/x", &mut (), |_: &mut ()| -> BoxFuture<'_, u32> {
                Box::pin(async { 200 })
            }),
        );
        assert_eq!(out, 403);
    }

    #[test]
    fn a_malformed_pattern_is_refused_rather_than_matching_nothing() {
        let mut stack: AsyncMiddlewareStack<Trace, ()> = AsyncMiddlewareStack::new();
        assert!(stack.use_at("/{*rest}/after", Pass("x")).is_err());
        assert!(stack.is_empty(), "a refused pattern must not register");
    }

    #[test]
    fn the_chain_future_crosses_threads() {
        // `Send` is the bound a server puts on a request future. A chain
        // that held a non-`Sync` reference across an await would fail
        // this at compile time, not at the first request.
        fn is_send<T: Send>(_: &T) {}
        let mut stack = AsyncMiddlewareStack::new();
        stack.use_at("/{*rest}", Pass("a")).expect("pattern");
        let mut trace = Trace::new();
        let future = stack.run("/x", &mut trace, handler);
        is_send(&future);
        block_on(future);
        assert_eq!(trace, ["a:in", "handler", "a:out"]);
    }
}
