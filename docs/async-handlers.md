# Async handlers in the brocanth core

A proposal, with a prototype behind it.

The request was: *"we want to make handlers async to propagate async down the
stack. Maybe we can propose 2 types of handlers."* Two handler types is the
right instinct. This document says what it costs, what it breaks, and where the
propagation must stop.

Everything below was built and measured. The prototype is a scratch copy of
`crates/brocanth`. `crates/brocanth` itself is untouched.

---

## 1. Summary

| Question | Answer |
|---|---|
| Two handler types? | Yes. `Handler<E>` and `AsyncHandler<E>`, chosen at registration, in one router table behind an enum. |
| Does the sync path pay? | 0 extra allocations. 1 predictable branch. +8 bytes per handler slot. |
| Do middleware layers become async? | A second, separate stack. The existing one cannot be reused. |
| Do request hooks become async? | They can, and it is cheaper than expected. See §5 — this is the surprise. |
| Do response, error and `after_dispatch` hooks become async? | No. There is nothing for them to wait for. |
| Is `routing_middleware.rs:17-19` true? | **No.** It is a bug. See §4. |
| New dependencies? | None. `no_hidden_dependencies` still passes with three crates. |
| Does `hooks_cost_nothing` still pass? | Yes, including a new probe for the async hook family. |
| Test count | 159 before, 175 after, all pass. |

---

## 2. The proposed API

### 2.1 The two handler types

`Handler<E>` does not change. Not one character.

```rust
// plugin_routes.rs -- unchanged
pub type Handler<E> =
    Box<dyn Fn(&mut RouteCx<'_, E>) -> Result<Response, HandlerError> + Send + Sync>;
```

Two types are added beside it.

```rust
/// Std only. `core::future::Future` and `alloc::boxed::Box` are free.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub type AsyncHandler<E> = Box<
    dyn for<'a, 'r> Fn(&'a mut RouteCx<'r, E>)
        -> BoxFuture<'a, Result<Response, HandlerError>>
        + Send
        + Sync,
>;
```

The future borrows the `RouteCx`. A handler reads the request and writes the
request state across an `.await`, and nothing is cloned to make that work. This
is tested: `request_state_written_before_an_await_is_there_after_it`.

### 2.2 One table, not two

```rust
pub enum AnyHandler<E> {
    Sync(Handler<E>),
    Async(AsyncHandler<E>),
}

impl<E> AnyHandler<E> {
    /// Readable before the handler runs. The bridge uses it to choose a path.
    pub fn is_async(&self) -> bool;

    /// Runs a sync route, or reports that this one is not sync.
    /// For a caller with no runtime: a test, or a command that only inspects.
    pub fn call_sync(&self, cx: &mut RouteCx<'_, E>)
        -> Option<Result<Response, HandlerError>>;

    /// Runs the route, whichever kind it is.
    pub async fn call(&self, cx: &mut RouteCx<'_, E>)
        -> Result<Response, HandlerError>;
}

impl<E> From<Handler<E>> for AnyHandler<E> { .. }
```

**Why an enum and not two tables.** `routing.rs` already argues this case, for
`wayfind` over two `matchit` routers: *"405 needs one matcher. A route must
answer 'this path exists, that verb does not'."* Splitting the table by handler
kind puts one path in two places and turns the `Allow` set into a merge. The
same argument applies, so the same answer applies.

**Why the router itself needs no change at all.** `Router<H>` is already
generic over its payload. An application with no async route holds
`Router<Handler<E>>` and is bit-for-bit what it was. An application with a mix
holds `Router<AnyHandler<E>>`. `routing.rs` was not edited in the prototype.

### 2.3 Registration

Sync registration does not change. Async registration is a parallel set of
names.

```rust
impl<'a, E> RouteBundle<'a, E> {
    // unchanged, every one of them
    pub fn route(&mut self, name, verb, path, handler: Handler<E>) -> &mut Self;
    pub fn described(&mut self, name, verb, path, description, handler: Handler<E>) -> &mut Self;
    pub fn get(&mut self, name, path, handler: Handler<E>) -> &mut Self;
    pub fn post(..) pub fn put(..) pub fn patch(..) pub fn delete(..)

    // new
    pub fn route_async(&mut self, name, verb, path, handler: AsyncHandler<E>) -> &mut Self;
    pub fn described_async(&mut self, name, verb, path, description, handler: AsyncHandler<E>) -> &mut Self;
    pub fn get_async(&mut self, name, path, handler: AsyncHandler<E>) -> &mut Self;
    pub fn post_async(..) pub fn put_async(..) pub fn patch_async(..) pub fn delete_async(..)
}
```

The kind is decided once, at registration, and never again. There is no handler
that is sync on Tuesday.

One field changes type:

```rust
pub struct ContributedRoute<E> {
    ..
    pub handler: AnyHandler<E>,   // was: Handler<E>
}
```

A plugin that writes sync routes compiles unchanged, because `get` still takes
`Handler<E>` and wraps it internally. Only a consumer that *reads*
`ContributedRoute::handler` sees the new type. §8 counts how many there are.

### 2.4 The pipeline, split in two

This is the part that makes the whole thing possible without the core ever
holding a future.

```rust
pub enum Begin {
    /// No hook answered. Run the handler.
    Dispatch,
    /// A hook answered, or failed. Hand this to `finish`.
    Answered(Result<Response, HandlerError>),
}

impl Pipeline {
    pub fn begin<H: HookFamilies>(&self, request: &mut Request, state: &mut RequestState) -> Begin;

    pub fn finish<H: HookFamilies>(
        &self,
        request: &mut Request,
        state: &mut RequestState,
        outcome: Result<Response, HandlerError>,
    ) -> Response;

    // unchanged, and now implemented over the two above
    pub fn run<H, F>(&self, request: &mut Request, state: &mut RequestState, dispatch: F) -> Response;
}
```

