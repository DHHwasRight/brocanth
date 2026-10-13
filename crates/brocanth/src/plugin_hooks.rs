//! The request, response and error seams, and the type-level record of which
//! of them an application installed.
//!
//! # Free when nobody asks
//!
//! The constraint is the whole design here: **a hook nobody registers must
//! cost nothing** — no allocation, no dynamic dispatch, and no branch on an
//! empty list in the hot path. A `Vec` that is empty still has to be looked at
//! to find that out, so emptiness cannot be a run-time fact. It is a fact
//! about the application's *type*.
//!
//! [`AppBuilder`](crate::plugin_app::AppBuilder) carries a second type
//! parameter beside the context provider list: a [`HookFamilies`] record,
//! starting at [`NoHooks`] and widened by
//! [`plugin_hooking`](crate::plugin_app::AppBuilder::plugin_hooking). Every
//! entry point here is generic over that
//! record and opens with `if !H::REQUEST { return … }`, which is a constant
//! after monomorphisation. `tests/hooks_cost_nothing.rs` reads the generated
//! LLVM IR and fails if the empty pipeline is anything other than a `ret`.
//!
//! The price is one extra builder method. `.plugin(p)` registers a plugin;
//! `.plugin_hooking(p)` registers a plugin **and** installs its hooks, and a
//! plugin that hooks declares `const INSTALLS_HOOKS: bool = true` so that
//! reaching for the wrong one fails the build with the line to write instead.
//! See ADR 1 in PLAN.md.
//!
//! # When each one fires
//!
//! ```text
//! before_dispatch   request hooks, declared order           may short-circuit
//!     dispatch      the application's handler
//!     on_error      error hooks, declared order             first answer wins
//! on_response       response hooks, reverse declared order  may rewrite
//! after_dispatch    request hooks, reverse declared order   read-only
//! ```
//!
//! Response hooks run inside-out, like an onion, so a hook declared *after*
//! another sees the response *first*. That is what compression needs: an ETag
//! hook ordered before it computes the tag over the body the client will
//! actually be able to verify, and compression, declared last, wraps the
//! result.
//!
//! `after_dispatch` runs whatever happened — a short-circuit, a handler error,
//! a response hook failure — because a logger that only logs the happy path is
//! not a logger.
//!
//! # The request hook that waits
//!
//! [`AsyncRequestHook`] is a fifth family, for the hook that must read a
//! session store before the handler runs. It is a second **trait**, because
//! its `before_dispatch` returns a future, and a second **marker**
//! ([`OnAsyncRequest`]), because the record has to say whether the chain
//! survives compilation. It is **not** a second phase and **not** a second
//! list.
//!
//! One list, one [`sort`](crate::plugin_order::sort), one declared order.
//! `after("auth.session")` means the same thing whichever kind
//! `auth.session` is, and a cycle across the two kinds is the same build
//! error as a cycle within one. The cost of that decision is stated once and
//! loudly:
//!
//! > **An application that installs an async request hook cannot use
//! > [`Pipeline::run`], [`Pipeline::begin`] or `App::run`.** They are a
//! > compile error for it, not a silent skip. It calls
//! > [`Pipeline::begin_async`], awaits the handler, and calls
//! > [`Pipeline::finish`].
//!
//! The alternative — two lists, the sync one drained before the async one —
//! was rejected. It makes `after("auth.session")` true or false depending on
//! a property of `auth.session` that its author never wrote down, and it
//! cannot express "load the session, then stamp the request id" at all if the
//! two hooks land in the wrong lists. A seam that cannot be declared is a
//! seam that gets discovered in production.
//!
//! `after_dispatch` has no async twin. It cannot fail, cannot change the
//! response, and runs after the response is final, so waiting there holds the
//! connection open for work the client is not waiting for. An async hook's
//! `after_dispatch` is the ordinary synchronous one, in the ordinary reverse
//! order, among the ordinary hooks.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;

use crate::plugin_api::{Plugin, PluginError};
use crate::plugin_model::{ModelEvent, ModelHook, ModelHooks};
use crate::plugin_order::{HookId, Order, Ordered};
use crate::plugin_routes::BoxFuture;

/// The request a hook sees. Buffered, because the framework buffers: PLAN.md
/// §3, *"Buffered render, clean 500 — no half-written responses."*
pub type Request = http::Request<Vec<u8>>;

/// The response a hook sees.
pub type Response = http::Response<Vec<u8>>;

// ---------------------------------------------------------------------------
// The type-level record of installed families
// ---------------------------------------------------------------------------

/// Which hook families an application installed, as a fact the compiler knows.
///
/// A plugin names one of these as its [`HookSource::Families`]; the builder
/// unions them with [`Both`] as plugins are registered. Nothing implementing
/// this trait has a value — they are markers, and every one of them is
/// zero-sized.
pub trait HookFamilies: 'static {
    const REQUEST: bool;
    const RESPONSE: bool;
    const ERROR: bool;
    const MODEL: bool;

    /// The plugin installed an [`AsyncRequestHook`].
    ///
    /// Defaulted, so a record written before this family existed still
    /// compiles. A record that says `true` here makes the **synchronous**
    /// request entry points a compile error for that application: see
    /// [`Pipeline::before_dispatch`].
    const ASYNC_REQUEST: bool = false;

    /// Whether any family at all is installed.
    const ANY: bool =
        Self::REQUEST || Self::RESPONSE || Self::ERROR || Self::MODEL || Self::ASYNC_REQUEST;
}

