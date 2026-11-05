# Writing a module

This guide is for a developer, or a model, who writes a brocanth module
from zero. It covers the contract, a full walkthrough, every hook, and the
tests a module must pass.

Read first:

- `docs/module-map.md`: the 38 modules, the five categories, and the
  dependency graph.
- `docs/modules.md`: why the seams are where they are.

Every signature below is quoted from the code. The source of truth is
`crates/brocanth-scaffold/src/lib.rs` for build time and
`crates/brocanth/src/plugin_*.rs` for run time.

## 1. What a module is

A module has two halves. Keep them apart in your head.

| Half | Runs | Lives in | Does |
|---|---|---|---|
| **Scaffold module** | once, inside `brocanth create` and `brocanth add` | your crate, behind the `scaffold` feature | writes files and lines into the generated project |
| **Plugin** | in the generated binary, at start and on each request | your crate, always compiled | does the work: hooks, layers, routes, context values |

A module that writes its code out in full into the project has no plugin
crate. `logging` is one (`crates/brocanth-cli/src/modules/logging.rs`).
Most modules have both halves.

### The trait

`crates/brocanth-scaffold/src/lib.rs:68`:

```rust
pub trait Module: Sync {
    /// What this module is called, what it costs, and what it depends on.
    fn manifest(&self) -> Manifest;

    /// Write. Called only when this module was chosen, in dependency order.
    fn contribute(&self, project: &Project, scaffold: &mut Scaffold);
}
```

A module is a unit struct. It holds no state. `manifest` describes it.
`contribute` writes. The CLI calls `contribute` only for a chosen module,
and only after every module named in its `after` list.

### The manifest

`Manifest` (`lib.rs:81`) has 18 fields. Build it with `Manifest::new(key,
gist)` and the builder methods. Do not set the fields directly: some
builders set two fields at once on purpose.

| Field | Builder | Meaning |
|---|---|---|
| `key` | `new(key, _)` | The word on the command line and in the chooser. Lower case. A hyphen only when the module has a parent, and then the key starts with `<parent>-`. |
| `gist` | `new(_, gist)` | One line for the flat list. Say what the binary pays. No full stop: it is a column entry. |
| `parent` | `part_of(p)`, `part_of_but_off(p)` | The choice this one sits inside. The chooser nests and hides it. The flags refuse it without the parent. |
| `on_with_parent` | `part_of` sets `true`, `part_of_but_off` sets `false` | Whether the chooser ticks it when the parent is ticked. The flags never apply it: `--with` never implies a module. |
| `after` | `after(k)`, also `needs(k, _)` | Write after `k` when both are chosen. Only order. If `k` is not chosen, the line has no effect. |
| `needs` | `needs(k, why)` | A requirement. The flags refuse the pick without `k` and print `why`. The chooser ticks `k` for you. Also adds `k` to `after`. |
| `crates` | `krate(Dep)` | Dependencies of the **generated** project, not of the CLI. |
| `because` | `because(why)` | Why this module needs its parent. One sentence. Required with a parent (a test checks it). |
| `blocked` | `blocked(why)` | Listed, but nobody can choose it. The reason fits on its row: at most 80 characters, one line. |
| `absent` | `absent(what, why)` | What the generated `brocanth.toml` says when the module was **not** chosen. |
| `section` | `asked_in(section, _)` | Which guided question asks about it: `shape`, `foundations`, `data`, `pages`, `behaviour`, `doors`, `operations`, `extras` (`crates/brocanth-cli/src/wizard.rs`). Empty puts it in the last question. |
| `short` | `asked_in(_, short)` | The shorter phrase the guided question uses. Empty falls back to `gist`. |
| `outside_runtime` | `outside_runtime()` | It adds nothing that only a running server uses: a build step, templates, CI, config, the command line. A static site may take it. |
| `static_output` | `static_output()` | It turns the project into files. Every other chosen module must be `outside_runtime`. |
| `categories` | `category(Category::X)`, once per category | The kinds of seam it hooks into: `Middleware`, `Renderer`, `External`, `Enhancer`, `Routes`. The first is the main one. A test holds the module to the slots its categories open (`Slot::open_to`). |
| `conflicts` | `conflicts(k, why)` | It cannot be chosen beside `k`. Declare it on the side that gives way. `create`, `add` and the chooser all read it through `brocanth_scaffold::clash`. |
| `adapts_to` | `adapts_to(k, what_changes)` | It writes different code when `k` is chosen: the optional dependency. Every `project.has(k)` in `contribute` must be declared here, in `needs`, or be the parent. A test checks both directions. |
| `recommends` | `recommends(k, why)` | A deployed project with this module usually wants `k`. Advice only: nothing refuses. `create` and `add` warn when `k` is absent, `brocanth check` warns (`check/recommended`), and `--questions` lists the edge. `why` names the cost in production. Not for a module already in `needs`. |

`Dep` (`lib.rs:313`) names a crate and leaves the source to the CLI for
brocanth's own crates:

```rust
Dep::brocanth("brocanth-cache")                       // the CLI picks git or --brocanth-path
Dep::git("jt", "https://github.com/DHHwasRight/jt", "main")
Dep::version("serde", "1").feature("derive").no_default_features().note("why it is here")
```

A third-party module uses `Dep::git` or `Dep::version` for its own crate.
`Dep::brocanth` resolves only names inside this workspace.

### What a module reads: `Project`

`Project` (`lib.rs:394`) is read-only:

```rust
pub name: &'a str,              // the cargo package name, also the directory
pub title: &'a str,             // the human name
pub fn has(&self, key: &str) -> bool
pub fn ident(&self) -> String   // `my-blog` -> `my_blog`, the env var prefix
```

`has` is how a module adapts to another module. Ask only for a key that
exists. `has("tailwind")`, when the key is `rsx-tailwind`, returns `false`
**and** the CLI reports the unknown key. A typo never reads as "not
chosen".

### What a module writes: `Scaffold`