`App::begin` and `App::finish` forward to them with the application's `H`
already applied, exactly as `App::run` does.

The caller writes:

```rust
let outcome = match app.begin(&mut request, &mut state) {
    Begin::Dispatch => handler.call(&mut cx).await,
    Begin::Answered(outcome) => outcome,
};
let response = app.finish(&mut request, &mut state, outcome);
```

**The rejected alternative.** A `Pipeline::run_async` taking a closure that
returns a future. It needs a higher-ranked bound, because the closure's `&mut`
borrows must end before `finish` reborrows them:

```rust
F: for<'c> FnOnce(&'c mut Request, &'c mut RequestState)
    -> BoxFuture<'c, Result<Response, HandlerError>>
```

That boxes the dispatch future, so an async route pays **two** `Box::pin` per
request instead of one, and a sync route pays one where it used to pay none.
The split costs nothing and reads better. It is also honest: the core says
"here are the two halves, you own the middle", which is what a core with no
runtime should say.

### 2.5 Async middleware

A second stack, because the existing one cannot carry a future. §4 proves why.

```rust
pub trait AsyncLayer<C, O>: Send + Sync {
    fn call<'a>(
        &'a self,
        ctx: &'a mut C,
        next: AsyncNext<'a, C, O>,
    ) -> BoxFuture<'a, (O, &'a mut C)>;
}

pub struct AsyncNext<'a, C, O> {
    rest: &'a [&'a dyn AsyncLayer<C, O>],
    terminal: &'a (dyn Fn(&'a mut C) -> BoxFuture<'a, (O, &'a mut C)> + Send + Sync),
}

impl<'a, C, O> AsyncNext<'a, C, O> {
    pub fn run(self, ctx: &'a mut C) -> BoxFuture<'a, (O, &'a mut C)>;
    pub const fn remaining(&self) -> usize;
}

pub struct AsyncMiddlewareStack<C, O> { .. }

impl<C, O> AsyncMiddlewareStack<C, O> {
    pub fn use_at(&mut self, pattern: &str, layer: impl AsyncLayer<C, O> + 'static)
        -> Result<&mut Self, TemplateError>;
    pub fn use_scoped(&mut self, scope: Scope, layer: impl AsyncLayer<C, O> + 'static)
        -> Result<&mut Self, TemplateError>;
    pub fn patterns_for(&self, path: &str) -> Vec<&str>;

    /// The caller holds the chain for as long as the future it builds.
    pub fn chain_for(&self, path: &str) -> Vec<&dyn AsyncLayer<C, O>>;

    pub fn run_chain<'a>(
        chain: &'a [&'a dyn AsyncLayer<C, O>],
        ctx: &'a mut C,
        terminal: &'a (dyn Fn(&'a mut C) -> BoxFuture<'a, (O, &'a mut C)> + Send + Sync),
    ) -> BoxFuture<'a, (O, &'a mut C)>;
}

/// Forces a terminal closure to be checked against the higher-ranked bound.
pub fn async_terminal<C, O, F>(f: F) -> F
where
    F: for<'a> Fn(&'a mut C) -> BoxFuture<'a, (O, &'a mut C)> + Send + Sync;
```

Three things are different from the sync stack, and each is forced.

1. **`run` returns `(O, &mut C)`, not `O`.** The future borrows the context for
   `'a`, and `'a` outlives the call. Without handing the context back, a layer
   could touch it before `next` and never again — so a logger could not write
   its own timing on the way out. The sync `Next::run` needs none of this,
   because its reborrow ends when the call does.
2. **`chain_for` is separate from `run_chain`.** `MiddlewareStack::run`
   collects the matching layers into a `Vec` local to itself. That is sound
   because the call finishes before `run` returns. A future outlives the call
   that made it, so the caller must hold the `Vec`. The allocation count is the
   same — the sync stack already allocates one `Vec` per request when any layer
   matches — but it moves into view.
3. **No blanket impl for closures.** The sync side has
   `impl<C, O, F: Fn(..)> Layer<C, O> for F`, so a closure is a layer. The
   async side cannot: a closure returning `Box::pin(async move { .. })` is
   inferred at one concrete lifetime and then fails the higher-ranked bound. An
   async layer is a named type with an `impl` block. That is a real ergonomic
   loss, and it is the main reason not to make layers async until something
   needs it.

The three behavioural rules survive, and are tested against the async stack:
registration order, short-circuit by not calling `run`, and `except` beating
the pattern.

---

## 3. What becomes async, and what does not