/// No hooks. Where every application starts, and where most of them stay.
pub struct NoHooks;

/// The plugin hooks the request: before dispatch, after dispatch, or both.
pub struct OnRequest;

/// The plugin hooks the request and waits while it does.
///
/// A separate marker from [`OnRequest`] because the two traits have different
/// signatures, **not** because the two run in separate phases. Both land in
/// one chain and one declared order: see [`Pipeline::before_dispatch_async`].
pub struct OnAsyncRequest;

/// The plugin rewrites responses.
pub struct OnResponse;

/// The plugin turns errors into responses.
pub struct OnError;

/// The plugin hooks model events an ORM emits.
pub struct OnModel;

/// Both of two records. A plugin that hooks two families writes
/// `Both<OnRequest, OnResponse>`.
pub struct Both<A, B>(PhantomData<fn() -> (A, B)>);

impl HookFamilies for NoHooks {
    const REQUEST: bool = false;
    const RESPONSE: bool = false;
    const ERROR: bool = false;
    const MODEL: bool = false;
    const ASYNC_REQUEST: bool = false;
}

impl HookFamilies for OnRequest {
    const REQUEST: bool = true;
    const RESPONSE: bool = false;
    const ERROR: bool = false;
    const MODEL: bool = false;
    const ASYNC_REQUEST: bool = false;
}

impl HookFamilies for OnAsyncRequest {
    const REQUEST: bool = false;
    const RESPONSE: bool = false;
    const ERROR: bool = false;
    const MODEL: bool = false;
    const ASYNC_REQUEST: bool = true;
}

impl HookFamilies for OnResponse {
    const REQUEST: bool = false;
    const RESPONSE: bool = true;
    const ERROR: bool = false;
    const MODEL: bool = false;
    const ASYNC_REQUEST: bool = false;
}

impl HookFamilies for OnError {
    const REQUEST: bool = false;
    const RESPONSE: bool = false;
    const ERROR: bool = true;
    const MODEL: bool = false;
    const ASYNC_REQUEST: bool = false;
}

impl HookFamilies for OnModel {
    const REQUEST: bool = false;
    const RESPONSE: bool = false;
    const ERROR: bool = false;
    const MODEL: bool = true;
    const ASYNC_REQUEST: bool = false;
}

impl<A: HookFamilies, B: HookFamilies> HookFamilies for Both<A, B> {
    const REQUEST: bool = A::REQUEST || B::REQUEST;
    const RESPONSE: bool = A::RESPONSE || B::RESPONSE;
    const ERROR: bool = A::ERROR || B::ERROR;
    const MODEL: bool = A::MODEL || B::MODEL;
    const ASYNC_REQUEST: bool = A::ASYNC_REQUEST || B::ASYNC_REQUEST;
}

// ---------------------------------------------------------------------------
// Per-request scratch
// ---------------------------------------------------------------------------

/// What hooks hand each other inside one request.
///
/// A `before`/`after` pair needs somewhere to put a timer, and the context is
/// sealed once built. This is that place: typed by the key's own type, so
/// there is no string to misspell, and empty until somebody writes — an empty
/// `HashMap` does not allocate.
#[derive(Default)]
pub struct RequestState {
    map: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl RequestState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores a value, returning what it replaced.
    pub fn insert<T: Send + Sync + 'static>(&mut self, value: T) -> Option<T> {
        self.map
            .insert(TypeId::of::<T>(), Box::new(value))
            .and_then(|b| b.downcast::<T>().ok().map(|b| *b))
    }

    pub fn get<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.map
            .get(&TypeId::of::<T>())
            .and_then(|b| b.downcast_ref::<T>())
    }

    pub fn get_mut<T: Send + Sync + 'static>(&mut self) -> Option<&mut T> {
        self.map
            .get_mut(&TypeId::of::<T>())
            .and_then(|b| b.downcast_mut::<T>())
    }

    /// Takes a value out.
    pub fn take<T: Send + Sync + 'static>(&mut self) -> Option<T> {
        self.map
            .remove(&TypeId::of::<T>())
            .and_then(|b| b.downcast::<T>().ok().map(|b| *b))
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }
}

impl std::fmt::Debug for RequestState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestState")
            .field("len", &self.len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// The error a handler can fail with
// ---------------------------------------------------------------------------

/// Anything a handler or a hook failed with, carried without being understood.
///
/// The core cannot know what a validation failure is, or an authorisation
/// failure, so it does not try: it carries the error and lets an
/// [`ErrorHook`] recognise its own type through [`HandlerError::downcast_ref`]
/// and answer. An error nobody recognises becomes a bare 500.
#[derive(Debug)]
pub struct HandlerError {
    inner: Box<dyn std::error::Error + Send + Sync + 'static>,
}

impl HandlerError {
    pub fn new<E: std::error::Error + Send + Sync + 'static>(error: E) -> Self {
        Self {
            inner: Box::new(error),
        }
    }

    /// For the error that is only a sentence.
    pub fn message(message: impl Into<String>) -> Self {
        Self::new(PluginError::new(message))
    }

    /// The error as its own type, when this is the hook that owns it.
    pub fn downcast_ref<E: std::error::Error + 'static>(&self) -> Option<&E> {
        self.inner.downcast_ref::<E>()
    }

    /// Whether the error is an `E`.
    pub fn is<E: std::error::Error + 'static>(&self) -> bool {
        self.inner.is::<E>()
    }

    pub fn as_error(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
        self.inner.as_ref()
    }
}

impl std::fmt::Display for HandlerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.inner.fmt(f)
    }
}