`Scaffold` (`lib.rs:643`) collects everything. A module cannot read the
filesystem, edit a file another module wrote, or invent a slot.

```rust
pub fn file(&mut self, path: impl Into<String>, contents: impl Into<String>)
pub fn at(&mut self, slot: Slot) -> Writer<'_>
pub fn binary(&mut self, name: impl Into<String>, path: impl Into<String>)
pub fn refuse(&mut self, why: impl Into<String>)
```

- `file` writes a whole file the module owns. Two modules writing one path
  is an error that names both. Put a plugin in `src/plugins/<key>/mod.rs`,
  never `src/plugins/<key>.rs`: a directory can be deleted whole, and a
  second file goes beside the first.
- `at(slot)` returns a `Writer` with three methods:
  - `line(text)` appends one line and adds the newline.
  - `once(text)` appends one line unless that exact line is already there.
    Use it for `use` lines.
  - `block(text)` appends text as it is. Use it for a TOML section or a
    Markdown section that already ends in a newline.
- `binary` adds a `[[bin]]` target. Two modules asking for one name is an
  error.
- `refuse` stops the run with a message. The CLI writes nothing.

## 2. Walkthrough: a module from zero

The example is `correlation`: an id per request. A request hook reads
`x-request-id` or makes one. A response hook sends it back. A page reads
it with `ctx.hooked::<CorrelationId>()`.

The shape copies `middleware-security-headers`
(`crates/brocanth-middleware/src/scaffold.rs:238`) for the scaffold half,
and `ProxyHeaders` (`crates/brocanth-middleware/src/proxy_headers.rs:74`)
for the plugin half.

### Step 1: the crate and its feature

```toml
# brocanth-correlation/Cargo.toml
[package]
name = "brocanth-correlation"
version = "0.1.0"
edition = "2021"

[features]
default = ["runtime"]
# What an application runs: every dependency but the scaffold's. On by
# default. The CLI turns it off, so `brocanth create` never links it.
runtime = ["dep:brocanth", "dep:http"]
# The code that writes this crate into a project. Off, so an application
# that depends on this crate never links the generator.
scaffold = ["dep:brocanth-scaffold"]

[dependencies]
brocanth = { git = "https://github.com/DHHwasRight/brocanth", branch = "main", optional = true }
http = { version = "1", optional = true }

[dependencies.brocanth-scaffold]
git = "https://github.com/DHHwasRight/brocanth"
branch = "main"
optional = true
```

The `scaffold` feature is off by default. This is the rule in `PLAN.md`
§8: nothing ships unless it is asked for. Compare
`crates/brocanth-middleware/Cargo.toml:66`.

The two features also split the crate the other way. `scaffold` depends
on `brocanth-scaffold` and nothing else. Everything an application runs
sits behind `runtime`:

- In `src/lib.rs`, every item but `pub mod scaffold` carries
  `#[cfg(feature = "runtime")]`. The README include becomes
  `#![cfg_attr(feature = "runtime", doc = include_str!("../README.md"))]`.
- Every other feature that needs the runtime lists `"runtime"` first.
- The scaffold module never names the crate's own types. It writes text.

The reason is the CLI. It takes every module crate with
`default-features = false, features = ["scaffold"]`. A runtime dependency
outside `runtime` (a driver, a TLS stack, tokio) then lands in a tool that
only writes files. The CLI still builds, slower and larger, and nobody
notices. `crates/brocanth-cli/tests/lean.rs` notices: it fails when the
CLI's dependency graph holds a runtime crate. A crate whose runtime needs
only small pure crates (`brocanth-robots`, `brocanth-policy`) may skip the
feature; that test decides. Compare `crates/brocanth-db/Cargo.toml`.

### Step 2: the plugin

```rust
// brocanth-correlation/src/lib.rs
use std::sync::atomic::{AtomicU64, Ordering};

use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{
    BodyShape, Both, Flow, HookRegistrar, HookSource, OnRequest, OnResponse, Request,
    RequestHook, RequestState, ResponseHead, ResponseHook,
};
use brocanth::plugin_order::Order;
use http::header::{HeaderName, HeaderValue};

#[cfg(feature = "scaffold")]
pub mod scaffold;

/// The id of this request. A page reads it with
/// `ctx.hooked::<CorrelationId>()`.
pub struct CorrelationId(pub String);

#[derive(Debug, Clone)]
pub struct CorrelationConfig {
    /// The header to read and to send back.
    pub header: String,
}

impl Default for CorrelationConfig {
    fn default() -> Self {
        Self {
            header: "x-request-id".to_string(),
        }
    }
}

pub struct Correlation {
    config: CorrelationConfig,
}

impl Plugin for Correlation {
    type Config = CorrelationConfig;
    type Provides = NoContext;
    const NAME: &'static str = "correlation";
    // Registering it with `plugin` instead of `plugin_hooking` is then a
    // build error, not hooks that never run.
    const INSTALLS_HOOKS: bool = true;

    fn new(config: Self::Config) -> Self {
        Self { config }
    }
}

impl HookSource for Correlation {
    type Families = Both<OnRequest, OnResponse>;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        let header = HeaderName::try_from(self.config.header.as_str())
            .map_err(|e| PluginError::new(format!("`header`: {e}")))?;
        registrar
            .request("read", Order::new(), Read { header: header.clone() })
            .response("echo", Order::new(), Echo { header });
        Ok(())
    }
}

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Read {
    header: HeaderName,
}

impl RequestHook for Read {
    fn before_dispatch(
        &self,
        request: &mut Request,
        state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        let id = request
            .headers()
            .get(&self.header)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
            .unwrap_or_else(|| NEXT.fetch_add(1, Ordering::Relaxed).to_string());
        state.insert(CorrelationId(id));
        Ok(Flow::Continue)
    }
}

struct Echo {
    header: HeaderName,
}

impl ResponseHook for Echo {
    fn on_headers(
        &self,
        _request: &Request,
        head: &mut ResponseHead,
        _body: BodyShape<'_>,
        state: &mut RequestState,
    ) -> Result<(), PluginError> {
        if let Some(CorrelationId(id)) = state.get::<CorrelationId>() {
            if let Ok(value) = HeaderValue::from_str(id) {
                head.headers.insert(self.header.clone(), value);
            }
        }
        Ok(())
    }
}
```

