//! The routing core: a route table, seven verbs, borrowed parameters,
//! file-path conversion, typed helpers, and pattern-scoped middleware.
//!
//! Everything hangs off a value. There is no registry, no `reset!`, and no
//! class-level hash, so two applications in one process cannot collide and
//! the tests below need no teardown.
//!
//! # The matcher
//!
//! brocanth does not write a radix trie. It writes a thin table over one,
//! and the choice of engine was settled by a measurement rather than a
//! preference.
//!
//! The file convention produces `/docs/{slug}` and `/docs/{*rest}` as
//! siblings whenever a directory holds both `[slug].rs` and `[...rest].rs`,
//! which is the ordinary way to write "a page per document, plus a
//! fallback". `matchit` 0.9.2 rejects that pair:
//!
//! ```text
//! insert /docs/{slug}  -> Ok(())
//! insert /docs/{*rest} -> Err(Conflict { with: "/docs/{slug}" })
//! // and in the other order
//! insert /docs/{*rest} -> Ok(())
//! insert /docs/{slug}  -> Err(Conflict { with: "/docs/{*rest}" })
//! ```
//!
//! Two ways out. Hold two `matchit` routers, the specific one first, and
//! fall back to the catch-all one. Or use `wayfind` 1.1, which accepts the
//! pair and reports which template matched:
//!
//! ```text
//! insert /docs/<slug>  -> Ok(())
//! insert /docs/<*rest> -> Ok(())
//! search "/docs/a"     -> ("/docs/<slug>",  [("slug", "a")])
//! search "/docs/a/b"   -> ("/docs/<*rest>", [("rest", "a/b")])
//! ```
//!
//! brocanth uses `wayfind`, for three reasons that the two-router split
//! does not give:
//!
//! 1. **405 needs one matcher.** A route must answer "this path exists,
//!    that verb does not", so the trie is keyed on path alone and the verb
//!    table hangs off the matched route. Splitting the trie in two to dodge
//!    a conflict puts the same path in two places and makes the `Allow`
//!    set a merge instead of a lookup.
//! 2. **Precedence stops being ours to get wrong.** `wayfind` orders
//!    static over dynamic over wildcard inside the trie. The two-router
//!    split reimplements that ordering in application code, where it is
//!    untested by the engine's own suite.
//! 3. **The matched template comes back.** `matchit` returns the value and
//!    the parameters, not the template, so the route identity has to be
//!    stored in the value and kept in step by hand.
//!
//! The engine's syntax (`<name>`, `<*name>`) stays an implementation
//! detail. brocanth templates are written `{name}` and `{*name}`, which is
//! what the file convention produces and what the rest of the ecosystem
//! reads, and [`RouteTemplate`] translates at build time. Swapping engines
//! later changes this module and nothing a user wrote.
//!
//! # No parameter ceiling
//!
//! The Crystal prototype's hand-rolled router had `MAX_PARAMS = 4` and
//! dropped the fifth with no warning, because its collector could not prove
//! that a captured parameter does not outlive the request. [`Params`]
//! borrows from the path, so the question does not arise and there is no
//! number to exceed. See `five_parameters_all_survive` below.
//!
//! # Example
//!
//! ```
//! use brocanth::routing::{Dispatch, Router, Verb};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut builder = Router::builder();
//! builder.route("/users/{id}", Verb::Get, "show")?;
//! builder.route("/users/{id}", Verb::Delete, "destroy")?;
//! let router = builder.build()?;
//!
//! match router.dispatch(Verb::Post, "/users/42") {
//!     Dispatch::MethodNotAllowed { allow, matched } => {
//!         assert_eq!(matched.template(), "/users/{id}");
//!         assert_eq!(allow.to_string(), "GET, HEAD, DELETE");
//!     }
//!     other => panic!("expected 405, got {other:?}"),
//! }
//! # Ok(())
//! # }
//! ```

use std::collections::HashMap;