impl std::error::Error for HandlerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.inner.source()
    }
}

impl From<PluginError> for HandlerError {
    fn from(e: PluginError) -> Self {
        Self::new(e)
    }
}

// ---------------------------------------------------------------------------
// The three hot-path traits
// ---------------------------------------------------------------------------

/// What `before_dispatch` decided.
pub enum Flow {
    /// Carry on to the next hook, and then to the handler.
    Continue,
    /// Answer now. No later request hook runs, and neither does the handler;
    /// response hooks and `after_dispatch` still do.
    Respond(Box<Response>),
}

impl Flow {
    /// Answer now with this response.
    pub fn respond(response: Response) -> Self {
        Flow::Respond(Box::new(response))
    }

    pub fn is_continue(&self) -> bool {
        matches!(self, Flow::Continue)
    }
}

impl std::fmt::Debug for Flow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Flow::Continue => f.write_str("Continue"),
            Flow::Respond(r) => write!(f, "Respond({})", r.status()),
        }
    }
}

/// Around dispatch.
///
/// `before_dispatch` runs in declared order and may short-circuit;
/// `after_dispatch` runs in reverse declared order, sees the finished
/// response, and cannot change it. A logger implements both; a metrics plugin
/// implements both; a request-id plugin implements only the first.
pub trait RequestHook: Send + Sync + 'static {
    /// Before the handler. Returning [`Flow::Respond`] stops the chain and the
    /// handler; returning `Err` stops them too and goes to the error hooks.
    fn before_dispatch(
        &self,
        request: &mut Request,
        state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        let _ = (request, state);
        Ok(Flow::Continue)
    }

    /// After the response is final. Cannot fail, because there is nothing left
    /// to fail into.
    fn after_dispatch(&self, request: &Request, response: &Response, state: &mut RequestState) {
        let _ = (request, response, state);
    }
}

/// Around dispatch, for a hook that waits.
///
/// The same seam as [`RequestHook`], with one difference: `before_dispatch`
/// returns a future, so the hook may read a session store, a cache or an HTTP
/// service before the handler runs. Everything else is the same, and that is
/// deliberate.
///
/// # One chain, not two
///
/// `OnRequest` and [`OnAsyncRequest`] are two families, two traits and two
/// shapes of `before_dispatch`. They are **not** two phases. Both kinds go
/// into one list, sorted once by [`sort`](crate::plugin_order::sort), so
/// `after("auth.session")` means the same thing whichever kind
/// `auth.session` is and whichever kind declared it. See
/// [`Pipeline::before_dispatch_async`] for what that costs.
///
/// # `after_dispatch` is not async, on purpose
///
/// It cannot fail, cannot change the response, and runs after the response is
/// final. Waiting there holds the connection open to do work the client is not
/// waiting for; a plugin that wants that wants a job queue. So the signature
/// here is the synchronous one, byte for byte the same as
/// [`RequestHook::after_dispatch`], and an async hook that started a timer in
/// `before_dispatch` still stops it in the right place and the right order.
///
/// # Writing one without `async_trait`
///
/// The core has no dependencies to spend on that, so the method returns a
/// [`BoxFuture`] and the body is an `async move` block:
///
/// ```
/// use brocanth::plugin_api::PluginError;
/// use brocanth::plugin_hooks::{AsyncRequestHook, Flow, Request, RequestState};
/// use brocanth::plugin_routes::BoxFuture;
///
/// struct LoadSession;
///
/// impl AsyncRequestHook for LoadSession {
///     fn before_dispatch<'a>(
///         &'a self,
///         request: &'a mut Request,
///         state: &'a mut RequestState,
///     ) -> BoxFuture<'a, Result<Flow, PluginError>> {
///         Box::pin(async move {
///             let _ = (request, state);
///             Ok(Flow::Continue)
///         })
///     }
/// }
/// ```
pub trait AsyncRequestHook: Send + Sync + 'static {
    /// Before the handler, with an `.await` available.
    ///
    /// [`Flow::Respond`] and `Err` mean exactly what they mean on
    /// [`RequestHook::before_dispatch`]: the rest of the chain does not run,
    /// the handler does not run, and response hooks and `after_dispatch`
    /// still do. "The rest of the chain" is the whole chain, sync hooks
    /// included, because there is only one.
    fn before_dispatch<'a>(
        &'a self,
        request: &'a mut Request,
        state: &'a mut RequestState,
    ) -> BoxFuture<'a, Result<Flow, PluginError>>;

    /// After the response is final. Synchronous, for the reason in the trait
    /// header.
    fn after_dispatch(&self, request: &Request, response: &Response, state: &mut RequestState) {
        let _ = (request, response, state);
    }
}

/// One step of the request chain: a hook that returns, or a hook that waits.
///
/// Private to the crate because it is a representation, not a seam. A plugin
/// implements [`RequestHook`] or [`AsyncRequestHook`]; this is only how the
/// pipeline keeps them in one order.
pub(crate) enum RequestStep {
    Sync(Box<dyn RequestHook>),
    Async(Box<dyn AsyncRequestHook>),
}

impl RequestStep {
    fn after_dispatch(&self, request: &Request, response: &Response, state: &mut RequestState) {
        match self {
            Self::Sync(hook) => hook.after_dispatch(request, response, state),
            Self::Async(hook) => hook.after_dispatch(request, response, state),
        }
    }
}