### Step 3: the scaffold module

```rust
// brocanth-correlation/src/scaffold.rs
//! `correlation` -- what `brocanth create` writes when somebody asks for a
//! request id. Compiled only under the `scaffold` feature.

use brocanth_scaffold::{snippet, Dep, Manifest, Module, Project, Scaffold, Slot};

pub struct Correlation;

impl Module for Correlation {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "correlation",
            "an id per request, read from or added to `x-request-id`",
        )
        .asked_in("doors", "Give every request an id")
        .krate(Dep::git(
            "brocanth-correlation",
            "https://github.com/you/brocanth-correlation",
            "main",
        ))
        .absent(
            "correlation",
            "no request id; a log line cannot be matched to the response it describes",
        )
    }

    fn contribute(&self, _project: &Project, s: &mut Scaffold) {
        // Three slots from one call: the registration, the provider list
        // and the hook record. They must agree, so one function writes all
        // three.
        snippet::hooking(
            s,
            "correlation",
            "brocanth_correlation::Correlation",
            REGISTRATION.to_string(),
        );
        s.at(Slot::AgentsSections).block(AGENTS.to_string());
    }
}

/// The registration. It lands inside `plugins::register!`, so it is
/// indented for that block and already in rustfmt's shape.
const REGISTRATION: &str = r#"
        // -- brocanth-correlation -----------------------------------------
        //
        // Reads `x-request-id`, or makes one, and sends it back. A page
        // reads it with `ctx.hooked::<brocanth_correlation::CorrelationId>()`.
        //
        let builder = builder.plugin_hooking(
            <brocanth_correlation::Correlation as brocanth::plugin_api::Plugin>::new(
                brocanth_correlation::CorrelationConfig::default(),
            ),
        );
"#;

const AGENTS: &str = "\n\
    ## Request ids\n\
    \n\
    `brocanth_correlation::Correlation` is registered in\n\
    `src/plugins/mod.rs`. Read the id with\n\
    `ctx.hooked::<brocanth_correlation::CorrelationId>()`. Do not make a\n\
    second id in a page.\n";
```

The registration is a literal string because the generated file must pass
its first `cargo fmt` unchanged (§4). Write the block in the shape rustfmt
gives it, with 8 spaces of indent.

### Step 4: register it in the CLI

Two lines, then rebuild the CLI. This is the whole registry, on purpose:
the menu is the receipt for what a binary contains, so nothing joins it
through a transitive dependency (`crates/brocanth-scaffold/src/lib.rs:38`).

```toml
# crates/brocanth-cli/Cargo.toml
brocanth-correlation = { git = "https://github.com/you/brocanth-correlation", default-features = false, features = ["scaffold"] }
```

```rust
// crates/brocanth-cli/src/modules/mod.rs, inside ALL (line 67)
&brocanth_correlation::scaffold::Correlation,
```

The position in `ALL` sets only the order of the list on screen. The order
of contributions comes from `after` and `needs`.

```
cargo install --path crates/brocanth-cli
brocanth create my-app --with correlation
```

### What the project gets

`snippet::hooking` writes three things into the owned file
`src/plugins/mod.rs`:

```rust
// in plugins::register! (Slot::AppPlugins): the REGISTRATION block

// the provider list (Slot::PluginsInstalled): `correlation brocanth_correlation::Correlation`
type WithCorrelation = After<brocanth_correlation::Correlation, Base>;
pub type Installed = WithCorrelation;

// the hook record (Slot::PluginsHooks): `brocanth_correlation::Correlation`
type HookedCorrelation =
    Both<<brocanth_correlation::Correlation as HookSource>::Families, NoHooks>;
pub type Hooks = HookedCorrelation;
```

The alias names come from the key, not the type. Two types with one name
from two crates get two aliases.

## 3. The hook reference

There are two kinds of hook:

- **Build time**: a `Slot`, a named place in a file the generator owns. A
  module appends lines to it.
- **Run time**: a trait in the `brocanth` crate that the plugin implements.

The slots are a closed list (`lib.rs:459`). A module that needs a new seam
needs a change to that enum, with its first reader. A slot with no reader
discards what a module writes, so unread slots were deleted.

### 3.1 Middleware: runs around a request

#### `Slot::PluginsHooks`, with `snippet::hooking`

Writes one hooking plugin type per line. The generator builds
`pub type Hooks` from these lines in `src/plugins/mod.rs`. Always write it
through `snippet::hooking` (`lib.rs:1030`):

```rust
pub fn hooking(s: &mut Scaffold, key: &str, ty: &str, block: String)
```

`block` is the full statement, and it must call `plugin_hooking` on
`builder`. See the walkthrough for an example.

A hooking plugin with settings uses `snippet::configurable_hooking`
instead. It takes a `Hooking { key, section, ty, config_ty, prelude,
adjust }` and the settings, and writes the `[section]` block, a
registration through `plugin_hooking_from_config` (or the flags without
`config`), the provider and hook record lines, and the settings type.
`section` is the plugin's instance name; `config_ty` is its configuration
type, which needs `Default` and public fields. `adjust` is a method chain
for a value only code can give. `crates/brocanth-middleware/src/scaffold.rs`
has three examples.

The run-time half is `HookSource` (`crates/brocanth/src/plugin_hooks.rs:798`):

```rust
pub trait HookSource: Plugin {
    type Families: HookFamilies;
    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError>;
}
```

- `Families` is a type: `OnRequest`, `OnAsyncRequest`, `OnResponse`,
  `OnError`, `OnModel`, or `Both<A, B>` for two of them. A family that no
  plugin installs costs nothing: the hot path checks a constant and the
  compiler removes the loop.