pub use crate::routing_codegen::{generate_helpers, CodegenError, RouteHelper};
pub use crate::routing_middleware::{Layer, MiddlewareStack, Next, Scope};
pub use crate::routing_params::{ParamError, ParamErrorKind, Params};
pub use crate::routing_path::{
    encode_catch_all, encode_segment, module_path_for_file, route_for_file, ParamKind, ParamSpec,
    PathError, RouteTemplate, TemplateError,
};
pub use crate::routing_pattern::ScopePattern;
pub use crate::routing_verb::{Allow, Verb};

/// One route: a template, and a handler slot per verb.
struct Route<H> {
    template: RouteTemplate,
    handlers: [Option<H>; 7],
}

impl<H> Route<H> {
    fn new(template: RouteTemplate) -> Self {
        Self {
            template,
            handlers: std::array::from_fn(|_| None),
        }
    }

    /// Verbs with a handler, plus HEAD whenever GET has one.
    fn allow(&self) -> Allow {
        let mut allow = Allow::empty();
        for verb in Verb::ALL {
            if self.handlers[verb.index()].is_some() {
                allow = allow.with(verb);
            }
        }
        if allow.contains(Verb::Get) {
            allow = allow.with(Verb::Head);
        }
        allow
    }
}

/// A route table.
///
/// Immutable once built, so it is `Send + Sync` whenever `H` is and can be
/// shared across threads without a lock.
pub struct Router<H> {
    engine: wayfind::Router<Route<H>>,
    routes: Vec<(RouteTemplate, Allow)>,
}

impl<H> Router<H> {
    /// Start building a table.
    #[must_use]
    pub fn builder() -> RouterBuilder<H> {
        RouterBuilder::new()
    }

    /// Match a path, ignoring the verb.
    ///
    /// Returns [`None`] only when no route has that shape. A path that
    /// exists but answers other verbs still matches; ask the result which
    /// verbs it allows.
    #[must_use]
    pub fn at<'r, 'p>(&'r self, path: &'p str) -> Option<RouteMatch<'r, 'p, H>> {
        self.engine.search(path).map(|inner| RouteMatch { inner })
    }

    /// Match a path and a verb.
    ///
    /// A path that matches with a verb that does not produces
    /// [`Dispatch::MethodNotAllowed`], never [`Dispatch::NotFound`].
    #[must_use]
    pub fn dispatch<'r, 'p>(&'r self, verb: Verb, path: &'p str) -> Dispatch<'r, 'p, H> {
        let Some(matched) = self.at(path) else {
            return Dispatch::NotFound;
        };
        match matched.handler(verb) {
            Some(handler) => Dispatch::Found { handler, matched },
            None => Dispatch::MethodNotAllowed {
                allow: matched.allow(),
                matched,
            },
        }
    }

    /// The verbs a path answers, or [`None`] when the path has no route.
    #[must_use]
    pub fn allow_for(&self, path: &str) -> Option<Allow> {
        self.at(path).map(|m| m.allow())
    }

    /// Every registered route, in registration order.
    #[must_use]
    pub fn routes(&self) -> &[(RouteTemplate, Allow)] {
        &self.routes
    }

    /// How many distinct paths are registered. Verbs on one path count
    /// once.
    #[must_use]
    pub fn len(&self) -> usize {
        self.routes.len()
    }

    /// Whether no route is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }
}

impl<H> core::fmt::Debug for Router<H> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Router")
            .field(
                "routes",
                &self
                    .routes
                    .iter()
                    .map(|(t, a)| format!("{t} [{a}]"))
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// A path that matched a route.
///
/// Borrows the router and the path, so the parameters cost nothing to carry
/// and cannot outlive the request.
pub struct RouteMatch<'r, 'p, H> {
    inner: wayfind::Match<'r, 'p, Route<H>>,
}

impl<'r, 'p, H> RouteMatch<'r, 'p, H> {
    /// The template that matched, in brocanth syntax.
    #[must_use]
    pub fn template(&self) -> &'r str {
        self.inner.data().template.pattern()
    }

    /// The full template, with its parameter list.
    #[must_use]
    pub fn route_template(&self) -> &'r RouteTemplate {
        &self.inner.data().template
    }

    /// The captured parameters.
    #[must_use]
    pub fn params(&self) -> Params<'_> {
        Params::new(self.inner.parameters())
    }

    /// The handler for `verb`.
    ///
    /// HEAD falls back to the GET handler when the page defines no
    /// `handle_head`, as RFC 9110 requires: a HEAD response is a GET
    /// response without the body. Use [`RouteMatch::handler_exact`] to see
    /// the raw registration.
    #[must_use]
    pub fn handler(&self, verb: Verb) -> Option<&'r H> {
        self.handler_exact(verb).or_else(|| {
            if verb == Verb::Head {
                self.handler_exact(Verb::Get)
            } else {
                None
            }
        })
    }

    /// The handler registered for `verb`, with no fallback.
    #[must_use]
    pub fn handler_exact(&self, verb: Verb) -> Option<&'r H> {
        self.inner.data().handlers[verb.index()].as_ref()
    }

    /// The verbs this route answers, HEAD included when GET is registered.
    #[must_use]
    pub fn allow(&self) -> Allow {
        self.inner.data().allow()
    }
}

