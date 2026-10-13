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

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;

use crate::plugin_api::{Plugin, PluginError};
use crate::plugin_model::{ModelEvent, ModelHook, ModelHooks};
use crate::plugin_order::{HookId, Order, Ordered};

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

    /// Whether any family at all is installed.
    const ANY: bool = Self::REQUEST || Self::RESPONSE || Self::ERROR || Self::MODEL;
}

/// No hooks. Where every application starts, and where most of them stay.
pub struct NoHooks;

/// The plugin hooks the request: before dispatch, after dispatch, or both.
pub struct OnRequest;

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
}

impl HookFamilies for OnRequest {
    const REQUEST: bool = true;
    const RESPONSE: bool = false;
    const ERROR: bool = false;
    const MODEL: bool = false;
}

impl HookFamilies for OnResponse {
    const REQUEST: bool = false;
    const RESPONSE: bool = true;
    const ERROR: bool = false;
    const MODEL: bool = false;
}

impl HookFamilies for OnError {
    const REQUEST: bool = false;
    const RESPONSE: bool = false;
    const ERROR: bool = true;
    const MODEL: bool = false;
}

impl HookFamilies for OnModel {
    const REQUEST: bool = false;
    const RESPONSE: bool = false;
    const ERROR: bool = false;
    const MODEL: bool = true;
}

impl<A: HookFamilies, B: HookFamilies> HookFamilies for Both<A, B> {
    const REQUEST: bool = A::REQUEST || B::REQUEST;
    const RESPONSE: bool = A::RESPONSE || B::RESPONSE;
    const ERROR: bool = A::ERROR || B::ERROR;
    const MODEL: bool = A::MODEL || B::MODEL;
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
    request: &'a mut Vec<Ordered<Box<dyn RequestHook>>>,
    response: &'a mut Vec<Ordered<Box<dyn ResponseHook>>>,
    error: &'a mut Vec<Ordered<Box<dyn ErrorHook>>>,
    model: &'a mut ModelHooks,
}

impl<'a> HookRegistrar<'a> {
    pub(crate) fn new(
        instance: &'a str,
        request: &'a mut Vec<Ordered<Box<dyn RequestHook>>>,
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
            Box::new(hook) as Box<dyn RequestHook>,
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
    request: Vec<Box<dyn RequestHook>>,
    response: Vec<Box<dyn ResponseHook>>,
    error: Vec<Box<dyn ErrorHook>>,
}

impl Pipeline {
    pub(crate) fn from_parts(
        request: Vec<Box<dyn RequestHook>>,
        response: Vec<Box<dyn ResponseHook>>,
        error: Vec<Box<dyn ErrorHook>>,
    ) -> Self {
        Self {
            request,
            response,
            error,
        }
    }

    pub fn request_hooks(&self) -> usize {
        self.request.len()
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

    /// Runs the request hooks in declared order.
    ///
    /// `H` is the application's family record, so when no plugin installed a
    /// request hook the whole body folds away to `Ok(Flow::Continue)`.
    #[inline]
    pub fn before_dispatch<H: HookFamilies>(
        &self,
        request: &mut Request,
        state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        if !H::REQUEST {
            return Ok(Flow::Continue);
        }
        for hook in &self.request {
            match hook.before_dispatch(request, state)? {
                Flow::Continue => {}
                stop => return Ok(stop),
            }
        }
        Ok(Flow::Continue)
    }

    /// Runs the request hooks in reverse declared order, read-only.
    #[inline]
    pub fn after_dispatch<H: HookFamilies>(
        &self,
        request: &Request,
        response: &Response,
        state: &mut RequestState,
    ) {
        if !H::REQUEST {
            return;
        }
        for hook in self.request.iter().rev() {
            hook.after_dispatch(request, response, state);
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
    pub fn run<H, F>(&self, request: &mut Request, state: &mut RequestState, dispatch: F) -> Response
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

        if self.on_response::<H>(request, &mut response, state).is_err() {
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
            .field("request", &self.request.len())
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
    use super::{
        Flow, HookFamilies, NoHooks, OnModel, OnRequest, OnResponse, Pipeline, Request,
        RequestState, Response,
    };
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