- `HookRegistrar` has `request`, `request_async`, `response`, `error` and
  `model`. Each takes a name, an `Order`, and the hook. The core qualifies
  the name with the instance name.
- Set `const INSTALLS_HOOKS: bool = true;` in `impl Plugin`.
  `AppBuilder::build` then refuses a hooking plugin that was registered
  with `plugin`.

The four hook traits:

| Trait | Method | Runs | Can |
|---|---|---|---|
| `RequestHook` | `before_dispatch(&self, &mut Request, &mut RequestState) -> Result<Flow, PluginError>` | before the handler, in declared order | rewrite the request, write state, answer with `Flow::respond(r)` |
| | `after_dispatch(&self, &Request, &Response, &mut RequestState)` | after the response is final, reverse order | observe; cannot fail |
| `AsyncRequestHook` | `before_dispatch` returns a future | in the same chain as `RequestHook` | wait on a store or a service |
| `ResponseHook` | `on_headers(&self, &Request, &mut ResponseHead, BodyShape<'_>, &mut RequestState) -> Result<(), PluginError>` | every response, a stream too | set headers |
| | `on_body(&self, &Request, &mut ResponseHead, &mut Vec<u8>, &mut RequestState)` | buffered responses only | rewrite the body |
| `ErrorHook` | `on_error(&self, &HandlerError, &Request, &mut RequestState) -> Option<Response>` | declared order; the first `Some` wins | turn an error into a response |

`Request` is `http::Request<Vec<u8>>`. `ResponseHead` is
`http::response::Parts`.

`Order` (`crates/brocanth/src/plugin_order.rs:106`) places a hook against
other hooks by their qualified name:

```rust
Order::new().after("proxy-headers.read").before_if_present("csrf.check")
```

`after_if_present` and `before_if_present` do not fail when the other hook
is missing. `after` and `before` do.

**`RequestState`** carries values between hooks and into the page. It is
a type map: `insert<T>(value) -> Option<T>` and `get<T>() -> Option<&T>`,
for `T: Send + Sync + 'static`. Use a type you own as the key, not a
string.

#### The pipeline in a generated project

`src/app.rs` is the developer's file. It runs every request like this:

```rust
pub async fn handle(app: &App, request: brocanth_server::Request) -> brocanth_server::Response {
    let Ok(mut request) = brocanth_server::bridge::to_core(request) else {
        return brocanth_server::Response::text(400, "Bad Request");
    };
    let mut state = RequestState::new();
    let outcome = match app.core.begin_async(&mut request, &mut state).await {
        Begin::Dispatch => Ok(sent(dispatch(app, &request, &mut state).await)),
        Begin::Answered(outcome) => outcome,
    };
    let response = app.core.finish(&mut request, &mut state, outcome);
    brocanth_server::bridge::from_core(response)
}
```

1. `to_core` turns the server request into a `Request`. It also stores the
   peer address as a `SocketAddr` extension.
2. `begin_async` runs the request hooks. A hook that answers skips the
   router and the page.
3. `dispatch` matches the route, builds `Ctx`, runs the layers, then the
   page. It is `async`: the layers and the page are awaited, so a page or a
   layer that waits holds no thread.
4. `sent` buffers the page's answer, so the body hooks can read it.
5. `finish` runs the response hooks, then the error hooks for an error,
   then `after_dispatch`.
6. `from_core` gives the response back to the server.

The response hooks see every answer, 404 and 405 included.

#### `Ctx`: how a page reads a hook

`dispatch` moves the state into the context, and takes it back after the
page:

```rust
let mut ctx = Ctx::new(verb, path, matched.params())
    .with_plugins(app.core.context())
    .with_state(std::mem::take(state))
    // ...
let response = app.stack.run(path, &mut ctx, |c| { /* the page */ }).await;
*state = ctx.take_state();
```

- `with_state(RequestState) -> Self` hands the state to the page.
- `take_state(&mut self) -> RequestState` takes it back for the response
  hooks. A value the page writes is visible to them.
- `hooked::<T>() -> Option<&T>` reads a value a request hook wrote.
  `None` when no hook wrote one.

```rust
let id = ctx.hooked::<brocanth_correlation::CorrelationId>();
```

#### `Slot::PluginsLayers` and `Slot::AppSetup`: a layer

A layer is a function around the page. It runs only for a matched route,
inside the `Ctx`. It returns a future, because a layer may wait:

```rust
pub type Layer = for<'a> fn(&'a mut Ctx, AsyncNext<'a, Ctx, Response>) -> BoxFuture<'a, Response>;
```

The lifetime is named because the future borrows the context and the rest
of the chain. A layer lends the context on with `next.run(&mut *ctx)` and
has it back after the `.await`, so it can change the response after the
page ran. This one marks every page under the mount as private to the user:

```rust
pub fn layer<'a>(ctx: &'a mut Ctx, next: AsyncNext<'a, Ctx, Response>) -> BoxFuture<'a, Response> {
    Box::pin(async move {
        let mut response = next.run(&mut *ctx).await;
        response
            .headers
            .push(("cache-control".to_string(), "private".to_string()));
        response
    })
}
```

Not calling `next.run` short-circuits the chain. A closure is not a layer:
it cannot name the lifetime, so write a `fn`.

Three parts, because the pieces have three owners:

1. The plugin **offers** the layer in `configure_router`. A
   `MiddlewareCatalog` has no `mount`, so a plugin cannot mount itself.
   ```rust
   fn configure_router(&mut self, catalog: &mut MiddlewareCatalog<'_>) -> Result<(), PluginError> {
       catalog.offer("session", "reads the cookie and puts a Session on the context");
       Ok(())
   }
   ```
2. `Slot::AppSetup` mounts it at a pattern, in `src/app.rs` inside
   `setup`. The pattern is the application's decision, so the line goes in
   the developer's file.
   ```rust
   s.at(Slot::AppSetup)
       .line(r#"            setup.router().mount("/users/{*rest}", "session");"#);
   ```
