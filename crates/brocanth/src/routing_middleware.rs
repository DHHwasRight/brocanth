//! Pattern-scoped middleware.
//!
//! Tower layers attach to a router, so scoping one to `/admin` means
//! building a second router and nesting it. Here a layer attaches to a
//! pattern and an `except` list, and the stack picks the chain for a path
//! before dispatch decides which handler runs.
//!
//! Three rules, all tested:
//!
//! 1. Chains run in **registration order**. The first layer registered is
//!    the outermost.
//! 2. A layer that does not call [`Next::run`] **short-circuits**: no later
//!    layer runs, and neither does the terminal handler.
//! 3. `except` wins over the pattern. A path that matches the pattern and
//!    also matches any `except` entry skips that layer, and only that layer.
//!
//! The stack holds no runtime and spawns nothing.
//!
//! # `O` is not enough for a future
//!
//! An earlier version of this header claimed that because `O` is whatever the
//! application's handler returns, a future-returning handler would work later
//! without changing anything here. **That claim is false.** It is recorded
//! here rather than quietly deleted, because it is the kind of claim that gets
//! planned against.
//!
//! A layer that awaits has to hold [`Next`] across the `.await`, because
//! `next.run(ctx)` is the thing it awaits. So the future borrows the chain and
//! the context, and its type has to name those borrows. But `O` is a **type
//! parameter of the trait**, fixed when the stack is built. Neither borrow
//! exists yet at that point, so `O` cannot name either one, and `rustc` says
//! so:
//!
//! ```text
//! error: lifetime may not live long enough
//!    |
//!    | .use_at("/{*rest}", |ctx: &mut (), next: Next<'_, (), Fut<'f>>| {
//!    |                           -                                   - return type of
//!    |                           |                                     closure is Fut<'2>
//!    |                           let's call the lifetime of this reference `'1`
//!    |     Box::pin(async move { next.run(ctx).await + 1 })
//!    |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ returning this value requires
//!    |                                                     that `'1` must outlive `'2`
//! ```
//!
//! A second, independent blocker sits under it: [`Next`] holds
//! `terminal: &'a dyn Fn(..) -> O`, and `dyn Fn` is not `Sync`, so a future
//! holding a [`Next`] is not `Send` even where every lifetime does line up.
//! Nothing that borrows this chain can cross a task boundary.
//!
//! What fixes both is a trait whose output type is **lifetime-parametric**,
//! which in today's Rust means a method-level `'a` and a concrete return type
//! rather than a trait parameter: an `AsyncLayer` whose `call<'a>` returns
//! `BoxFuture<'a, ..>`. Tower reaches the same place from the other side. Its
//! `Service::Future` is an associated type and a layer *owns* its inner
//! service rather than borrowing a chain, so nothing it awaits is a borrow of
//! the stack.
//!
//! `docs/async-handlers.md` §4 has the full argument, both compiler errors in
//! full, and the `AsyncLayer` shape that does work. It is not built here:
//! nothing mounts a layer yet, so there is no caller to need it.

use crate::routing_path::TemplateError;
use crate::routing_pattern::ScopePattern;

/// One step of a middleware chain.
///
/// Implemented for any `Fn(&mut C, Next<'_, C, O>) -> O`, so a plain closure
/// is a layer.
pub trait Layer<C, O>: Send + Sync {
    /// Run this layer. Call `next.run(ctx)` to continue, or return without
    /// calling it to short-circuit.
    fn call(&self, ctx: &mut C, next: Next<'_, C, O>) -> O;
}

impl<C, O, F> Layer<C, O> for F
where
    F: Fn(&mut C, Next<'_, C, O>) -> O + Send + Sync,
{
    fn call(&self, ctx: &mut C, next: Next<'_, C, O>) -> O {
        self(ctx, next)
    }
}

/// The rest of the chain, handed to a layer.
///
/// Dropping it without calling [`Next::run`] short-circuits the chain.
pub struct Next<'a, C, O> {
    rest: &'a [&'a dyn Layer<C, O>],
    terminal: &'a dyn Fn(&mut C) -> O,
}

