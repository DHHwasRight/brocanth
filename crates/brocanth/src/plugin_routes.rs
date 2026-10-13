//! The router seam: a plugin that owns twelve screens, and an application that
//! still decides where they live.
//!
//! # The problem this solves
//!
//! Until now a plugin could only *offer* middleware, and the rule behind that
//! is the Crystal author's: *"I do not want plugins to register middleware
//! themselves. I want to manually inject middlewares since I may want to
//! configure the pattern."* Routes are the same decision — a path is a
//! decision — but they do not scale the same way. An admin plugin has twelve
//! screens, an OpenAPI plugin has a document and a viewer, an auth plugin has
//! a callback. Making the application write twelve `mount` lines turns the
//! rule into a chore, and a chore gets a macro wrapped round it, and then the
//! rule is gone.
//!
//! So the unit of the decision is the **bundle**, not the route. A plugin
//! implements [`RouteSource`] and describes its routes with names and paths
//! *relative to a prefix it does not choose*. The application writes one line:
//!
//! ```text
//! app.mount_routes::<AdminPlugin>("/admin")?;
//! ```
//!
//! and the twelve screens appear under `/admin`. Three properties keep this on
//! the right side of the rule:
//!
//! 1. **Nothing mounts itself.** A [`RouteSource`] that is never mounted
//!    contributes no route, and [`App::unmounted_route_sources`] lists the
//!    plugins that offered and were not taken, so "where did the admin go" has
//!    an answer.
//! 2. **The prefix is always the application's.** A plugin's
//!    [`RouteSource::PREFIX`] is a suggestion the application may use with
//!    [`App::mount_routes_default`] and may ignore.
//! 3. **Subsets are one line too.** [`App::mount_routes_where`] filters by
//!    route name, so "the admin, but not the destructive screens" does not
//!    need the plugin's cooperation or twelve lines.
//!
//! # What happens when two plugins want the same path
//!
//! [`RouteError::Conflict`], naming both of them and the path. There is no
//! last-one-wins, because last-one-wins is registration order deciding
//! behaviour, which is the thing this whole design is against.
//!
//! [`App::unmounted_route_sources`]: crate::plugin_app::App::unmounted_route_sources
//! [`App::mount_routes_default`]: crate::plugin_app::App::mount_routes_default
//! [`App::mount_routes_where`]: crate::plugin_app::App::mount_routes_where

use core::future::Future;
use core::pin::Pin;
use std::collections::HashMap;

use thiserror::Error;

use crate::plugin_api::{Plugin, PluginError};
use crate::plugin_context::Context;
use crate::plugin_hooks::{HandlerError, Request, RequestState, Response};
use crate::routing_params::Params;
use crate::routing_path::RouteTemplate;
use crate::routing_verb::Verb;
use crate::suggest::nearest_name;

// ---------------------------------------------------------------------------
// What a contributed handler sees
// ---------------------------------------------------------------------------

/// What a route a plugin contributed is handed.
///
/// The request is borrowed, not owned: a contributed route reads the body, it
/// does not consume it. `E` is the application's context provider list, so a
/// plugin generic over `E` reaches its own values with
/// [`Context::try_get`] and a plugin written against one application's
/// provider list can use [`Context::get`] and have it checked at compile time.
pub struct RouteCx<'a, E> {
    request: &'a Request,
    params: &'a Params<'a>,
    context: &'a Context<E>,
    state: &'a mut RequestState,
}

impl<'a, E> RouteCx<'a, E> {
    pub fn new(
        request: &'a Request,
        params: &'a Params<'a>,
        context: &'a Context<E>,
        state: &'a mut RequestState,
    ) -> Self {
        Self {
            request,
            params,
            context,
            state,
        }
    }

    pub fn request(&self) -> &Request {
        self.request
    }

    pub fn params(&self) -> &Params<'a> {
        self.params
    }

    pub fn context(&self) -> &Context<E> {
        self.context
    }

    pub fn state(&self) -> &RequestState {
        self.state
    }

    pub fn state_mut(&mut self) -> &mut RequestState {
        self.state
    }
}