3. `Slot::PluginsLayers` maps the name to the function, in
   `plugins::layer`:
   ```rust
   s.at(Slot::PluginsLayers)
       .line(r#"        "session" => Some(sessions::layer),"#);
   ```

A mounted name with no layer stops the application at start.
`crates/brocanth-cli/src/modules/sessions.rs` uses all three.

**Hook or layer?** A hook sees every request, before routing, and can
answer a 404. A layer sees only matched routes and has `Ctx`. A security
policy is a hook. A request log is a hook too: a layer never sees a 404, a
405, a CORS preflight or a 401 that a request hook answered.
`crates/brocanth-cli/src/modules/logging.rs` writes its line from
`after_dispatch`, which runs for every response. A page passes a value to
that hook with `ctx.hook_insert(value)` or changes a hook's value with
`ctx.hooked_mut::<T>()`.

#### `Slot::PluginsInstalled` and `Slot::AppContext`: a context value

A plugin that puts a value on the per-request context declares it in its
type:

```rust
type Provides = ProvidesContext<Session>;   // Session: ContextItem

fn provide(&self, cx: &mut ContextBuilder) {
    cx.insert(Session { /* ... */ });
}
```

`Slot::PluginsInstalled` takes one line per plugin: `"<key> <type path>"`.
The generator builds `pub type Installed` from it. `snippet::plugin` and
`snippet::hooking` write it. Never write it alone.

A page reads the value. It **compiles only if a registered plugin provides
`T`**:

```rust
let session = ctx.provided::<Session, _>().expect("dispatched");
```

`Slot::AppContext` writes lines into `plugins::announce!`, which runs once
at start. The generator writes `let cx = $app.context();` above them. Use
it to fail at start, not on the first request:

```rust
s.at(Slot::AppContext).block(
    "    let session: &crate::plugins::sessions::Session = cx.extract();\n".to_string(),
);
```

Spell paths from the crate root (`crate::plugins::..`): the macro expands
in `src/app.rs`.

**A value that depends on the request.** Override `provide_for_request`
as well as `provide`:

```rust
fn provide(&self, cx: &mut ContextBuilder) {
    cx.insert(Locale::default());      // a context with no request: `announce!`, a test
}

fn provide_for_request(&self, request: &Request, state: &RequestState, cx: &mut ContextBuilder) {
    cx.insert(Locale::from_header(request.headers().get("accept-language")));
}
```

`src/app.rs` calls `app.core.context_for(request, state)` after the request
hooks ran, so `state` holds what they wrote. A page reads the value with
`ctx.provided`, with the compile-time check. Keep `provide` too: it is the
value a context with no request gets, and `Provides` promises a value in
every context.

`provide_for_request` cannot wait and cannot fail. A lookup that waits (a
session in a store) belongs in an async request hook, which writes the
result into `state`; `provide_for_request` moves it onto the typed context.
A failure belongs to the hook, which can answer the request. A project made
before this method existed calls `app.core.context()` in `src/app.rs`, and
its pages get `provide`'s value.

To say something at start, use `crate::plugins::say!`, not `eprintln!`.
With `observability-logs` it is a `tracing` event (JSON in production);
without it, a line on stderr. Bind each value first, then name it in the
message and, after `;`, as a field in `tracing`'s shorthand:

```rust
let connections = db.pool().size();
crate::plugins::say!(info, "db: {connections} connection(s) open"; connections);
```

#### `Slot::ServerWrap`: around the whole server

A server wrap sits around `handle`, outside the request hooks. It sees
every request, a static file included, and the response after the hooks.
`observability` uses one to answer `/up`, `/readyz` and `/metrics` and to
time every request. `rsx` uses one to serve static files before the router.

Write a type that implements the generated `plugins::ServerWrap` trait,
and name the type in the slot. The path resolves in `src/plugins/mod.rs`:

```rust
s.file("src/plugins/timing/mod.rs", TIMING_RS);
s.at(Slot::PluginsMod).line("pub mod timing;");
s.at(Slot::ServerWrap).line("timing::Timing");
```

The trait, as the generator writes it when one wrap exists:

```rust
pub type Handler = Arc<dyn Fn(brocanth_server::Request) -> Answer + Send + Sync>;

pub trait ServerWrap {
    fn wrap(app: &Arc<crate::app::App>, next: Handler) -> Handler;
    fn side_listeners(app: &crate::app::App) -> impl Future<Output = Result<(), String>>; // default: none
    fn before_stop(app: &crate::app::App) -> impl Future<Output = ()>;                   // default: nothing
}
```

- `wrap` returns `next` with the wrap around it. Not calling `next`
  answers the request: that is how a probe or a static file answers.
- `side_listeners` opens listeners beside the main one, once, after the
  plugins started. `observability-metrics` serves `/metrics` there.
- `before_stop` runs when a stop begins, before the listener closes.
  `observability` fails `/readyz` here and waits `drain_delay_secs`.

**The composition rule.** The first line in the slot is the outermost
wrap: it sees a request first and the response last. Lines land in the
order the modules contribute. A wrap that must sit inside another declares
`after(other)`: `rsx` declares `after("observability")`, so the observer
logs and counts static files. `side_listeners` and `before_stop` run in
the same order, one after the other, so two drain delays add up.

Wrap or hook? A hook has `RequestState`, the route, and an order against
other hooks; a page can pass it a value. A wrap has none of these. Use a
wrap only for what must happen outside the hooks: an answer before any
hook runs, a listener of its own, a step in the stop.

Without a wrap, `src/plugins/mod.rs` has no trait, and `wrap` passes the
handler through as it is.

### 3.2 Renderer: makes a response body

#### `Slot::PagesRenderers`

Writes items into `src/pages/mod.rs`, **before** `Slot::PagesItems`. One
`pub mod` per template engine that wraps the engine's `load_dir!`, and the
function that turns a template into a `Response`. Two renderers do not
conflict: each `load_dir!` reads the same tree and compiles only its own
file extension. The route table ignores both, because neither is a `.rs`
file.