impl<'a, C, O> Next<'a, C, O> {
    /// Run the next layer, or the terminal handler when none is left.
    pub fn run(self, ctx: &mut C) -> O {
        match self.rest.split_first() {
            Some((head, tail)) => head.call(
                ctx,
                Next {
                    rest: tail,
                    terminal: self.terminal,
                },
            ),
            None => (self.terminal)(ctx),
        }
    }

    /// How many layers are still ahead, the terminal handler excluded.
    #[must_use]
    pub const fn remaining(&self) -> usize {
        self.rest.len()
    }
}

/// Where a layer applies.
///
/// ```
/// use brocanth::routing::Scope;
///
/// let scope = Scope::new("/admin/{*rest}").except("/admin/login");
/// assert_eq!(scope.pattern(), "/admin/{*rest}");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pattern: String,
    except: Vec<String>,
}

impl Scope {
    /// Apply to every path the pattern covers.
    #[must_use]
    pub fn new(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            except: Vec::new(),
        }
    }

    /// Apply to every path, which is `/{*rest}`.
    #[must_use]
    pub fn all() -> Self {
        Self::new("/{*rest}")
    }

    /// Exclude the paths this pattern covers. Repeatable.
    #[must_use]
    pub fn except(mut self, pattern: impl Into<String>) -> Self {
        self.except.push(pattern.into());
        self
    }

    /// Exclude several patterns at once.
    #[must_use]
    pub fn except_all<I, S>(mut self, patterns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.except.extend(patterns.into_iter().map(Into::into));
        self
    }

    /// The pattern as written.
    #[must_use]
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    /// The exclusion patterns as written.
    #[must_use]
    pub fn exceptions(&self) -> &[String] {
        &self.except
    }

    fn compile(self) -> Result<CompiledScope, TemplateError> {
        let pattern = ScopePattern::parse(&self.pattern)?;
        let except = self
            .except
            .iter()
            .map(|p| ScopePattern::parse(p))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(CompiledScope { pattern, except })
    }
}

#[derive(Debug, Clone)]
struct CompiledScope {
    pattern: ScopePattern,
    except: Vec<ScopePattern>,
}

impl CompiledScope {
    fn applies_to(&self, path: &str) -> bool {
        self.pattern.matches(path) && !self.except.iter().any(|p| p.matches(path))
    }
}

struct Entry<C, O> {
    scope: CompiledScope,
    layer: Box<dyn Layer<C, O>>,
}

/// An ordered set of pattern-scoped layers.
///
/// The stack is a plain value. Build one per application; two applications
/// in one process never see each other's layers.
pub struct MiddlewareStack<C, O> {
    entries: Vec<Entry<C, O>>,
}

impl<C, O> Default for MiddlewareStack<C, O> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C, O> MiddlewareStack<C, O> {
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
        layer: impl Layer<C, O> + 'static,
    ) -> Result<&mut Self, TemplateError> {
        self.use_scoped(Scope::new(pattern), layer)
    }

    /// Register a layer for a scope.
    ///
    /// Order of registration is order of execution.
    ///
    /// # Errors
    ///
    /// Returns [`TemplateError`] if the pattern or any exclusion does not
    /// parse.
    pub fn use_scoped(
        &mut self,
        scope: Scope,
        layer: impl Layer<C, O> + 'static,
    ) -> Result<&mut Self, TemplateError> {
        self.entries.push(Entry {
            scope: scope.compile()?,
            layer: Box::new(layer),
        });
        Ok(self)
    }

    /// The patterns that apply to `path`, in registration order.
    ///
    /// Useful for diagnostics; [`MiddlewareStack::run`] does the dispatch.
    #[must_use]
    pub fn patterns_for(&self, path: &str) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|entry| entry.scope.applies_to(path))
            .map(|entry| entry.scope.pattern.as_str())
            .collect()
    }

    /// Run the chain for `path`, ending at `terminal`.
    ///
    /// When no layer matches, `terminal` is called directly and nothing is
    /// allocated.
    pub fn run<F>(&self, path: &str, ctx: &mut C, terminal: F) -> O
    where
        F: Fn(&mut C) -> O,
    {
        let selected: Vec<&dyn Layer<C, O>> = self
            .entries
            .iter()
            .filter(|entry| entry.scope.applies_to(path))
            .map(|entry| &*entry.layer)
            .collect();

        let terminal: &dyn Fn(&mut C) -> O = &terminal;
        Next {
            rest: &selected,
            terminal,
        }
        .run(ctx)
    }
}