/// What a contributed route is, when it does not wait for anything.
///
/// Unchanged, deliberately. A sync route is the same box, the same vtable and
/// the same call it has always been; see [`AnyHandler`] for what the one added
/// branch costs.
pub type Handler<E> =
    Box<dyn Fn(&mut RouteCx<'_, E>) -> Result<Response, HandlerError> + Send + Sync>;

/// A future the core carries without owning a runtime.
///
/// `core::future::Future` and `alloc::boxed::Box` are std, so this adds no
/// dependency: `tests/no_hidden_dependencies.rs` still sees three crates.
/// `Send` is here because a bridged request crosses a task boundary, and a
/// future that is not `Send` cannot cross one.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What a contributed route is, when it waits.
///
/// The future borrows the [`RouteCx`], so it may read the request and write
/// the request state across an `.await` without any of it being cloned. The
/// price is one `Box::pin` per request, paid by async routes only.
pub type AsyncHandler<E> = Box<
    dyn for<'a, 'r> Fn(&'a mut RouteCx<'r, E>) -> BoxFuture<'a, Result<Response, HandlerError>>
        + Send
        + Sync,
>;

/// Boxes an async route. A convenience, not a necessity.
///
/// `Box::new(|cx: &mut RouteCx<'_, E>| Box::pin(async move { .. }))` compiles
/// on its own wherever the expected type is already `AsyncHandler<E>` -- a
/// `let` with that annotation, or an argument of
/// [`RouteBundle::get_async`] -- because the expected type drives the
/// closure's signature. Measured, not assumed: `tests/async_handlers.rs`
/// registers one of each way.
///
/// This exists for the third place, where the expected type is a generic `F`
/// and there is nothing to drive the inference. There rustc settles the
/// closure's `Fn` impl at one concrete lifetime and then reports that it does
/// not satisfy the higher-ranked bound, which is the famously unhelpful
/// `lifetime may not live long enough` pointing at a `Box::pin` that is
/// correct. `docs/async-handlers.md` §6.4 has the full error.
pub fn async_handler<E, F>(f: F) -> AsyncHandler<E>
where
    F: for<'a, 'r> Fn(&'a mut RouteCx<'r, E>) -> BoxFuture<'a, Result<Response, HandlerError>>
        + Send
        + Sync
        + 'static,
{
    Box::new(f)
}

/// A route, of either kind.
///
/// One table, not two. The router is generic over its payload, so an
/// application with no async route can still hold a `Router<Handler<E>>` and
/// pay nothing at all; an application with a mix holds a
/// `Router<AnyHandler<E>>` and pays one predictable branch per dispatch, on
/// top of the indirect call it already had.
///
/// Two tables were the other candidate, and they are wrong here for the reason
/// `routing` gives for choosing `wayfind` over two `matchit` routers: a 405
/// needs one matcher. Splitting the table by handler kind puts one path in two
/// places and turns the `Allow` set into a merge.
pub enum AnyHandler<E> {
    Sync(Handler<E>),
    Async(AsyncHandler<E>),
}

impl<E> AnyHandler<E> {
    /// Whether running this route needs a runtime to drive it.
    ///
    /// The bridge reads it to decide between `spawn_blocking` and an inline
    /// `.await`, before it touches the handler.
    #[must_use]
    pub fn is_async(&self) -> bool {
        matches!(self, Self::Async(_))
    }

    /// Runs a sync route, or reports that this one is not sync.
    ///
    /// For a caller with no runtime at all: a test, or the `routes` command.
    pub fn call_sync(&self, cx: &mut RouteCx<'_, E>) -> Option<Result<Response, HandlerError>> {
        match self {
            Self::Sync(handler) => Some(handler(cx)),
            Self::Async(_) => None,
        }
    }

    /// Runs the route, whichever kind it is.
    ///
    /// A sync route run this way is called inline and the future it is wrapped
    /// in completes on its first poll, so nothing is queued and no waker is
    /// used. It is still a future, which is the cost of the uniform call.
    pub async fn call(&self, cx: &mut RouteCx<'_, E>) -> Result<Response, HandlerError> {
        match self {
            Self::Sync(handler) => handler(cx),
            Self::Async(handler) => handler(cx).await,
        }
    }
}

impl<E> From<Handler<E>> for AnyHandler<E> {
    fn from(handler: Handler<E>) -> Self {
        Self::Sync(handler)
    }
}

impl<E> std::fmt::Debug for AnyHandler<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.is_async() { "Async" } else { "Sync" })
    }
}