```rust
s.at(Slot::PagesRenderers).block(
    "pub mod payload {\n    jt::load_dir!(\"src/pages\");\n}\n\n\
     pub fn render_json(content: impl jt::ToJson) -> Result<Response, Error> {\n    \
     Ok(Response::ok(jt::to_string(&content)).with_header(\"content-type\", \"application/json\"))\n}\n"
        .to_string(),
);
```

`crates/brocanth-cli/src/modules/jt.rs:58` is the full example. A renderer
is nearly always `outside_runtime()`.

#### `Slot::PagesItems`

Writes items the pages share into `src/pages/mod.rs`, after the
renderers. Use it for a helper function, not for an engine.

### 3.3 Routes: answers at a path

#### `Slot::RouterMounts`

One line per route source, inside the generated `pages::mounted`. The line
calls a function with this shape:

```rust
pub fn mount(
    routes: &mut RouterBuilder<Handler>,
    prefix: &str,
) -> Result<(), BuildError>;
```

```rust
s.file("src/plugins/admin/mod.rs", ADMIN_RS);   // defines `mount`
s.at(Slot::RouterMounts)
    .line("    crate::plugins::admin::mount(routes, \"/admin\")?;");
```

The module writes the default prefix. The developer may change it: the
prefix is the application's decision. There is one route table, so a path
claimed twice is a build error, not a silent override. Customers:
`brocanth-admin/src/scaffold.rs:103`, `brocanth-resource/src/scaffold.rs:83`.

#### `Slot::PagesRoutes`

One route pattern per route a module adds, as the table spells it:
`/status`, `/users/{id}`. `src/pages/mod.rs` has a test that asserts the
whole table. Without this line, the generated test fails as soon as the
module is chosen.

A page is a file. To add one, write `src/pages/<name>.rs` with `s.file`
and add its pattern here. A mount also adds patterns: list each one.

### 3.4 Enhancer: adds a capability

#### `Slot::AppPlugins`, `Slot::PluginsMod`, `Slot::AppUses`

- `AppPlugins`: one statement per plugin inside `plugins::register!`.
- `PluginsMod`: `pub mod <key>;` for a plugin written into
  `src/plugins/<key>/`.
- `AppUses`: `use` lines the chain needs. Always `once`.

Write a plain plugin with `snippet::plugin` (`lib.rs:1013`). It writes
`AppPlugins` and `PluginsInstalled` together:

```rust
pub fn plugin(s: &mut Scaffold, project: &Project, key: &str, ty: &str)
```

It writes one of two forms, chosen by `project.has("config")`:

```rust
// with config: built from the `[key]` section
let builder = builder
    .plugin_from_config::<crate::plugins::cache::Cache>("cache")
    .map_err(|e| e.to_string())?;

// without: built from a literal the compiler checks
let builder = builder.plugin(crate::plugins::cache::Cache::new(
    crate::plugins::cache::Settings::default(),
));
```

The second form calls a trait method. So without config, also write
`s.at(Slot::AppUses).once(snippet::PLUGIN_TRAIT_USE)`.

The `Plugin` trait (`crates/brocanth/src/plugin_api.rs:345`) has these
stages. Each one runs for every instance before the next stage starts:

| Stage | Use it to |
|---|---|
| `configure` | settle internal state |
| `register` | publish services for other plugins |
| `after_config` | look up other plugins' services |
| *the application's `setup` block* | mount layers |
| `configure_router` | offer middleware |
| `before_start` | fail cheaply, the last time |
| `start` | spawn workers; the socket is open |
| `begin_shutdown` | stop accepting work (reverse order) |
| `shutdown` | drain, within the time left (reverse order) |

**Never block on async work in a stage.** Every stage is synchronous. A
stage that waits for a future, through `Handle::block_on` on a scoped
thread or anything else, hangs on a current-thread runtime: the thread
that runs the stage is the thread that drives the I/O, so the future
never advances. `#[tokio::test]` is a current-thread runtime, so the first
test that builds the application hangs. A production server uses a
multi-thread runtime, which is why the bug stays hidden.

So open lazily and check in the startup gate:

- In `before_start`, build the pool with no connection
  (`Db::connect_lazy`, `PgCache::connect_lazy`, `Queue::connect_lazy`,
  `PgStore::connect_lazy`) inside `handle.enter()`, because the pool
  spawns its upkeep task.
- Register the check with `lc.ready_when`. Put one-time async setup, such
  as creating a table, in the check, behind a flag, so it runs once.
- The generated `serve` awaits `Readiness::check_all` once, before it
  binds. A failed check stops the start with `[instance] reason`, so give
  the reason a `help:` line. `/readyz` runs the same checks later.
- Spawn workers in `start`, which runs after the gate.
- In `shutdown`, wait for an async close only on a multi-thread runtime.
  On a current-thread one, spawn it and return.

#### `Slot::CommandsMod`

`pub mod <name>;` in `src/commands/mod.rs`, for a subcommand the module
writes. The generator owns the lines above the `// Your commands` line;
the developer's own `pub mod` lines go below it, and `add` keeps them. A project without `commands` has no command line, so declare
`needs("commands", "<what breaks>")`.

#### Settings: `snippet::configurable`

Declare each setting once. `snippet::configurable` writes everything that
follows from the declaration:

- with `config`: the `[key]` section in `config/<env>.toml`, built from
  the settings, and the configured registration;
- without `config`: a registration that starts from `Settings::default()`
  and applies the environment and the flags of this run;
- either way: a `--key.setting` flag and a `$<APP>_<KEY>__<SETTING>` variable.

It replaces `snippet::plugin` for a plugin with settings. Real use, from
`crates/brocanth-cli/src/modules/sessions.rs`:

```rust
configurable(
    s,
    project,
    "sessions",
    "Sessions",
    vec![
        Setting::text("cookie", &format!("_{}_session", project.ident()), "The session cookie's name."),
        Setting::bool("secure", false, "Send the cookie over HTTPS only."),
    ],
);
```