/// Rewriting a response: headers, body, or both.
///
/// Runs in **reverse** declared order, so the hook declared last is the
/// outermost and sees the response first. Compression, partial-navigation
/// headers and ETag all live here.
pub trait ResponseHook: Send + Sync + 'static {
    /// Returning `Err` replaces the response with a 500 and skips the
    /// remaining response hooks. `after_dispatch` still runs.
    fn on_response(
        &self,
        request: &Request,
        response: &mut Response,
        state: &mut RequestState,
    ) -> Result<(), PluginError>;
}

/// Turning an error into a response.
///
/// Runs in declared order; the **first** hook that answers `Some` wins and the
/// rest do not run. A validation plugin recognises its own `ValidationErrors`
/// and re-renders the form; an auth plugin recognises `Unauthorized` and
/// redirects; the core recognises neither and falls back to a bare 500.
pub trait ErrorHook: Send + Sync + 'static {
    fn on_error(
        &self,
        error: &HandlerError,
        request: &Request,
        state: &mut RequestState,
    ) -> Option<Response>;
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Where a plugin puts its hooks.
///
/// Handed to [`HookSource::install_hooks`]. Each call takes a name, which is
/// qualified with the instance name so that two instances of one plugin get
/// two distinguishable hooks, and an [`Order`].
pub struct HookRegistrar<'a> {
    instance: &'a str,
    request: &'a mut Vec<Ordered<RequestStep>>,
    response: &'a mut Vec<Ordered<Box<dyn ResponseHook>>>,
    error: &'a mut Vec<Ordered<Box<dyn ErrorHook>>>,
    model: &'a mut ModelHooks,
}

impl<'a> HookRegistrar<'a> {
    pub(crate) fn new(
        instance: &'a str,
        request: &'a mut Vec<Ordered<RequestStep>>,
        response: &'a mut Vec<Ordered<Box<dyn ResponseHook>>>,
        error: &'a mut Vec<Ordered<Box<dyn ErrorHook>>>,
        model: &'a mut ModelHooks,
    ) -> Self {
        Self {
            instance,
            request,
            response,
            error,
            model,
        }
    }

    /// The instance registering. The core fills this in; a plugin cannot claim
    /// another instance's name.
    pub fn instance(&self) -> &str {
        self.instance
    }

    pub fn request(&mut self, name: &str, order: Order, hook: impl RequestHook) -> &mut Self {
        self.request.push(Ordered::new(
            HookId::new(self.instance, name),
            order,
            RequestStep::Sync(Box::new(hook)),
        ));
        self
    }

    /// A request hook that waits. See [`AsyncRequestHook`].
    ///
    /// It goes into the **same** list as [`HookRegistrar::request`], so an
    /// `Order` may name a sync hook and be obeyed, and a sync hook may name
    /// this one and be obeyed. The plugin must also name
    /// [`OnAsyncRequest`] in its [`HookSource::Families`]; it is the record
    /// that decides whether the chain survives compilation at all, so a hook
    /// registered here and missing from there fails the build rather than
    /// going quiet.
    pub fn request_async(
        &mut self,
        name: &str,
        order: Order,
        hook: impl AsyncRequestHook,
    ) -> &mut Self {
        self.request.push(Ordered::new(
            HookId::new(self.instance, name),
            order,
            RequestStep::Async(Box::new(hook)),
        ));
        self
    }

    pub fn response(&mut self, name: &str, order: Order, hook: impl ResponseHook) -> &mut Self {
        self.response.push(Ordered::new(
            HookId::new(self.instance, name),
            order,
            Box::new(hook) as Box<dyn ResponseHook>,
        ));
        self
    }

    pub fn error(&mut self, name: &str, order: Order, hook: impl ErrorHook) -> &mut Self {
        self.error.push(Ordered::new(
            HookId::new(self.instance, name),
            order,
            Box::new(hook) as Box<dyn ErrorHook>,
        ));
        self
    }

    /// A hook on a model event the ORM defines. See [`crate::plugin_model`].
    pub fn model<Ev: ModelEvent>(
        &mut self,
        name: &str,
        order: Order,
        hook: impl ModelHook<Ev>,
    ) -> &mut Self {
        self.model
            .register::<Ev>(HookId::new(self.instance, name), order, hook);
        self
    }
}

/// A plugin that installs hooks.
///
/// Implemented *in addition to* [`Plugin`], and the application registers it
/// with [`AppBuilder::plugin_hooking`] rather than `plugin`, which is what
/// widens the application's [`HookFamilies`] record. Set
/// `Plugin::INSTALLS_HOOKS` to `true` in the same `impl Plugin` so that
/// registering it the other way is a build error rather than silence.
///
/// [`AppBuilder::plugin_hooking`]: crate::plugin_app::AppBuilder::plugin_hooking
pub trait HookSource: Plugin {
    /// Which families this plugin hooks. `OnRequest`, `Both<OnRequest,
    /// OnResponse>`, and so on.
    type Families: HookFamilies;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError>;
}

// ---------------------------------------------------------------------------
// The pipeline
// ---------------------------------------------------------------------------

/// The sorted hooks of one application.
///
/// Built once, at the end of `AppBuilder::build`, and read-only after that.
/// It is a plain field of the [`App`](crate::plugin_app::App): two
/// applications in one process have two pipelines and cannot reach each
/// other's.
#[derive(Default)]
pub struct Pipeline {
    /// The sync and the async request hooks, in one declared order. The kind
    /// of each step is a property of the step, never of its position.
    request: Vec<RequestStep>,
    response: Vec<Box<dyn ResponseHook>>,
    error: Vec<Box<dyn ErrorHook>>,
}