| Seam | Async? | Reason |
|---|---|---|
| Route handler | **Yes**, per route | The whole point. A handler waits on a database, an HTTP call, a file. |
| Middleware layer | **Yes**, separate stack | An auth layer reads a session store. But see §3.1 — nothing in the request path uses the stack today. |
| `RequestHook::before_dispatch` | **Yes, optionally** | A session loader needs I/O before the handler. §5 shows this costs nothing when nobody registers one. |
| `RequestHook::after_dispatch` | **No** | It cannot fail, cannot change the response, and runs after the response is final. Awaiting there holds the connection open to do work the client is not waiting for. A plugin that wants that wants a job queue. |
| `ResponseHook::on_headers` / `on_body` | **No** | They rewrite headers, and a `Vec<u8>` that is already in memory. Compression, ETag and partial-navigation headers are CPU, not I/O. There is nothing to wait for. A streaming body is not an exception: the hooks run before the first byte, and the waiting is the transport's. |
| `ErrorHook::on_error` | **No** | It recognises an error by type and builds a response. Same argument. An error hook that wants to *report* to a remote service should hand the report to a queue, not hold the 500 open. |
| `ModelHook` | **No** | The ORM owns its own write path and is already async if it wants to be. The core forwards a value it never looks inside. Making the forwarding async puts a future in the ORM's hot loop for no gain. |
| `Plugin::start` / `shutdown` | **No** (out of scope) | Separate question. Worth asking later; not asked here. |

### 3.1 A fact worth knowing before deciding about middleware

`MiddlewareStack` is **not wired into the request path**. `bridge.rs` never
calls it. `MiddlewareCatalog::offer` takes a name and a description and no
layer at all:

```rust
// brocanth-auth/src/plugin.rs:301
catalog.offer(
    "auth.session",
    "reads the signed session cookie, refreshes its idle timer, ..",
);
```

So `AsyncMiddlewareStack` can be added, or not added, without affecting
anything that runs today. The cheap decision is to add it and leave it unused
until the catalogue carries layers. The cheaper decision is to not add it yet
and fix the false doc comment, which is the actual bug. See §11.

---

## 4. `routing_middleware.rs:17-19` is wrong

The claim:

> The stack holds no runtime and spawns nothing. `O` is whatever the
> application's handler returns, so a future-returning handler works later
> without changing anything here.

It does not work, and the reason is structural rather than a missing bound.

A layer that awaits must hold `Next` across the `.await`, because
`next.run(ctx)` is the thing it awaits. So the future borrows the chain, and
its type must name that borrow's lifetime. But `O` is a **type parameter of the
trait**, fixed when the stack is built. The chain's borrow does not exist yet
at that point, so `O` cannot name it.

Written out and compiled:

```rust
type Fut<'a> = Pin<Box<dyn Future<Output = u32> + 'a>>;

fn attempt<'f>() {
    let mut stack: MiddlewareStack<(), Fut<'f>> = MiddlewareStack::new();
    stack.use_at("/{*rest}", |ctx: &mut (), next: Next<'_, (), Fut<'f>>| {
        Box::pin(async move { next.run(ctx).await + 1 }) as Fut<'_>
    }).expect("pattern");
}
```

```text
error: lifetime may not live long enough
  --> src/main.rs:15:13
   |
14 | .use_at("/{*rest}", |ctx: &mut (), next: Next<'_, (), Fut<'f>>| {
   |                           -                                   - return type of
   |                           |                                     closure is Fut<'2>
   |                           let's call the lifetime of this reference `'1`
15 |     Box::pin(async move { next.run(ctx).await + 1 })
   |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ returning this value requires
   |                                                     that `'1` must outlive `'2`

error: lifetime may not live long enough
   | ..  has type `Next<'3, (), Pin<Box<dyn Future<Output = u32>>>>`
   | ..  returning this value requires that `'3` must outlive `'2`
```

`'1` is the context borrow, `'3` is the chain borrow, `'2` is the lifetime
baked into `O`. Both must outlive `'2`, and neither can, because `'2` was fixed
first.

**A second, independent blocker** sits under it. `Next` holds
`terminal: &'a dyn Fn(&mut C) -> O`. `dyn Fn` is not `Sync`, so a future
holding a `Next` is not `Send`, even where every lifetime does line up:

```text
error: future cannot be sent between threads safely
   = help: the trait `Sync` is not implemented for
           `dyn for<'a> Fn(&'a mut ()) -> Pin<Box<dyn Future<Output = u32> + Send>>`
note: captured value is not `Send`
   |     Box::pin(async move { next.run(ctx).await + 1 })
   |                           ^^^^ has type `Next<'_, (), ..>` which is not `Send`
```

**What fixes both** is a trait whose output type is lifetime-parametric: a
method-level `'a` and a concrete return type, not a trait parameter. That is
`AsyncLayer` in §2.5. Tower arrives at the same place from the other side. Its
`Service::Future` is an **associated** type, and a Tower layer **owns** its
inner service rather than borrowing a chain — `Layer<S> -> S'` composes types
at build time, `call(&mut self, req)` takes the request by value, and
`BoxCloneService` erases to a `'static` future. Nothing Tower awaits is a
borrow of the stack, so the problem never arises.

brocanth cannot copy that wholesale, because `Layer::call` takes `&mut C` and
Tower takes `Req` by value. Owning the context would mean moving it in and out
of every layer, or an `Arc` per request. The method-lifetime form keeps the
borrow and costs a tuple in the return type instead. That is the trade this
proposal takes.

**The doc comment should be deleted or corrected regardless of whether async
handlers ship.** It is the kind of claim that gets planned against.

---

## 5. The surprise: an async hook family is free when nobody registers it

This was expected to be the thing that broke. It is not.

`tests/hooks_cost_nothing.rs` reads the LLVM IR and asserts that an
unregistered hook family compiles to a bare `ret`. The prototype adds a fifth
family — `ASYNC_REQUEST` on `HookFamilies`, a marker `OnAsyncRequest`, a trait
`AsyncRequestHook`, a `Vec` on `Pipeline`, and an entry point — then adds
probes for it to `MUST_BE_FREE` and `MUST_NOT_BE_FREE`.

