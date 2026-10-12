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
//! The stack holds no runtime and spawns nothing. `O` is whatever the
//! application's handler returns, so a future-returning handler works later
//! without changing anything here.

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