Kinds: `text`, `bool`, `integer`, `float`, `list` (file and environment
only, no flag). Modifiers:

- `.secret()`: no flag, because every user on the host can read a
  process's arguments. Use it for any URL that can carry a password.
- `.none_as("-1")`: one more value that means `None` for this setting.
  An empty value means `None` for every setting.
- `.commented()`: the key is written commented out in the file.

Two modules that declare the same key fail `create` and name both. The
flag layer, the renames in `src/flags.rs` and the precedence are in
`docs/module-flags.md`.

A settings struct that `plugin_from_config` reads must derive
`Deserialize` only with config. `snippet::settings_derive(project,
section)` returns the right derive lines and a doc sentence. Without
config the project then has no parser in its tree.

`Slot::ConfigSections` is still there for a section with no plugin
behind it. Write it only when `project.has("config")`.

#### `Scaffold::binary`

A second `[[bin]]`. Write the source with `s.file`, then
`s.binary("admin", "src/bin/admin.rs")`.

### 3.5 External: does not touch the code

| Slot | File | Unit |
|---|---|---|
| `MiseTasks` | `mise.toml` | one `[tasks.x]` table, with `block` |
| `GitIgnore` | `.gitignore` | one path per `line` |
| `NodeDependencies` | `package.json` | **not read**. `rsx-tailwind` writes the whole `package.json` with `s.file` |

Anything else is a whole file: `Dockerfile`, `.github/workflows/ci.yml`.
Write it with `s.file`. An external module is nearly always
`outside_runtime()`.

```rust
s.at(Slot::MiseTasks).block(
    "\n[tasks.css]\nrun = \"npx @tailwindcss/cli -i src/app.css -o public/app.css\"\n".to_string(),
);
s.at(Slot::GitIgnore).line("/node_modules");
```

### 3.6 Every category: `Slot::AgentsSections`

One `##` section in `AGENTS.md`, for a rule a coding agent must follow in
this project. The sections land after "Adding a route", before the
sections every project gets, in the order the modules contribute. Start
the block with `"\n## "`. Write a rule with its
reason, not a feature list. See `SECURITY_HEADERS_AGENTS` in
`crates/brocanth-middleware/src/scaffold.rs`.

### 3.7 Every category: `Slot::MainMods`

One `mod` line in `src/main.rs`, for a file a module puts at the crate
root: `rsx` writes `mod assets;` for `src/assets.rs`. The generator sorts
the lines among its own, as rustfmt does. `src/main.rs` is the developer's
file, so `brocanth add` writes there only while it is unedited. Prefer a
file under `src/plugins/<key>/`, which needs no line here.

### 3.8 Run-time seams that no slot reaches

- `RouteSource` with `App::mount_routes`: works in the core. Generated
  projects do not use it, by decision: they mount through
  `Slot::RouterMounts`.
- `Services`: a typed lookup between plugins. Publish in `register`, read
  in `after_config` or later.
- `ModelEvent` / `ModelHook`: hooks on ORM events, through
  `HookRegistrar::model`.

## 4. Rules and tests

### The tests in `crates/brocanth-cli/src/modules/mod.rs`

A module in `ALL` must pass these:

| Test | Rule |
|---|---|
| `every_key_is_spelled_once` | No two modules share a key. |
| `a_parent_and_an_after_name_a_module_that_exists` | `parent` and each `after` name a registered key. |
| `a_child_is_named_after_the_choice_it_is_part_of` | A child's key starts with `<parent>-` and it has a `because`. A key with no parent has no hyphen. |
| `a_requirement_names_a_module_that_exists_and_says_what_breaks` | Each `needs` names a real key, not itself, with a reason. |
| `a_module_nobody_can_choose_says_why_on_its_own_row` | `blocked` is at most 80 characters, one line, not `on_with_parent`, and has no `absent`. |
| `every_module_says_what_it_costs` | `gist` is not empty and has no full stop. |
| `a_module_asks_only_about_what_its_manifest_declares` | Each `project.has(k)` is declared (`adapts_to`, `needs`, parent), and each `adapts_to` is asked. The module runs twice: beside every module, and with only its needs. |
| `a_module_writes_only_into_the_slots_its_categories_open` | The module has a category, and writes only into slots its categories open. |
| `a_conflict_names_a_module_that_exists_and_says_why` | Each `conflicts` and `adapts_to` names a real key, with a reason. |

### The generator tests in `crates/brocanth-cli/src/commands/create.rs`

- **Goldens.** `the_generator_writes_what_it_wrote_yesterday` compares
  each shape in `shapes()` (`create.rs:3854`) with
  `crates/brocanth-cli/tests/golden/<shape>.txt`. The `everything` shape
  takes every available module, so a new module lands there with no
  change to the test. After a change, bless the goldens and read the diff:
  ```
  BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- generator_writes
  ```
  Add a shape of your own when the module writes something different with
  and without another module.
- **Formatting.** `every_generated_rust_file_is_already_formatted`
  (`create.rs:3949`) runs rustfmt over every generated `.rs` file in every
  shape. A file must come out unchanged. So write blocks in rustfmt's
  shape: 100 columns, a chain broken after `=` when it is long, and
  `mod` lines in alphabetical order.
- **Registration.**
  `a_registration_without_a_provider_list_entry_is_refused_by_name`: a
  module that writes `AppPlugins` without `PluginsInstalled` is refused.
  Use the snippets.
- **`brocanth add`.**
  `adding_a_module_writes_what_creating_with_it_would_have_written`:
  `add` writes the same thing as `create`. A module that changes a file
  the developer owns goes through while that file is unedited, and is
  refused once the developer has edited it. Keep writes in your own files
  and in slots, so the refusal stays rare.

Then generate a project and build it. A golden proves only the text:

```
cd "$(mktemp -d)"
brocanth create try --with correlation --brocanth-path ~/path/to/brocanth/crates/brocanth
cd try && cargo build && cargo test && cargo clippy --all-targets -- -D warnings
```