```rust
pub trait HookFamilies: 'static {
    const REQUEST: bool;
    const RESPONSE: bool;
    const ERROR: bool;
    const MODEL: bool;
    const ASYNC_REQUEST: bool = false;
    const ANY: bool = Self::REQUEST || Self::RESPONSE || Self::ERROR
        || Self::MODEL || Self::ASYNC_REQUEST;
}

pub trait AsyncRequestHook: Send + Sync + 'static {
    fn before_dispatch<'a>(
        &'a self,
        request: &'a mut Request,
        state: &'a mut RequestState,
    ) -> BoxFuture<'a, Result<Flow, PluginError>>;
}

impl Pipeline {
    pub async fn before_dispatch_async<H: HookFamilies>(
        &self, request: &mut Request, state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        if !H::ASYNC_REQUEST { return Ok(Flow::Continue); }
        for hook in &self.async_request {
            match hook.before_dispatch(request, state).await? {
                Flow::Continue => {}
                stop => return Ok(stop),
            }
        }
        Ok(Flow::Continue)
    }
}
```

The generated IR, read out of the probe build:

```llvm
@_RNv..probe27empty_async_before_dispatch = unnamed_addr alias i1 (ptr, ptr, ptr),
    ptr @_RNv..probe21empty_before_dispatch

; brocanth::plugin_hooks::probe::empty_before_dispatch
define noundef zeroext i1 @_RNv..probe21empty_before_dispatch(..) {
start:
  ret i1 true
}
```

LLVM folded the empty async probe into the empty **sync** probe, because they
compile to identical instructions. The empty async family is not merely small.
It is byte-identical to a case already proved free.

The populated one is a full generator, so the comparison is not measuring
nothing:

```llvm
; brocanth::plugin_hooks::probe::populated_async_before_dispatch
define .. {
bb3.i.i:
  %_16.i.i = alloca [24 x i8], align 8
  %pinned.i = alloca [80 x i8], align 8
  %cx.i = alloca [32 x i8], align 8
  call void @llvm.lifetime.start.p0(ptr nonnull %cx.i), ..
```

**One change to the test was needed, and it is a fix rather than a weakening.**
`function_body` looked only for a `define` line, so an alias read as "the probe
moved" and failed. It now follows aliases:

```rust
fn function_body(ir: &str, name: &str) -> Option<String> {
    for line in ir.lines() {
        let Some((lhs, rhs)) = line.split_once(" = ") else { continue };
        if !lhs.starts_with('@') || !lhs.contains(name) || !rhs.contains(" alias ") {
            continue;
        }
        let target = rhs.rsplit_once("ptr @")?.1.trim();
        return function_body(ir, target);
    }
    body_of_define(ir, name)
}
```

### 5.1 What this result does and does not mean

It means the **mechanism** survives. A fifth family costs nothing to an
application that does not use it.

It does not mean async request hooks are cheap to *use*. A registered one costs
a `Box::pin` per hook per request, and the `Vec<Box<dyn AsyncRequestHook>>` sits
on `Pipeline` whether or not anyone fills it — 24 bytes of a struct that is
built once.

It also does not mean a family can be "async only when registered" in the sense
of one trait that is sometimes sync. That is still hard, and the prototype does
not do it. `OnRequest` and `OnAsyncRequest` are **two families**, two traits and
two `Vec`s. A plugin declares which one it is. An application that installs both
runs the sync hooks and then the async hooks. That is a real seam in the
contract, and whoever writes the first async hook has to be told which list
their hook lands in and in what order relative to the other list. Say it in the
docs or it will be found by surprise.

My recommendation: **ship the two handler types first, and the async request
hook family second, as a separate change.** The evidence says it is affordable.
It is still a widening of the plugin contract, and it should not ride in on a
router change.

---

## 6. The cost, stated plainly

### 6.1 Allocations per request, measured

`tests/async_costs.rs` uses the same counting global allocator as
`hooks_cost_nothing.rs`. Handlers that return `Response::new(Vec::new())`
allocate nothing of their own, so every count is machinery.

| Path | Allocations | Test |
|---|---|---|
| Sync route, called directly | 0 | baseline |
| Sync route, through `AnyHandler::call_sync` | **0** | `a_sync_route_through_the_enum_allocates_exactly_what_it_did_before` |
| Sync route, awaited through `AnyHandler::call` | **0** | `a_sync_route_awaited_through_the_uniform_call_still_allocates_nothing` |
| Async route | **1** | `an_async_route_costs_one_allocation_for_its_future` |

One `Box::pin`, for async routes only. The number is asserted exactly, so a
later change that quietly adds a second one fails the test.

An async middleware chain adds one `Box::pin` per layer per request, plus the
`Vec` from `chain_for` — which the sync stack already pays.

### 6.2 Size

| Type | Bytes |
|---|---|
| `Handler<E>` | 16 |
| `AsyncHandler<E>` | 16 |
| `AnyHandler<E>` | **24** |
| `Option<Handler<E>>` | 16 |
| `Option<AnyHandler<E>>` | 24 |

`Router` stores `[Option<H>; 7]` per route, so a route grows from 112 bytes to
168. For a thousand routes that is 56 KB, once, at build time. It is not
nothing and it is not a reason to decide anything.

### 6.3 Bounds that propagate

- **`Send` on every handler future.** `BoxFuture` carries `+ Send`, because a
  bridged request crosses a task boundary. A handler that holds an `Rc`, a
  `RefCell` guard or a non-`Send` database cursor across an `.await` does not
  compile. The error points at the `Box::pin`, not at the `Rc`.