impl Pipeline {
    pub(crate) fn from_parts(
        request: Vec<RequestStep>,
        response: Vec<Box<dyn ResponseHook>>,
        error: Vec<Box<dyn ErrorHook>>,
    ) -> Self {
        Self {
            request,
            response,
            error,
        }
    }

    /// How many [`RequestHook`]s are installed.
    pub fn request_hooks(&self) -> usize {
        self.request
            .iter()
            .filter(|s| matches!(s, RequestStep::Sync(_)))
            .count()
    }

    /// How many [`AsyncRequestHook`]s are installed.
    pub fn async_request_hooks(&self) -> usize {
        self.request
            .iter()
            .filter(|s| matches!(s, RequestStep::Async(_)))
            .count()
    }

    /// Whether any installed request hook waits.
    ///
    /// A value, not a type, so a server layer can log it. The decision that
    /// matters is the type-level one on [`HookFamilies::ASYNC_REQUEST`].
    pub fn has_async_request_hooks(&self) -> bool {
        self.async_request_hooks() > 0
    }

    pub fn response_hooks(&self) -> usize {
        self.response.len()
    }

    pub fn error_hooks(&self) -> usize {
        self.error.len()
    }

    pub fn is_empty(&self) -> bool {
        self.request.is_empty() && self.response.is_empty() && self.error.is_empty()
    }

    /// Runs the request hooks in declared order, for a caller that cannot
    /// wait.
    ///
    /// `H` is the application's family record, so when no plugin installed a
    /// request hook the whole body folds away to `Ok(Flow::Continue)`.
    ///
    /// # It does not compile when an async request hook is installed
    ///
    /// An application whose record says `H::ASYNC_REQUEST` has a chain this
    /// function cannot walk: one of the steps returns a future, and there is
    /// nothing here to drive it. Skipping that step would skip a session
    /// lookup or an authorisation check and answer the request anyway, so the
    /// const block below refuses at compile time instead, naming
    /// [`Pipeline::before_dispatch_async`] as the call to write. The error
    /// arrives at the line that called it — `App::run`, `App::begin` or this
    /// — and not at run time.
    ///
    /// ```compile_fail
    /// # use brocanth::plugin_api::{Plugin, PluginError};
    /// # use brocanth::plugin_app::AppBuilder;
    /// # use brocanth::plugin_context::NoContext;
    /// # use brocanth::plugin_hooks::{
    /// #     AsyncRequestHook, Flow, HookRegistrar, HookSource, OnAsyncRequest, Request,
    /// #     RequestState, Response,
    /// # };
    /// # use brocanth::plugin_order::Order;
    /// # use brocanth::plugin_routes::BoxFuture;
    /// # struct Session;
    /// # impl Plugin for Session {
    /// #     type Config = ();
    /// #     type Provides = NoContext;
    /// #     const NAME: &'static str = "session";
    /// #     const INSTALLS_HOOKS: bool = true;
    /// #     fn new(_: ()) -> Self { Self }
    /// # }
    /// # impl AsyncRequestHook for Session {
    /// #     fn before_dispatch<'a>(
    /// #         &'a self,
    /// #         _request: &'a mut Request,
    /// #         _state: &'a mut RequestState,
    /// #     ) -> BoxFuture<'a, Result<Flow, PluginError>> {
    /// #         Box::pin(async { Ok(Flow::Continue) })
    /// #     }
    /// # }
    /// # impl HookSource for Session {
    /// #     type Families = OnAsyncRequest;
    /// #     fn install_hooks(&self, r: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
    /// #         r.request_async("load", Order::new(), Session);
    /// #         Ok(())
    /// #     }
    /// # }
    /// let app = AppBuilder::new().plugin_hooking(Session).build().unwrap();
    /// let mut request = Request::new(Vec::new());
    /// let mut state = RequestState::new();
    ///
    /// // The chain holds a step this call cannot run, so it does not build.
    /// app.run(&mut request, &mut state, |_, _| Ok(Response::new(Vec::new())));
    /// ```
    ///
    /// The same application with the same hook, through the entry point that
    /// can run it. This one builds, which is what makes the refusal above a
    /// statement about the const block and not about a typo:
    ///
    /// ```
    /// # use brocanth::plugin_api::{Plugin, PluginError};
    /// # use brocanth::plugin_app::AppBuilder;
    /// # use brocanth::plugin_context::NoContext;
    /// # use brocanth::plugin_hooks::{
    /// #     AsyncRequestHook, Flow, HookRegistrar, HookSource, OnAsyncRequest, Request,
    /// #     RequestState, Response,
    /// # };
    /// # use brocanth::plugin_order::Order;
    /// # use brocanth::plugin_routes::BoxFuture;
    /// # struct Session;
    /// # impl Plugin for Session {
    /// #     type Config = ();
    /// #     type Provides = NoContext;
    /// #     const NAME: &'static str = "session";
    /// #     const INSTALLS_HOOKS: bool = true;
    /// #     fn new(_: ()) -> Self { Self }
    /// # }
    /// # impl AsyncRequestHook for Session {
    /// #     fn before_dispatch<'a>(
    /// #         &'a self,
    /// #         _request: &'a mut Request,
    /// #         _state: &'a mut RequestState,
    /// #     ) -> BoxFuture<'a, Result<Flow, PluginError>> {
    /// #         Box::pin(async { Ok(Flow::Continue) })
    /// #     }
    /// # }
    /// # impl HookSource for Session {
    /// #     type Families = OnAsyncRequest;
    /// #     fn install_hooks(&self, r: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
    /// #         r.request_async("load", Order::new(), Session);
    /// #         Ok(())
    /// #     }
    /// # }
    /// let app = AppBuilder::new().plugin_hooking(Session).build().unwrap();
    /// let mut request = Request::new(Vec::new());
    /// let mut state = RequestState::new();
    ///
    /// // A future for the caller's own executor to drive. Building it is
    /// // what this example checks; `tests/async_request_hooks.rs` awaits it.
    /// let _begin = app.begin_async(&mut request, &mut state);
    /// ```
    #[inline]
    pub fn before_dispatch<H: HookFamilies>(
        &self,
        request: &mut Request,
        state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        const {
            assert!(
                !H::ASYNC_REQUEST,
                "this application installed an async request hook, and the synchronous \
                 request-hook entry points cannot run one. Replace `App::run` with \
                 `App::begin_async`, the handler, and `App::finish`."
            );
        }
        if !H::REQUEST {
            return Ok(Flow::Continue);
        }
        for step in &self.request {
            // `H::ASYNC_REQUEST` is false here, so the chain holds no async
            // step and this arm is dead. It is written as a skip rather than
            // an `unreachable!` so the hot path carries no panic landing pad.
            let RequestStep::Sync(hook) = step else {
                continue;
            };
            match hook.before_dispatch(request, state)? {
                Flow::Continue => {}
                stop => return Ok(stop),
            }
        }
        Ok(Flow::Continue)
    }