impl<C, O> core::fmt::Debug for MiddlewareStack<C, O> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MiddlewareStack")
            .field(
                "patterns",
                &self
                    .entries
                    .iter()
                    .map(|e| e.scope.pattern.as_str())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A context that records what ran, so order and short-circuiting are
    /// observed rather than inferred from a return value.
    type Trace = Arc<Mutex<Vec<String>>>;

    fn note(trace: &Trace, what: &str) {
        trace.lock().expect("trace").push(what.to_string());
    }

    /// A layer that records its name and continues.
    fn pass(name: &'static str) -> impl Layer<Trace, ()> {
        move |ctx: &mut Trace, next: Next<'_, Trace, ()>| {
            note(ctx, &format!("{name}:in"));
            next.run(ctx);
            note(ctx, &format!("{name}:out"));
        }
    }

    /// A layer that records its name and does not continue.
    fn stop(name: &'static str) -> impl Layer<Trace, ()> {
        move |ctx: &mut Trace, _next: Next<'_, Trace, ()>| {
            note(ctx, &format!("{name}:stop"));
        }
    }

    fn run(stack: &MiddlewareStack<Trace, ()>, path: &str) -> Vec<String> {
        let trace: Trace = Arc::new(Mutex::new(Vec::new()));
        let mut ctx = Arc::clone(&trace);
        stack.run(path, &mut ctx, |c| note(c, "handler"));
        let out = trace.lock().expect("trace").clone();
        out
    }

    #[test]
    fn chains_run_in_registration_order_and_unwind_in_reverse() {
        // Rule 1. The first layer registered is the outermost, so it is
        // entered first and left last -- which is what makes a timing or
        // logging layer able to wrap everything inside it.
        let mut stack = MiddlewareStack::new();
        stack.use_at("/{*rest}", pass("a")).expect("pattern");
        stack.use_at("/{*rest}", pass("b")).expect("pattern");

        assert_eq!(
            run(&stack, "/anything"),
            ["a:in", "b:in", "handler", "b:out", "a:out"]
        );
    }

    #[test]
    fn a_layer_that_does_not_continue_stops_everything_after_it() {
        // Rule 2. Neither the later layers nor the handler run -- which is
        // the whole point of an authentication layer.
        let mut stack = MiddlewareStack::new();
        stack.use_at("/{*rest}", pass("outer")).expect("pattern");
        stack.use_at("/{*rest}", stop("guard")).expect("pattern");
        stack.use_at("/{*rest}", pass("never")).expect("pattern");

        let trace = run(&stack, "/admin");
        assert_eq!(trace, ["outer:in", "guard:stop", "outer:out"]);
        assert!(
            !trace.iter().any(|s| s == "handler"),
            "handler ran: {trace:?}"
        );
        assert!(!trace.iter().any(|s| s.starts_with("never")), "{trace:?}");
    }

    #[test]
    fn except_wins_over_the_pattern_and_only_for_that_layer() {
        // Rule 3. The excluded layer is skipped; every other layer matching
        // the same path still runs. Getting this wrong by skipping the whole
        // chain is the obvious implementation and is silently wrong.
        let mut stack = MiddlewareStack::new();
        stack
            .use_scoped(Scope::all().except("/health"), pass("auth"))
            .expect("pattern");
        stack.use_at("/{*rest}", pass("log")).expect("pattern");

        assert_eq!(
            run(&stack, "/admin"),
            ["auth:in", "log:in", "handler", "log:out", "auth:out"]
        );
        assert_eq!(run(&stack, "/health"), ["log:in", "handler", "log:out"]);
    }

    #[test]
    fn a_path_outside_every_pattern_reaches_the_handler_alone() {
        let mut stack = MiddlewareStack::new();
        stack
            .use_at("/admin/{*rest}", pass("auth"))
            .expect("pattern");
        assert_eq!(run(&stack, "/public"), ["handler"]);
    }

    #[test]
    fn an_empty_stack_still_runs_the_handler() {
        let stack: MiddlewareStack<Trace, ()> = MiddlewareStack::new();
        assert!(stack.is_empty());
        assert_eq!(run(&stack, "/"), ["handler"]);
    }

    #[test]
    fn scopes_select_by_prefix() {
        let mut stack = MiddlewareStack::new();
        stack
            .use_at("/admin/{*rest}", pass("admin"))
            .expect("pattern");
        stack.use_at("/api/{*rest}", pass("api")).expect("pattern");

        assert_eq!(stack.patterns_for("/admin/users"), ["/admin/{*rest}"]);
        assert_eq!(stack.patterns_for("/api/v1"), ["/api/{*rest}"]);
        assert!(stack.patterns_for("/other").is_empty());
    }

    #[test]
    fn a_catch_all_covers_the_bare_prefix_too() {
        // `/admin/{*rest}` must cover `/admin`, or a layer guarding a section
        // leaves its own index page unguarded -- an authorisation hole that
        // looks like a routing detail.
        let mut stack = MiddlewareStack::new();
        stack
            .use_at("/admin/{*rest}", pass("auth"))
            .expect("pattern");
        assert_eq!(stack.patterns_for("/admin"), ["/admin/{*rest}"]);
        assert_eq!(run(&stack, "/admin"), ["auth:in", "handler", "auth:out"]);
    }

    #[test]
    fn several_exclusions_are_all_honoured() {
        let mut stack = MiddlewareStack::new();
        stack
            .use_scoped(
                Scope::all().except_all(["/health", "/metrics"]),
                pass("auth"),
            )
            .expect("pattern");

        assert_eq!(run(&stack, "/health"), ["handler"]);
        assert_eq!(run(&stack, "/metrics"), ["handler"]);
        assert_eq!(run(&stack, "/x"), ["auth:in", "handler", "auth:out"]);
    }

    #[test]
    fn a_layer_can_change_the_context_before_and_after_the_handler() {
        let mut stack = MiddlewareStack::new();
        stack
            .use_at(
                "/{*rest}",
                |ctx: &mut Vec<u8>, next: Next<'_, Vec<u8>, ()>| {
                    ctx.push(1);
                    next.run(ctx);
                    ctx.push(3);
                },
            )
            .expect("pattern");

        let mut ctx: Vec<u8> = Vec::new();
        stack.run("/x", &mut ctx, |c| c.push(2));
        assert_eq!(ctx, [1, 2, 3]);
    }

    #[test]
    fn the_terminal_value_is_returned_through_the_chain() {
        let mut stack: MiddlewareStack<(), u32> = MiddlewareStack::new();
        stack
            .use_at("/{*rest}", |ctx: &mut (), next: Next<'_, (), u32>| {
                next.run(ctx) + 1
            })
            .expect("pattern");
        assert_eq!(stack.run("/x", &mut (), |_| 41), 42);
    }

    #[test]
    fn a_short_circuiting_layer_chooses_the_returned_value() {
        let mut stack: MiddlewareStack<(), u32> = MiddlewareStack::new();
        stack
            .use_at("/{*rest}", |_ctx: &mut (), _next: Next<'_, (), u32>| 403)
            .expect("pattern");
        assert_eq!(stack.run("/x", &mut (), |_| 200), 403);
    }

    #[test]
    fn a_malformed_pattern_is_refused_rather_than_matching_nothing() {
        // Silently never matching is how an authorisation layer goes missing.
        let mut stack: MiddlewareStack<Trace, ()> = MiddlewareStack::new();
        assert!(stack.use_at("/{*rest}/after", pass("x")).is_err());
        assert!(stack.is_empty(), "a refused pattern must not register");
    }

    #[test]
    fn registering_returns_the_stack_so_calls_chain() {
        let mut stack: MiddlewareStack<Trace, ()> = MiddlewareStack::new();
        stack
            .use_at("/{*rest}", pass("a"))
            .expect("pattern")
            .use_at("/{*rest}", pass("b"))
            .expect("pattern");
        assert_eq!(stack.len(), 2);
    }
}