- **`Send + Sync` on the async middleware terminal.** `AsyncNext::terminal` is
  `&(dyn Fn(..) + Send + Sync)`, not `&dyn Fn(..)`, for the reason §4 gives.
  That bound reaches every terminal a caller writes.
- **`C: Send` on an async middleware context.** `&'a mut C` is in a `Send`
  future, so `C` must be `Send`.
- **`'a` on everything in `AsyncLayer`.** `&'a self, ctx: &'a mut C` is
  stricter than the sync `&self, ctx: &mut C`. A layer cannot hold a borrow of
  itself for a shorter time than the chain.

### 6.4 What gets harder to write

**The context handback.** An async layer reads
`let (out, ctx) = next.run(ctx).await;` where the sync one reads
`next.run(ctx);`. It is explainable but it is one more thing to explain, and
forgetting it produces a borrow error, not a compile-time nudge toward the
right shape.

**No closure layers.** The sync side gets `impl Layer for F: Fn(..)` free. The
async side does not. Every async layer is a struct and an `impl` block.

**Closure inference, in exactly one place.** This turned out better than
expected, and the honest version is worth recording because the pessimistic
version would have been easy to write.

A plain boxed closure **works** wherever the expected type is already known:

```rust
// compiles, no helper
bundle.get_async("slow", "/slow", Box::new(|cx: &mut RouteCx<'_, Base>| {
    Box::pin(async move {
        work().await;
        let n = cx.request().body().len();
        cx.state_mut().insert(n);
        Ok(Response::new(Vec::new()))
    })
}));
```

The expected type drives the closure's signature, and the higher-ranked bound
is satisfied. The same holds for a `let` with an `AsyncHandler<E>` annotation.
`async_handler` is therefore a convenience for route handlers, not a necessity.

It is a necessity for the **middleware terminal**, where the expected type is a
generic `F` and there is nothing to drive inference:

```rust
// does NOT compile
let terminal = |c: &mut Trace| -> BoxFuture<'_, ((), &mut Trace)> {
    Box::pin(async move { ((), c) })
};
```

```text
error: lifetime may not live long enough
   |     let terminal = |c: &mut Trace| -> BoxFuture<'_, ()> {
   |                        -              ----------------- return type of closure is
   |                        |                                Pin<Box<dyn Future + Send + '2>>
   |                        let's call the lifetime of this reference `'1`
   | /     Box::pin(async move {
   | |         note(c, "handler");
   | |     })
   | |______^ returning this value requires that `'1` must outlive `'2`
```

This is the famously unhelpful one. It points at a `Box::pin` that is correct,
names two anonymous lifetimes, and suggests nothing. `async_terminal` exists to
turn it into a bound violation with a name on it.

### 6.5 Error messages that get worse

- The `lifetime may not live long enough` above, for any generic position.
- `future cannot be sent between threads safely`, with a `note: captured value
  is not Send` pointing at a local inside an `async move` block. Rust has got
  better at this; it is still three screens when the offending value is behind
  two layers of `impl Trait`.
- `type annotations needed` with `multiple impls satisfying for<'a, 'r> _: Fn(..)
  found in the following crates: alloc, core`, listing `&F`, `Box<F>` and
  `SyncView<F>`. Produced by the prototype while writing §6.4. It is accurate
  and it is not a sentence anyone reads happily.

### 6.6 What is not a cost

No new dependency. `cargo tree` on the prototype:

```
brocanth v0.1.0
├── http v1.5.0
├── thiserror v2.0.21
└── wayfind v1.1.2
```

The core holds no runtime, spawns nothing, and polls nothing. It hands the
caller a future and a seam to put an `.await` in. `no_hidden_dependencies`
passes unchanged.

---

## 7. What the bridge becomes

`crates/brocanth-server/src/bridge.rs` runs **every** request on
`tokio::task::spawn_blocking`, because the core is sync. Its own header states
the cost: *"One concurrent core request is one OS thread. Tokio's blocking pool
grows to 512 threads by default and then queues."* The measured number in the
brief is 50 concurrent 100 ms handlers in 113 ms, with one OS thread each.

With two handler types the bridge chooses per request, and it can choose
**before touching the handler**, because `AnyHandler::is_async` is readable
from the `Dispatch::Found` arm.

```rust
pub async fn handle(self: &Arc<Self>, request: Request) -> Response {
    let Ok(mut request) = to_core(request) else {
        return Response::text(400, "Bad Request");
    };

    // The hooks are sync and cheap. They run here, on the worker.
    let outcome = match self.app.begin(&mut request, &mut state) {
        Begin::Answered(outcome) => outcome,
        Begin::Dispatch => match self.route(&request) {
            // An async route: no thread, no spawn, no 'static bound.
            Some(h) if h.is_async() => h.call(&mut cx).await,
            // A sync route: exactly the thread it has today.
            Some(h) => {
                let bridge = Arc::clone(self);
                tokio::task::spawn_blocking(move || bridge.blocking_route(..)).await?
            }
            None => Ok(plain(404, "Not Found")),
        },
    };

    from_core(self.app.finish(&mut request, &mut state, outcome))
}
```

What changes:

- **An async route costs no OS thread.** The 50-concurrent-handler case becomes
  50 futures on the worker pool. This is the point of the whole exercise.
- **A sync route keeps `spawn_blocking` and keeps every word of the module's
  current justification.** No regression, and no need to rewrite the table of
  who answers what.
- **`App::run` is replaced by `begin` + `finish` in the bridge only.** The 404
  and 405 are still built inside the dispatch region, so response hooks and
  `after_dispatch` still see them.

What gets harder:

- **The panic story splits.** Today a panicking handler comes back as a
  `JoinError` and becomes a 500 for that request. An inline `.await` has no such
  net: a panicking async handler unwinds into hyper's connection task and the
  client gets a closed socket. The bridge must wrap the async arm in
  `AssertUnwindSafe(..).catch_unwind()` to keep the current behaviour, and
  `catch_unwind` across an `.await` is a correctness question of its own —
  a future that panicked mid-poll must not be polled again. This is the single
  largest piece of work the bridge change implies, and it is not in the
  prototype.
- **`Send + 'static` splits too.** Today everything crosses a `spawn_blocking`
  boundary, so everything is already `Send + 'static`. The async arm needs
  `Send` but not `'static`, which is strictly easier — but it means two
  different sets of requirements in one function, and the compiler will explain
  the difference badly.