`--brocanth-path` names the core crate's directory, `crates/brocanth`,
not the checkout.

### Optional behaviour: `project.has`

A module adapts to another module with `project.has(key)` in
`contribute`, and declares it with `adapts_to(key, what_changes)`.
`security-headers` adds the admin's inline script hash only when `admin`
is chosen:

```rust
// manifest
.adapts_to("admin", "the CSP allows the admin's inline script by hash")
// contribute
let config = if project.has("admin") { WITH_ADMIN } else { DEFAULT };
```

`snippet::configurable` asks `project.has("config")`, so every module with
declared settings adapts to `config` (13 today). The generated
`brocanth.toml` lists, per module, `adapted_to`: the declared keys that
were chosen. `create` prints them under the module in its receipt.

Add `after(key)` as well when you write into a slot the other module also
writes, so the order is defined.

### Static output

A module that only builds, renders, configures or ships says
`outside_runtime()`. Without it, `brocanth_scaffold::clash` refuses
the module beside `static`: `create` and `add` say so, and the chooser
unticks it. Do not add it to a module that registers a plugin, a hook or a route:
a static export has no process to run them.

### Conflicts

Two modules that cannot stand together say so in a manifest:

```rust
.conflicts(
    "observability-logs",
    "both write a line per request; `observability-logs` writes JSON on stdout",
)
```

That is `logging`'s manifest, the only pair today. Declare it on one
side, the side that gives way: `clash` reads both manifests and tells the
user to drop the declaring module. A third-party module adds a pair
without a change to the CLI.

### Checklist

1. A crate with a `scaffold` feature, off by default, and its runtime
   behind a default `runtime` feature that the CLI turns off.
2. `Manifest::new(key, gist)`: a key with no hyphen, unless it has a
   parent; a gist with no full stop.
3. `category` for each kind of seam it writes; `after` for order,
   `needs` for a requirement with its reason, `adapts_to` for each other
   module it asks `project.has` about, `conflicts` for a module it cannot
   stand beside.
4. `absent` that says what the project lacks without the module.
5. Plugins in `src/plugins/<key>/mod.rs`, registered through
   `snippet::plugin` or `snippet::hooking`.
6. Settings through `snippet::configurable`; `Deserialize` only when
   `project.has("config")`.
7. A `PagesRoutes` line for each route the module adds.
8. An `AgentsSections` block for each rule an agent must follow.
9. `outside_runtime()` when the module does not run in the server.
10. No stage waits for async work: a lazy pool and a readiness check.
11. Two lines in the CLI, the goldens blessed and read, a generated
    project built.

## 5. Gaps

Found while writing this guide. Input for the module-design discussion.

1. ~~**No slot for `plugins::wrap`.**~~ Closed: `Slot::ServerWrap`, with
   a generated `ServerWrap` trait (§3.1). The first line is the outermost
   wrap; `after` places a wrap inside another. `observability` and `rsx`
   (static files) use it. `logging` does not: its line needs the route and
   `RequestState`, which only a hook has, and a hook times a request as
   well as a wrap does.
2. ~~**Conflicts are not in the manifest.**~~ Closed: `Manifest::conflicts`,
   read by `brocanth_scaffold::clash` for `create`, `add` and the chooser.
3. ~~**Optional dependencies are invisible.**~~ Closed:
   `Manifest::adapts_to`, held to every `project.has` by a test, and
   listed as `adapted_to` in `brocanth.toml`.
4. ~~**No category field.**~~ Closed: `Manifest::category`. A test holds
   each module to the slots its categories open, and `--with`'s help
   leads with the categories. The chooser does not group by it yet.
5. ~~**No config form for a hooking plugin.**~~ Closed:
   `brocanth-config` has `plugin_hooking_from_config` and
   `plugin_hooking_from_config_with`, and `snippet::configurable_hooking`
   declares a hooking plugin's settings. `csrf`, `security-headers` and
   `rate-limit` read `[csrf]`, `[security_headers]` and `[rate_limit]`.
   See `docs/module-flags.md`.
6. **The hooking registration is hand-formatted text.** `snippet::plugin`
   writes the statement for you. `snippet::hooking` takes the whole
   statement as a string, so each module repeats the rustfmt shape and the
   `<T as Plugin>::new(..)` form. A `snippet::hooking_line(project, ty,
   config_expr)` would remove that.
7. ~~**A config section is parsed back from text.**~~ Closed: a
   declared setting names its section, and `snippet::configurable` writes
   it. `docs/module-flags.md` has the design.
8. **An out-of-tree module has no test harness.** The goldens and the
   formatting test live in the CLI crate. A third-party author must fork
   the CLI to run them. A `brocanth_scaffold::testing` helper (generate a
   project with these keys, run rustfmt over it, compare a golden) would
   let a module crate test itself.
9. **`NodeDependencies` is still unread.** `rsx-tailwind` writes the
   whole `package.json`. A second node module would collide on that file.
10. ~~**`Plugin::provide` cannot see the request.**~~ Closed:
    `Plugin::provide_for_request(&self, &Request, &RequestState, &mut
    ContextBuilder)`, called by `App::context_for` after the request hooks
    (§3.1). A hook and `ctx.hooked` was the other answer; it was rejected
    because it skips the compile-time check `ctx.provided` gives, and a
    value every page reads (a locale, a tenant) deserves that check. The
    method stays synchronous and infallible: a wait or a refusal belongs to
    a hook. `brocanth-i18n` still provides a `Negotiator` that each page
    calls; it can now provide the negotiated locale.
11. **Three `rsx` branches remain in `create.rs`**, down from sixteen: the
    starter pages `index.rs`, `about.rs` and `users/[id].rs`. The rest
    moved to `Slot::PagesRenderers`, `Slot::ServerWrap`, the new
    `Slot::MainMods`, `Slot::AgentsSections` (which now lands where the
    views sections were) and the children's manifests. The pages stay
    because each path has one writer; see `docs/modules.md` §8.