    /// Runs the request hooks in declared order, waiting where a hook waits.
    ///
    /// This is the entry point for a caller that has a runtime. It walks the
    /// **whole** chain — the [`RequestHook`]s and the [`AsyncRequestHook`]s,
    /// in the one order [`sort`](crate::plugin_order::sort) produced — so
    /// `after("auth.session")` is obeyed whichever kind each hook is. A sync
    /// hook in this chain is called, not awaited; it costs the call and
    /// nothing else.
    ///
    /// [`Flow::Respond`] and `Err` mean here exactly what they mean in
    /// [`Pipeline::before_dispatch`]: no later hook of either kind runs, the
    /// handler does not run, and [`Pipeline::finish`] still runs the response
    /// hooks and `after_dispatch`.
    ///
    /// When neither request family is installed the body folds away to
    /// `Ok(Flow::Continue)` and the future is ready on its first poll:
    /// `tests/hooks_cost_nothing.rs` reads the generated code for it.
    #[inline]
    pub async fn before_dispatch_async<H: HookFamilies>(
        &self,
        request: &mut Request,
        state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        if !H::REQUEST && !H::ASYNC_REQUEST {
            return Ok(Flow::Continue);
        }
        for step in &self.request {
            let flow = match step {
                RequestStep::Sync(hook) => hook.before_dispatch(request, state)?,
                RequestStep::Async(hook) => hook.before_dispatch(request, state).await?,
            };
            match flow {
                Flow::Continue => {}
                stop => return Ok(stop),
            }
        }
        Ok(Flow::Continue)
    }

    /// Runs the request hooks in reverse declared order, read-only.
    ///
    /// Both kinds, and synchronously for both: `after_dispatch` has nothing
    /// to wait for. So a caller that used the async entry point still
    /// finishes the request with no runtime in hand.
    #[inline]
    pub fn after_dispatch<H: HookFamilies>(
        &self,
        request: &Request,
        response: &Response,
        state: &mut RequestState,
    ) {
        if !H::REQUEST && !H::ASYNC_REQUEST {
            return;
        }
        for step in self.request.iter().rev() {
            step.after_dispatch(request, response, state);
        }
    }

    /// Runs the response hooks in reverse declared order.
    #[inline]
    pub fn on_response<H: HookFamilies>(
        &self,
        request: &Request,
        response: &mut Response,
        state: &mut RequestState,
    ) -> Result<(), PluginError> {
        if !H::RESPONSE {
            return Ok(());
        }
        for hook in self.response.iter().rev() {
            hook.on_response(request, response, state)?;
        }
        Ok(())
    }

    /// Asks each error hook, in declared order, until one answers.
    ///
    /// Falls back to a bare 500 with no body, which is the only thing the core
    /// can say about an error it does not understand.
    #[inline]
    pub fn render_error<H: HookFamilies>(
        &self,
        error: &HandlerError,
        request: &Request,
        state: &mut RequestState,
    ) -> Response {
        if H::ERROR {
            for hook in &self.error {
                if let Some(response) = hook.on_error(error, request, state) {
                    return response;
                }
            }
        }
        bare_500()
    }

    /// One request, end to end.
    ///
    /// With `H = NoHooks` this is `dispatch(request, state)` and the error
    /// path, and nothing else survives compilation.
    pub fn run<H, F>(
        &self,
        request: &mut Request,
        state: &mut RequestState,
        dispatch: F,
    ) -> Response
    where
        H: HookFamilies,
        F: FnOnce(&mut Request, &mut RequestState) -> Result<Response, HandlerError>,
    {
        let outcome = match self.begin::<H>(request, state) {
            Begin::Dispatch => dispatch(request, state),
            Begin::Answered(outcome) => outcome,
        };
        self.finish::<H>(request, state, outcome)
    }