Noted and left alone, as instructed: the bridge copies the body twice,
`Bytes -> Vec<u8>` on the way in and `Vec<u8> -> Bytes` on the way out, because
`Request`/`Response` are `http::{Request,Response}<Vec<u8>>`. Async makes that
copy more visible, not worse. It is a separate change.

---

## 8. What breaks for an existing plugin

**One call site, in the whole core test suite.** It is a *reader* of the plan,
not a writer:

```rust
// tests/plugin_hooks.rs:756, before
let response = (route.handler)(&mut cx).unwrap();
// after
let response = route.handler.call_sync(&mut cx).expect("a sync route").unwrap();
```

That is the entire source breakage in 9,690 lines of core plus its tests.

**Nothing else in the workspace reads `ContributedRoute::handler`.** A grep for
`Router::builder|route_plan()|ContributedRoute` outside `crates/brocanth`
returns `brocanth-server`, `brocanth-openapi` and `brocanth-cli`, and every one
of them builds a `Router<T>` with its own `T` — `Handler<Base>`, `Format`,
`&'static str`. `Router<H>` is unchanged, so all of them compile unchanged.

**A plugin that writes sync routes does not change a line.** `bundle.get`,
`bundle.post`, `bundle.described` and the rest still take `Handler<E>`. The
wrapping happens inside. Proved by
`one_plugin_contributes_a_sync_route_and_an_async_route_in_one_bundle`, whose
sync route is written exactly as it was before async existed.

### 8.1 Do `brocanth-auth`'s two bodyless middleware names become writable?

**`auth.require_authenticated`: yes, and it did not need async.**
It reads the session off the `RequestState`, and answers 401 or continues. It
is pure CPU. What it is waiting for is not a runtime — it is a
`MiddlewareCatalog` that carries a layer instead of a name, and a `before_dispatch`
hook that already ran. Both are missing today for reasons that have nothing to
do with async.

**`auth.session`: yes, and this one is the real case for async.**
It reads the signed session cookie, looks the session up in a store, refreshes
its idle timer, and puts the account on the context. The lookup and the refresh
are I/O against whatever `SessionStore` the application supplied. Today that
work can only happen on a blocked thread.

But look at where it belongs. `auth.session` runs before dispatch, on every
request in its scope, and it writes to the context. That is
`RequestHook::before_dispatch`, not a middleware layer — the plugin's own header
says so: *"Request hooks are what the middleware bodies need, and they land with
the core's HTTP layer."*

So the thing that unblocks `brocanth-auth` is **§5's async request hook family**,
not §2.5's async middleware stack. That is worth knowing before building the
harder of the two.

The honest caveat: §5 proves the family is free when unused and demonstrates
the trait. It does not wire `AsyncRequestHook` into `HookRegistrar`,
`HookSource::Families` or `AppBuilder::plugin_hooking`. That is the remaining
work, and it is ordinary work, not research.

---

## 9. Streaming responses

**Built since this section was written.** What it predicted was right, so the
prediction is kept above the correction.

The blocker was never the type of a handler's return value. It was
`ResponseHook::on_response(&self, request, response: &mut Response, state)`,
which was specified to see a **complete body**: compression compresses a whole
`Vec<u8>`, an ETag is a hash of a whole `Vec<u8>`, and both are documented as
running inside-out so that compression wraps the ETag. A streaming body made
`&mut Response` meaningless for both.

*Correction: that is what the change did.* `Response` is now
`http::Response<brocanth::plugin_body::Body>`, where a `Body` is either every
byte or a `BodyStream` the transport polls, and `ResponseHook` is two methods
rather than one:

* `on_headers(&self, request, head: &mut ResponseHead, body: BodyShape<'_>, state)`
  runs for **every** response, before the first byte, a stream included.
* `on_body(&self, request, head: &mut ResponseHead, body: &mut Vec<u8>, state)`
  runs only for a body that is complete.

So a hook is never handed an unfinished body. Compression sets its header in
`on_headers` and either rewrites bytes in `on_body` or wraps the stream with
`Streaming::wrap`; the partial-navigation headers are head-only and reach a
stream unchanged; `ETag` over a stream is unavailable and is stated to be,
rather than computed over nothing.

The failure story is the one this section predicted and it has not improved:
once the first byte is out there is no clean 500. A stream that fails mid-body
stops writing and the client sees a truncated response, which is why PLAN.md
§3 buffers by default and why streaming is opt-in per response.

Async was indeed a prerequisite rather than a step: the core still holds no
future and polls no body. `StreamGate` is the shutdown half — `close()` is
ADR 2's `begin_shutdown` and calls `BodyStream::finish`, and
`StreamGate::drained()` is what the transport awaits.