impl<H> core::fmt::Debug for RouteMatch<'_, '_, H> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RouteMatch")
            .field("template", &self.template())
            .field("params", &self.params())
            .field("allow", &self.allow().to_string())
            .finish()
    }
}

/// What the router decided about a verb and a path.
#[derive(Debug)]
pub enum Dispatch<'r, 'p, H> {
    /// The path and the verb both matched.
    Found {
        /// The handler to run.
        handler: &'r H,
        /// The template and parameters that got there.
        matched: RouteMatch<'r, 'p, H>,
    },
    /// The path matched; the verb did not.
    ///
    /// Answer 405 and send `allow` as the `Allow` header. Answering 404
    /// instead hides the only fact that makes the mistake findable.
    MethodNotAllowed {
        /// The verbs this path does answer.
        allow: Allow,
        /// The template and parameters that matched.
        matched: RouteMatch<'r, 'p, H>,
    },
    /// No route has that shape.
    NotFound,
}

impl<'r, H> Dispatch<'r, '_, H> {
    /// The status to answer with, or [`None`] when a handler should run.
    #[must_use]
    pub fn status(&self) -> Option<http::StatusCode> {
        match self {
            Self::Found { .. } => None,
            Self::MethodNotAllowed { .. } => Some(http::StatusCode::METHOD_NOT_ALLOWED),
            Self::NotFound => Some(http::StatusCode::NOT_FOUND),
        }
    }

    /// The handler, if one matched.
    #[must_use]
    pub fn handler(&self) -> Option<&'r H> {
        match self {
            Self::Found { handler, .. } => Some(handler),
            _ => None,
        }
    }

    /// Whether a handler matched.
    #[must_use]
    pub const fn is_found(&self) -> bool {
        matches!(self, Self::Found { .. })
    }
}