    /// The first half of [`Pipeline::run`]: the request hooks, and what they
    /// decided.
    ///
    /// This exists so that an **async** caller can put its own `.await`
    /// between the two halves without the core ever holding a future. The
    /// alternative was a `run_async` taking a higher-ranked closure that
    /// returns a boxed future, and that costs a second `Box::pin` per request
    /// on top of the handler's own, and an error message nobody can read. The
    /// caller writes:
    ///
    /// ```ignore
    /// // `ignore` rather than `no_run`: running this needs an executor, and
    /// // the core does not have one. That is the point of the split.
    /// let outcome = match app.begin(&mut request, &mut state) {
    ///     Begin::Dispatch => handler.call(&mut cx).await,
    ///     Begin::Answered(outcome) => outcome,
    /// };
    /// let response = app.finish(&mut request, &mut state, outcome);
    /// ```
    ///
    /// With `H = NoHooks` this is `Begin::Dispatch` and nothing else survives
    /// compilation, exactly as before. See `docs/async-handlers.md` §2.4.
    #[inline]
    pub fn begin<H: HookFamilies>(&self, request: &mut Request, state: &mut RequestState) -> Begin {
        match self.before_dispatch::<H>(request, state) {
            Ok(Flow::Continue) => Begin::Dispatch,
            Ok(Flow::Respond(response)) => Begin::Answered(Ok(*response)),
            Err(e) => Begin::Answered(Err(HandlerError::from(e))),
        }
    }

    /// [`Pipeline::begin`] for a caller that has a runtime.
    ///
    /// The one entry point that can run an [`AsyncRequestHook`], and the one
    /// an application that installed one must use. It runs the sync hooks
    /// too, in the single declared order, so a caller never needs both.
    ///
    /// [`Pipeline::finish`] is unchanged and still synchronous: nothing after
    /// dispatch waits.
    #[inline]
    pub async fn begin_async<H: HookFamilies>(
        &self,
        request: &mut Request,
        state: &mut RequestState,
    ) -> Begin {
        match self.before_dispatch_async::<H>(request, state).await {
            Ok(Flow::Continue) => Begin::Dispatch,
            Ok(Flow::Respond(response)) => Begin::Answered(Ok(*response)),
            Err(e) => Begin::Answered(Err(HandlerError::from(e))),
        }
    }

    /// The second half of [`Pipeline::run`]: error hooks, response hooks and
    /// `after_dispatch`, over whatever the handler produced.
    #[inline]
    pub fn finish<H: HookFamilies>(
        &self,
        request: &mut Request,
        state: &mut RequestState,
        outcome: Result<Response, HandlerError>,
    ) -> Response {
        let mut response = match outcome {
            Ok(response) => response,
            Err(error) => self.render_error::<H>(&error, request, state),
        };

        if self
            .on_response::<H>(request, &mut response, state)
            .is_err()
        {
            response = bare_500();
        }

        self.after_dispatch::<H>(request, &response, state);
        response
    }
}

/// What the request hooks left for the caller to do.
///
/// Not a `Result<Flow, _>`, because by this point a hook's error and a hook's
/// short-circuit are the same thing to the caller: an outcome to hand to
/// [`Pipeline::finish`].
pub enum Begin {
    /// No hook answered. Run the handler.
    Dispatch,
    /// A hook answered, or failed. Hand this to [`Pipeline::finish`].
    Answered(Result<Response, HandlerError>),
}

impl std::fmt::Debug for Begin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dispatch => f.write_str("Dispatch"),
            Self::Answered(Ok(r)) => write!(f, "Answered({})", r.status()),
            Self::Answered(Err(e)) => write!(f, "Answered(Err({e}))"),
        }
    }
}