---

## 10. Migration path

> **Status, 2026-10-13: steps 1 to 5 are landed.** Only step 6 remains, and it
> still has no caller. Three steps turned out differently from the plan, and
> the correction sits under the claim that was wrong rather than only in §13.

There is one, and it is unusually cheap, because `Handler<E>` does not change
and `Router<H>` does not change.

**Step 1 — correct the lie.** Fix or delete `routing_middleware.rs:17-19`.
This is worth doing on its own, today, whatever happens to the rest.
*Landed in `47537ee`. The comment was replaced rather than deleted, with both
compiler errors, because the claim is the kind people plan against.*

**Step 2 — the two handler types.** `BoxFuture`, `AsyncHandler`, `AnyHandler`,
`*_async` on `RouteBundle`, `ContributedRoute::handler: AnyHandler<E>`. One
call site changes in the test suite. No plugin changes. No router changes.
*Landed in `47537ee`, exactly as described.*

**Step 3 — split the pipeline.** `Begin`, `Pipeline::begin`,
`Pipeline::finish`, `App::begin`, `App::finish`. `Pipeline::run` keeps working
and is implemented over the two halves. Purely additive.
*Landed in `47537ee`, exactly as described.*

**Step 4 — the bridge.** Choose per request on `is_async`. Keep `spawn_blocking`
for sync routes. **Solve the panic story for the async arm before shipping it**;
this is the one step with real unknowns.
*Landed in `0e27087`. The panic is catchable — see §7, now rewritten — but
three other things in that section were wrong: the hooks must **not** run on
the worker, the bridge cannot simply hold a `Router<AnyHandler<E>>`, and
reading `is_async` is not free. The measured result:*

```
async: 50 x 100ms in 107ms, peak 50 concurrent on  2 thread(s)
 sync: 50 x 100ms in 115ms, peak 50 concurrent on 50 thread(s)
```

**Step 5 — the async request hook family**, if `brocanth-auth` wants
`auth.session` written. Wire `AsyncRequestHook` through `HookRegistrar`,
`HookSource::Families` and `plugin_hooking`. Document the ordering between the
sync list and the async list, because it is a seam a plugin author will trip
over.
*Landed in `e70adcd`, and the seam does not exist: there are no longer two
lists. One `Vec<RequestStep>` and one sort, so `after("session")` is obeyed
whichever kind `session` is — including a **sync** hook ordered after an
**async** one, which no two-list design can satisfy at all.*

**Step 6 — async middleware**, if and when `MiddlewareCatalog` carries layers
rather than names. Not before. There is no caller today.
*Not landed. Still no caller.*

Steps 1 to 3 are reversible and testable on their own. Step 4 is the one to
plan time for. Steps 5 and 6 are optional and independent of each other.

*Correction to that last sentence: step 5 is independent of step 6 but is
**gated on step 4** for any real application. An application that installs an
async request hook can no longer call `App::run` — that is a compile error, by
design — so it needs the bridge. The document listed 5 as optional and
independent; it is neither, for anyone who actually wants `auth.session`.*

---

## 11. State of the prototype

Location: `/tmp/async-proto-brocanth`, a copy of `crates/brocanth` made
standalone by adding `[workspace]` to its manifest. `crates/brocanth` was not
modified.

### What compiles and passes

```
     Running unittests src/lib.rs
test result: ok. 118 passed; 0 failed
     Running tests/async_costs.rs
test result: ok. 4 passed; 0 failed
     Running tests/async_handlers.rs
test result: ok. 12 passed; 0 failed
     Running tests/hooks_cost_nothing.rs
test result: ok. 3 passed; 0 failed
     Running tests/no_hidden_dependencies.rs
test result: ok. 3 passed; 0 failed
     Running tests/plugin_hooks.rs
test result: ok. 25 passed; 0 failed
   Doc-tests brocanth
test result: ok. 10 passed; 0 failed; 1 ignored
```

**175 passing, 6 binaries.** Baseline was 159 across 5. The 16 new tests are 12
behavioural and 4 allocation counts. `cargo clippy --all-targets` reports zero
warnings.

The one ignored doc test is the `Begin` usage example on `Pipeline::begin`. It
is `ignore` rather than `no_run` because running it needs an executor, and the
core has none. All 10 original doc tests still run and pass.

Constraint checks:

- **`no_hidden_dependencies`: passes.** Three dependencies, unchanged. The ban
  list still names `futures`, `tokio`, `async-std` and the rest, and none is
  reachable.
- **`hooks_cost_nothing`: passes**, with the async family added to
  `MUST_BE_FREE`. One change to the test's IR reader, to follow aliases — §5
  explains why that strengthens rather than weakens it.
- **159 original tests: all pass.** One of them has one line changed, §8.

No test driving an async route uses a runtime. `tests/async_handlers.rs` ships
a twenty-line `block_on` built from `std::task::Wake` and `thread::park`. A test
that reached for tokio would be testing the wrong thing, and
`no_hidden_dependencies` would fail the build anyway.

### What is in the prototype

- `plugin_routes.rs`: `BoxFuture`, `AsyncHandler`, `async_handler`,
  `AnyHandler` with `is_async`/`call_sync`/`call`, `route_async`,
  `described_async`, `get_async`/`post_async`/`put_async`/`patch_async`/`delete_async`,
  `ContributedRoute::handler: AnyHandler<E>`.