// ---------------------------------------------------------------------------
// Contribution
// ---------------------------------------------------------------------------

/// One route a plugin contributed, after the application's prefix is applied.
pub struct ContributedRoute<E> {
    /// `admin.dashboard` — the name URL helpers and `brocanth routes` use.
    pub name: String,
    /// The full path, prefix included.
    pub path: String,
    pub verb: Verb,
    /// The plugin instance it came from.
    pub source: String,
    /// One line for `brocanth routes`.
    pub description: String,
    pub handler: AnyHandler<E>,
}

impl<E> std::fmt::Debug for ContributedRoute<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContributedRoute")
            .field("name", &self.name)
            .field("path", &self.path)
            .field("verb", &self.verb)
            .field("source", &self.source)
            .field("kind", &self.handler)
            .finish()
    }
}

/// Where a plugin writes its routes.
///
/// Paths are written relative to the prefix: `"/"` is the prefix itself,
/// `"/posts/{id}"` hangs under it. A plugin cannot read its way out of the
/// prefix, because it never sees an absolute path to start from.
pub struct RouteBundle<'a, E> {
    prefix: &'a str,
    source: &'a str,
    filter: &'a dyn Fn(&str) -> bool,
    out: &'a mut Vec<ContributedRoute<E>>,
    errors: Vec<RouteError>,
}

impl<'a, E> RouteBundle<'a, E> {
    pub(crate) fn new(
        prefix: &'a str,
        source: &'a str,
        filter: &'a dyn Fn(&str) -> bool,
        out: &'a mut Vec<ContributedRoute<E>>,
    ) -> Self {
        Self {
            prefix,
            source,
            filter,
            out,
            errors: Vec::new(),
        }
    }

    /// The prefix the application chose. Read it to build a link; do not try
    /// to escape it.
    pub fn prefix(&self) -> &str {
        self.prefix
    }

    /// The instance contributing.
    pub fn source(&self) -> &str {
        self.source
    }

    /// Adds one route.
    ///
    /// `name` is qualified with the instance name, so `"dashboard"` from the
    /// `admin` instance becomes `admin.dashboard`. A bad path is reported from
    /// [`RouteSource`]'s caller rather than here, so a plugin can describe its
    /// whole bundle without threading a `Result` through every line.
    pub fn route(&mut self, name: &str, verb: Verb, path: &str, handler: Handler<E>) -> &mut Self {
        self.described(name, verb, path, "", handler)
    }

    /// Adds one route that waits.
    ///
    /// The kind is decided here, at registration, and never again. There is no
    /// handler that is sync on Tuesday.
    pub fn route_async(
        &mut self,
        name: &str,
        verb: Verb,
        path: &str,
        handler: AsyncHandler<E>,
    ) -> &mut Self {
        self.described_async(name, verb, path, "", handler)
    }

    /// [`RouteBundle::route`], with a line for `brocanth routes`.
    pub fn described(
        &mut self,
        name: &str,
        verb: Verb,
        path: &str,
        description: &str,
        handler: Handler<E>,
    ) -> &mut Self {
        self.push(name, verb, path, description, AnyHandler::Sync(handler))
    }

    /// [`RouteBundle::route_async`], with a line for `brocanth routes`.
    pub fn described_async(
        &mut self,
        name: &str,
        verb: Verb,
        path: &str,
        description: &str,
        handler: AsyncHandler<E>,
    ) -> &mut Self {
        self.push(name, verb, path, description, AnyHandler::Async(handler))
    }

    fn push(
        &mut self,
        name: &str,
        verb: Verb,
        path: &str,
        description: &str,
        handler: AnyHandler<E>,
    ) -> &mut Self {
        let qualified = format!("{}.{}", self.source, name);
        if !(self.filter)(&qualified) {
            return self;
        }
        let full = join_path(self.prefix, path);
        if let Err(e) = RouteTemplate::parse(&full) {
            self.errors.push(RouteError::BadPath {
                plugin: self.source.to_string(),
                name: qualified,
                path: full,
                reason: e.to_string(),
            });
            return self;
        }
        self.out.push(ContributedRoute {
            name: qualified,
            path: full,
            verb,
            source: self.source.to_string(),
            description: description.to_string(),
            handler,
        });
        self
    }