/// A table that cannot be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum BuildError {
    /// A template does not parse.
    #[error("invalid template")]
    Template(#[from] TemplateError),

    /// A page file does not name a route.
    #[error("invalid page path")]
    Path(#[from] PathError),

    /// The same template and verb were registered twice.
    ///
    /// Silently overwriting would make the live handler depend on scan
    /// order, so this is an error.
    #[error("`{template}` already has a {verb} handler")]
    DuplicateHandler {
        /// The template that was registered twice.
        template: String,
        /// The verb that was registered twice.
        verb: Verb,
    },

    /// Two templates cannot coexist in the trie.
    #[error("`{template}` conflicts with `{existing}`")]
    Conflict {
        /// The template being inserted.
        template: String,
        /// The template already in the table.
        existing: String,
    },

    /// The matcher rejected a template for some other reason.
    #[error("the matcher rejected `{template}`: {reason}")]
    Engine {
        /// The rejected template.
        template: String,
        /// What the matcher reported.
        reason: String,
    },
}

/// Collects routes, then freezes them into a [`Router`].
///
/// Registration order is preserved for [`Router::routes`]; it has no effect
/// on matching, which the trie decides by specificity.
pub struct RouterBuilder<H> {
    order: Vec<Route<H>>,
    index: HashMap<String, usize>,
}

impl<H> Default for RouterBuilder<H> {
    fn default() -> Self {
        Self::new()
    }
}

impl<H> RouterBuilder<H> {
    /// An empty builder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            order: Vec::new(),
            index: HashMap::new(),
        }
    }

    /// Register `handler` for `verb` on `template`.
    ///
    /// Calling this again with the same template and a different verb adds
    /// to the same route.
    ///
    /// # Errors
    ///
    /// [`BuildError::Template`] if the template does not parse, and
    /// [`BuildError::DuplicateHandler`] if that verb is already taken.
    pub fn route(
        &mut self,
        template: &str,
        verb: Verb,
        handler: H,
    ) -> Result<&mut Self, BuildError> {
        let parsed = RouteTemplate::parse(template)?;
        self.route_template(parsed, verb, handler)
    }

    /// Register `handler` for `verb` on an already parsed template.
    ///
    /// # Errors
    ///
    /// [`BuildError::DuplicateHandler`] if that verb is already taken.
    pub fn route_template(
        &mut self,
        template: RouteTemplate,
        verb: Verb,
        handler: H,
    ) -> Result<&mut Self, BuildError> {
        let slot = match self.index.get(template.pattern()) {
            Some(index) => *index,
            None => {
                let index = self.order.len();
                self.index.insert(template.pattern().to_owned(), index);
                self.order.push(Route::new(template));
                index
            }
        };
        let route = &mut self.order[slot];
        if route.handlers[verb.index()].is_some() {
            return Err(BuildError::DuplicateHandler {
                template: route.template.pattern().to_owned(),
                verb,
            });
        }
        route.handlers[verb.index()] = Some(handler);
        Ok(self)
    }

    /// Register several verbs for one template at once.
    ///
    /// This is the shape a file-based page produces: one file, one route,
    /// one handler per `handle*` function it defines.
    ///
    /// # Errors
    ///
    /// As [`RouterBuilder::route`].
    pub fn page(
        &mut self,
        template: &str,
        handlers: impl IntoIterator<Item = (Verb, H)>,
    ) -> Result<&mut Self, BuildError> {
        let parsed = RouteTemplate::parse(template)?;
        for (verb, handler) in handlers {
            self.route_template(parsed.clone(), verb, handler)?;
        }
        Ok(self)
    }

    /// Register a handler by page file path, applying the file convention.
    ///
    /// # Errors
    ///
    /// [`BuildError::Path`] if the file does not name a route, otherwise as
    /// [`RouterBuilder::route`].
    pub fn file(
        &mut self,
        file: &str,
        base: &str,
        verb: Verb,
        handler: H,
    ) -> Result<&mut Self, BuildError> {
        let template = route_for_file(file, base)?;
        self.route_template(template, verb, handler)
    }

    /// The templates registered so far, in registration order.
    ///
    /// Feed these to [`generate_helpers`] to emit typed route helpers for
    /// the same table the server will serve.
    pub fn templates(&self) -> impl Iterator<Item = &RouteTemplate> {
        self.order.iter().map(|route| &route.template)
    }

    /// Freeze into a [`Router`].
    ///
    /// # Errors
    ///
    /// [`BuildError::Conflict`] when two templates cannot coexist, and
    /// [`BuildError::Engine`] for any other rejection by the matcher.
    pub fn build(self) -> Result<Router<H>, BuildError> {
        let mut engine = wayfind::RouterBuilder::new();
        let mut routes = Vec::with_capacity(self.order.len());

        for route in self.order {
            let pattern = route.template.pattern().to_owned();
            routes.push((route.template.clone(), route.allow()));
            let engine_syntax = route.template.to_engine_syntax();
            engine.insert(&engine_syntax, route).map_err(|error| {
                match error {
                    wayfind::InsertError::Conflict { existing } => BuildError::Conflict {
                        template: pattern.clone(),
                        // Report back in brocanth syntax.
                        existing: existing.replace('<', "{").replace('>', "}"),
                    },
                    other => BuildError::Engine {
                        template: pattern.clone(),
                        reason: other.to_string(),
                    },
                }
            })?;
        }

        Ok(Router {
            engine: engine.build(),
            routes,
        })
    }
}