- `plugin_hooks.rs`: `Begin`, `Pipeline::begin`, `Pipeline::finish`,
  `Pipeline::run` rebuilt over them. Plus the §5 experiment: `ASYNC_REQUEST`,
  `OnAsyncRequest`, `AsyncRequestHook`, `Pipeline::before_dispatch_async`, and
  two probes.
- `plugin_app.rs`: `App::begin`, `App::finish`.
- `routing_middleware.rs`: `AsyncLayer`, `AsyncNext`, `AsyncMiddlewareStack`,
  `async_terminal`, and the module header corrected with the compiler's own
  output.
- `routing.rs`: four new re-exports. No other change.

### What is not in the prototype, and why

1. **`AsyncRequestHook` is not wired into registration.** The trait, the family
   constant, the `Pipeline` field and the entry point exist and are probed.
   `HookRegistrar::request_async`, `HookSource::Families` recognising
   `OnAsyncRequest`, and `AppBuilder::plugin_hooking` widening on it are not
   written. The cost question is answered; the plumbing is not done.
2. **The bridge is not modified.** It is in another crate, and other agents are
   editing that workspace. §7 is a design with a code sketch, not a measurement.
   **The panic story for the async arm is unsolved and is the largest open
   risk in this proposal.**
3. **No `Pipeline::run_async`.** §2.4 says why: it costs a second `Box::pin`
   and the split costs nothing.
4. **The GAT unification was not attempted to completion.** A trait
   `Outcome { type Out<'a>; }` could in principle let one `MiddlewareStack`
   serve both the sync and async cases, with `Out<'a> = Response` and
   `Out<'a> = BoxFuture<'a, Response>`. It would require the blanket closure
   impl to be `for<'a> Fn(&'a mut C, Next<'a, C, F>) -> F::Out<'a>`, which is a
   higher-ranked bound over a GAT, and those do not infer for closures today.
   The sync side would lose `impl Layer for F: Fn(..)` to buy the unification —
   a bad trade. Recorded as rejected on reasoning, not on a compiler error.
5. **No streaming.** §9.
6. **No benchmark.** Allocations are counted exactly; wall-clock throughput of
   an async route against a `spawn_blocking` one is not measured, because the
   bridge was not modified. The 50-in-113ms figure in the brief is the number
   this change is meant to improve, and nothing here has improved it yet.
   *Stale. Step 4 landed and measured it: 107ms on **2** threads against 115ms
   on **50**. The win is the thread count, not the clock.*

---

## 12. Recommendation

Do steps 1 to 3. They are additive, cheap, measured, and they break one line.

Do step 4 with the panic story solved first. That is where the benefit is and
where the risk is.

Treat step 5 as a second proposal. §5 says it is affordable, which is the
question that needed answering. Whether the plugin contract should grow a fifth
hook family is a different question, and this document does not answer it.

Do not do step 6 until something mounts a middleware layer.

---

## 13. What building it proved wrong

Steps 1 to 5 are landed. This section is the list of places where the document
above was mistaken, kept so that the next person reading it knows which
sentences to distrust.

1. **§7's code sketch runs the request hooks on the worker**, calling them
   "sync and cheap". They are cheap as a *response* hook. As a *request* hook
   they are the one thing in the system most likely to block, because §8.1 of
   this same document names `auth.session` — a session-store lookup — as what
   most wants to be one. Shipping the sketch would have blocked a worker inside
   exactly the hook the document exists to enable, and would have broken §7's
   own promise that a sync route keeps every word of the module's
   justification. Only the async arm's hooks moved.

2. **§7 assumes the bridge can hold a `Router<AnyHandler<E>>`.** It cannot
   without breaking every existing caller: `Router` has no `map` and does not
   expose its handlers. A `RouteKind` trait and a default type parameter were
   needed — which buys a better property anyway, since an application with no
   async routes gets the function it had, with the inline arm and the panic net
   deleted by a constant.

3. **§7 implies reading `is_async` is free.** It is readable from
   `Dispatch::Found`, but that arm is reached *after* the request hooks, and the
   thread choice has to be made before them. The cost is an extra router lookup
   on any table containing an async route, plus two mispredictions when a hook
   reroutes across the kinds. Both are handled and tested; neither was in the
   document.

4. **§7's panic paragraph puts `catch_unwind` on the handler.** Not enough:
   `spawn_blocking` also catches a panicking *hook*, so the net must cover
   `begin` and `finish` or the two arms disagree about a panicking response
   hook.

5. **§5's "two families, two traits and two `Vec`s"** — the third was a choice,
   not a consequence, and dropping it dissolves the ordering seam the document
   flagged as its main open risk.

6. **§11 claims the async hook family was partly built in the prototype.** None
   of it was in the committed tree; only steps 1 to 3 were ported. Anyone
   planning against that sentence would underestimate the work.

7. **Cancellation is absent from this document entirely**, and it is a larger
   change for application authors than the panic story it does discuss. A
   handler that is a future is droppable, so a client disconnect can abandon one
   mid-request — which `spawn_blocking` could not do, because a thread cannot be
   cancelled. The precise statement about panics falls out of the same fact: a
   panicking async handler is a handler cancelled at its last `.await`, plus an
   unwind. So what it can leave half-done is what *cancellation* can leave
   half-done, which is work whose cleanup is itself a future — a transaction
   whose rollback is async, for instance. This belongs in the handler-authoring
   docs.

8. **One thing the document got right**, and it was listed as the open risk:
   `catch_unwind` across an `.await` is sound here. Not because futures are
   unwind-safe, but because the bridge stops looking at anything the handler
   touched — the request and its state are local to the caught region, are shown
   to no hook, and are dropped.