    /// `GET`, which is most of them.
    pub fn get(&mut self, name: &str, path: &str, handler: Handler<E>) -> &mut Self {
        self.route(name, Verb::Get, path, handler)
    }

    pub fn post(&mut self, name: &str, path: &str, handler: Handler<E>) -> &mut Self {
        self.route(name, Verb::Post, path, handler)
    }

    pub fn put(&mut self, name: &str, path: &str, handler: Handler<E>) -> &mut Self {
        self.route(name, Verb::Put, path, handler)
    }

    pub fn patch(&mut self, name: &str, path: &str, handler: Handler<E>) -> &mut Self {
        self.route(name, Verb::Patch, path, handler)
    }

    pub fn delete(&mut self, name: &str, path: &str, handler: Handler<E>) -> &mut Self {
        self.route(name, Verb::Delete, path, handler)
    }

    pub fn get_async(&mut self, name: &str, path: &str, handler: AsyncHandler<E>) -> &mut Self {
        self.route_async(name, Verb::Get, path, handler)
    }

    pub fn post_async(&mut self, name: &str, path: &str, handler: AsyncHandler<E>) -> &mut Self {
        self.route_async(name, Verb::Post, path, handler)
    }

    pub fn put_async(&mut self, name: &str, path: &str, handler: AsyncHandler<E>) -> &mut Self {
        self.route_async(name, Verb::Put, path, handler)
    }

    pub fn patch_async(&mut self, name: &str, path: &str, handler: AsyncHandler<E>) -> &mut Self {
        self.route_async(name, Verb::Patch, path, handler)
    }

    pub fn delete_async(&mut self, name: &str, path: &str, handler: AsyncHandler<E>) -> &mut Self {
        self.route_async(name, Verb::Delete, path, handler)
    }

    pub(crate) fn into_errors(self) -> Vec<RouteError> {
        self.errors
    }
}

/// A plugin that contributes routes.
///
/// Implemented in addition to [`Plugin`]. Set `Plugin::CONTRIBUTES_ROUTES` to
/// `true` in the same `impl Plugin`, so that a plugin whose routes were never
/// mounted can say so instead of disappearing quietly.
pub trait RouteSource<E>: Plugin {
    /// Where the plugin would put itself if it were asked. A suggestion:
    /// [`App::mount_routes`] overrides it and
    /// [`App::mount_routes_default`] takes it.
    ///
    /// [`App::mount_routes`]: crate::plugin_app::App::mount_routes
    /// [`App::mount_routes_default`]: crate::plugin_app::App::mount_routes_default
    const PREFIX: &'static str = "/";

    fn routes(&self, bundle: &mut RouteBundle<'_, E>) -> Result<(), PluginError>;
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum RouteError {
    #[error(
        "`{first}` and `{second}` both mount `{verb} {path}`; a path answers to one handler, \
         so one of the two mounts needs a different prefix or a filter"
    )]
    Conflict {
        path: String,
        verb: Verb,
        first: String,
        second: String,
    },

    #[error(
        "two routes are named `{name}`, one from `{first}` and one from `{second}`; a route \
         name is what a link is written against, so it has to be unique"
    )]
    DuplicateName {
        name: String,
        first: String,
        second: String,
    },

    #[error("`{plugin}` contributes `{name}` at `{path}`, which is not a valid path: {reason}")]
    BadPath {
        plugin: String,
        name: String,
        path: String,
        reason: String,
    },

    #[error(
        "no registered plugin instance is `{plugin}`{}; registered: {}",
        crate::plugin_routes::route_hint(.suggestion),
        if .registered.is_empty() { "nothing".to_string() } else { .registered.join(", ") }
    )]
    NoSuchPlugin {
        plugin: String,
        suggestion: Option<String>,
        registered: Vec<String>,
    },

    #[error("`{plugin}` failed while describing its routes: {error}")]
    Source {
        plugin: String,
        #[source]
        error: PluginError,
    },
}