impl std::fmt::Debug for Pipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pipeline")
            .field("request", &self.request_hooks())
            .field("async_request", &self.async_request_hooks())
            .field("response", &self.response.len())
            .field("error", &self.error.len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// The evidence
// ---------------------------------------------------------------------------

/// Named entry points that exist only so a test can read the code the compiler
/// generated for them.
///
/// `tests/hooks_cost_nothing.rs` asks `rustc` for the LLVM IR of this crate
/// and asserts that the `empty_*` functions below contain no call, no
/// allocation and no loop, while the `populated_*` ones do. That is the proof
/// of the claim in this module's header, and it is checked rather than
/// asserted in prose.
///
/// Not part of the public contract. `#[inline(never)]` so the symbol survives
/// to be read; nothing else in the crate calls them.
#[doc(hidden)]
pub mod probe {
    use std::future::Future;
    use std::task::{Context, Poll, Waker};

    use super::{
        Flow, HookFamilies, NoHooks, OnAsyncRequest, OnModel, OnRequest, OnResponse, Pipeline,
        Request, RequestState, Response,
    };
    use crate::plugin_api::PluginError;
    use crate::plugin_model::{ModelDispatch, ModelEvent};

    /// An event kind nobody hooks, for the model probe.
    pub struct Unhooked;
    impl ModelEvent for Unhooked {
        type Payload<'a> = ();
    }

    #[inline(never)]
    pub fn empty_before_dispatch(
        pipeline: &Pipeline,
        request: &mut Request,
        state: &mut RequestState,
    ) -> bool {
        before::<NoHooks>(pipeline, request, state)
    }

    #[inline(never)]
    pub fn populated_before_dispatch(
        pipeline: &Pipeline,
        request: &mut Request,
        state: &mut RequestState,
    ) -> bool {
        before::<OnRequest>(pipeline, request, state)
    }

    #[inline(never)]
    pub fn empty_async_before_dispatch(
        pipeline: &Pipeline,
        request: &mut Request,
        state: &mut RequestState,
    ) -> bool {
        before_async::<NoHooks>(pipeline, request, state)
    }

    #[inline(never)]
    pub fn populated_async_before_dispatch(
        pipeline: &Pipeline,
        request: &mut Request,
        state: &mut RequestState,
    ) -> bool {
        before_async::<OnAsyncRequest>(pipeline, request, state)
    }

    #[inline(never)]
    pub fn empty_on_response(
        pipeline: &Pipeline,
        request: &Request,
        response: &mut Response,
        state: &mut RequestState,
    ) -> bool {
        pipeline
            .on_response::<NoHooks>(request, response, state)
            .is_ok()
    }

    #[inline(never)]
    pub fn populated_on_response(
        pipeline: &Pipeline,
        request: &Request,
        response: &mut Response,
        state: &mut RequestState,
    ) -> bool {
        pipeline
            .on_response::<OnResponse>(request, response, state)
            .is_ok()
    }

    #[inline(never)]
    pub fn empty_after_dispatch(
        pipeline: &Pipeline,
        request: &Request,
        response: &Response,
        state: &mut RequestState,
    ) {
        pipeline.after_dispatch::<NoHooks>(request, response, state);
    }

    #[inline(never)]
    pub fn empty_model_emit(dispatch: &ModelDispatch<NoHooks>) -> bool {
        dispatch.emit::<Unhooked>(&mut ()).is_ok()
    }

    #[inline(never)]
    pub fn populated_model_emit(dispatch: &ModelDispatch<OnModel>) -> bool {
        dispatch.emit::<Unhooked>(&mut ()).is_ok()
    }

    fn before<H: HookFamilies>(
        pipeline: &Pipeline,
        request: &mut Request,
        state: &mut RequestState,
    ) -> bool {
        matches!(
            pipeline.before_dispatch::<H>(request, state),
            Ok(Flow::Continue)
        )
    }

    /// The async chain, polled once with a no-op waker.
    ///
    /// Polling once is enough for the measurement: with no family installed
    /// the future is ready on its first poll, which is the thing being
    /// proved. With one installed the first poll reaches the loop, which is
    /// the thing being proved there.
    fn before_async<H: HookFamilies>(
        pipeline: &Pipeline,
        request: &mut Request,
        state: &mut RequestState,
    ) -> bool {
        poll_once(pipeline.before_dispatch_async::<H>(request, state))
    }

    fn poll_once(future: impl Future<Output = Result<Flow, PluginError>>) -> bool {
        let mut cx = Context::from_waker(Waker::noop());
        matches!(
            std::pin::pin!(future).poll(&mut cx),
            Poll::Ready(Ok(Flow::Continue))
        )
    }
}

/// What the core says about an error nobody claimed.
fn bare_500() -> Response {
    let mut response = Response::new(Vec::new());
    *response.status_mut() = http::StatusCode::INTERNAL_SERVER_ERROR;
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_family_record_unions() {
        // In `const` blocks, because the whole point of the record is that
        // these are facts the compiler has before the program runs.
        const {
            assert!(!NoHooks::ANY);
            assert!(<Both<OnRequest, OnResponse>>::REQUEST);
            assert!(<Both<OnRequest, OnResponse>>::RESPONSE);
            assert!(!<Both<OnRequest, OnResponse>>::ERROR);
            assert!(<Both<OnRequest, Both<OnError, OnModel>>>::MODEL);
            assert!(!<Both<NoHooks, NoHooks>>::ANY);
        }
    }

    #[test]
    fn every_family_marker_is_zero_sized() {
        assert_eq!(std::mem::size_of::<NoHooks>(), 0);
        assert_eq!(std::mem::size_of::<OnRequest>(), 0);
        assert_eq!(std::mem::size_of::<Both<OnRequest, OnError>>(), 0);
    }

    #[test]
    fn an_empty_request_state_has_not_allocated() {
        let state = RequestState::new();
        assert!(state.is_empty());
    }

    #[test]
    fn request_state_is_keyed_by_type() {
        struct Started(u64);
        let mut state = RequestState::new();
        assert!(state.get::<Started>().is_none());
        state.insert(Started(7));
        assert_eq!(state.get::<Started>().unwrap().0, 7);
        assert_eq!(state.take::<Started>().unwrap().0, 7);
        assert!(state.get::<Started>().is_none());
    }

    #[test]
    fn a_handler_error_can_be_recognised_by_the_hook_that_owns_it() {
        #[derive(Debug, thiserror::Error)]
        #[error("not allowed")]
        struct Denied;

        let error = HandlerError::new(Denied);
        assert!(error.is::<Denied>());
        assert!(error.downcast_ref::<Denied>().is_some());
        assert!(error.downcast_ref::<PluginError>().is_none());
        assert_eq!(error.to_string(), "not allowed");
    }

    #[test]
    fn an_empty_pipeline_runs_only_the_handler() {
        let pipeline = Pipeline::default();
        let mut request = Request::new(Vec::new());
        let mut state = RequestState::new();
        let response = pipeline.run::<NoHooks, _>(&mut request, &mut state, |_, _| {
            Ok(Response::new(b"ok".to_vec()))
        });
        assert_eq!(response.body(), b"ok");
        assert!(state.is_empty());
    }

    #[test]
    fn an_empty_pipeline_still_answers_500_for_an_unclaimed_error() {
        let pipeline = Pipeline::default();
        let mut request = Request::new(Vec::new());
        let mut state = RequestState::new();
        let response = pipeline.run::<NoHooks, _>(&mut request, &mut state, |_, _| {
            Err(HandlerError::message("boom"))
        });
        assert_eq!(response.status(), http::StatusCode::INTERNAL_SERVER_ERROR);
    }
}