#[doc(hidden)]
pub fn route_hint(suggestion: &Option<String>) -> String {
    match suggestion {
        Some(s) => format!("; did you mean `{s}`?"),
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------
// The plan
// ---------------------------------------------------------------------------

/// Every route the application accepted, and from whom.
///
/// A plain field of the [`App`](crate::plugin_app::App). Two applications in
/// one process have two plans.
pub struct RoutePlan<E> {
    routes: Vec<ContributedRoute<E>>,
    occupied: HashMap<(String, Verb), String>,
    names: HashMap<String, String>,
    mounted: Vec<String>,
}

impl<E> Default for RoutePlan<E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E> RoutePlan<E> {
    pub fn new() -> Self {
        Self {
            routes: Vec::new(),
            occupied: HashMap::new(),
            names: HashMap::new(),
            mounted: Vec::new(),
        }
    }

    /// Every accepted route, in the order it was mounted.
    pub fn routes(&self) -> &[ContributedRoute<E>] {
        &self.routes
    }

    pub fn len(&self) -> usize {
        self.routes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }

    /// One route by its qualified name.
    pub fn named(&self, name: &str) -> Option<&ContributedRoute<E>> {
        self.routes.iter().find(|r| r.name == name)
    }

    /// The instances whose bundles were accepted, in mount order.
    pub fn mounted_sources(&self) -> &[String] {
        &self.mounted
    }

    pub(crate) fn accept(
        &mut self,
        instance: &str,
        incoming: Vec<ContributedRoute<E>>,
    ) -> Result<usize, RouteError> {
        // Check everything before changing anything, so a rejected mount
        // leaves the plan exactly as it was.
        let mut seen_here: HashMap<(String, Verb), String> = HashMap::new();
        let mut names_here: HashMap<String, String> = HashMap::new();
        for route in &incoming {
            let key = (route.path.clone(), route.verb);
            if let Some(other) = self
                .occupied
                .get(&key)
                .or_else(|| seen_here.get(&key))
                .cloned()
            {
                return Err(RouteError::Conflict {
                    path: route.path.clone(),
                    verb: route.verb,
                    first: other,
                    second: route.source.clone(),
                });
            }
            if let Some(other) = self
                .names
                .get(&route.name)
                .or_else(|| names_here.get(&route.name))
                .cloned()
            {
                return Err(RouteError::DuplicateName {
                    name: route.name.clone(),
                    first: other,
                    second: route.source.clone(),
                });
            }
            seen_here.insert(key, route.source.clone());
            names_here.insert(route.name.clone(), route.source.clone());
        }

        let count = incoming.len();
        for route in incoming {
            self.occupied
                .insert((route.path.clone(), route.verb), route.source.clone());
            self.names.insert(route.name.clone(), route.source.clone());
            self.routes.push(route);
        }
        self.mounted.push(instance.to_string());
        Ok(count)
    }
}

impl<E> std::fmt::Debug for RoutePlan<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoutePlan")
            .field("routes", &self.routes.len())
            .field("mounted", &self.mounted)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// Hangs `path` under `prefix`, without ever producing `//` or a trailing
/// slash that was not asked for.
pub(crate) fn join_path(prefix: &str, path: &str) -> String {
    let prefix = prefix.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    if path.is_empty() {
        if prefix.is_empty() {
            return "/".to_string();
        }
        return prefix.to_string();
    }
    format!("{prefix}/{path}")
}

#[doc(hidden)]
pub fn no_such_plugin(plugin: &str, registered: &[String]) -> RouteError {
    RouteError::NoSuchPlugin {
        plugin: plugin.to_string(),
        suggestion: nearest_name(plugin, registered),
        registered: registered.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prefix_and_a_path_join_without_a_double_slash() {
        assert_eq!(join_path("/admin", "/posts"), "/admin/posts");
        assert_eq!(join_path("/admin/", "/posts"), "/admin/posts");
        assert_eq!(join_path("/admin", "posts"), "/admin/posts");
        assert_eq!(join_path("/admin", "/"), "/admin");
        assert_eq!(join_path("/", "/posts"), "/posts");
        assert_eq!(join_path("/", "/"), "/");
        assert_eq!(join_path("/admin", "/posts/{id}"), "/admin/posts/{id}");
    }
}
