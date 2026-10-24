# What a module is, and where it attaches

A reader arrives here with a crate and one question: *where does it plug in?*
Today the answer is "read `create.rs` and guess". This document is the answer
written down.

It does three things. It names the **kinds** of module brocanth has, by the
seams each kind touches. It lists **every seam** that exists, every one that
is declared and unread, and every one that is missing. And it checks the
list against Rails, Django and Laravel, which have solved this three times.

The occasion is `jt`: compile-time JSON templates, a sibling of `rsx`. A
`.jt` file belongs in `src/pages` exactly as an `.rsx` file does. **There is
nowhere to say so.** `rsx` is not a module in brocanth; it is a special case
written into the generator by hand, and the second renderer is the one that
proves it.

---

## 1. The two planes brocanth already has

A module attaches at two different times, and the two have nothing in
common except the word "hook".

**Build time.** `brocanth create` writes a project. A module implements
`brocanth_scaffold::Module`, declares a `Manifest`, and writes text into
named `Slot`s of files the generator owns. The output is source code. The
compiler checks it afterwards, and nothing is dynamic.

**Run time.** A project runs. A plugin implements `brocanth::plugin_api::Plugin`,
is handed its own config value, and passes through a recorded lifecycle.
`AppBuilder` carries the installed provider set and the installed hook set in
the application's *type*, so asking for a plugin nobody registered does not
compile.

These are genuinely separate. A module may use one, the other, or both.
`logging` uses both: it writes a `plugins/logging.rs` at build time, and
that file registers a plugin at run time.

```text
brocanth create            cargo build             a request
      │                         │                      │
  Module::contribute   ──▶  the written source  ──▶  Plugin lifecycle
  Slot::*                    pages! / html!          hooks, context, routes
  (16 slots, 11 read)        (macros read the tree)  (5 stages + 5 seams)
```

**The gap is the middle column.** The macros that read `src/pages` know one
file extension, `.rsx`, and they know it as a literal. No module can add a
second. That is the renderer seam, and §5 is about it.

---

## 2. How the other three do it

| | Rails | Django | Laravel |
| --- | --- | --- | --- |
| **The unit** | `Railtie` / `Engine` | app in `INSTALLED_APPS` | `ServiceProvider` |
| **Discovery** | gem on the load path | written list in settings | `composer.json` `extra.laravel.providers`, with `dont-discover` to opt out |
| **Two phases** | `initializer` blocks, ordered `:before`/`:after` | `AppConfig.ready()`, after the registry is full | `register()` then `boot()` |
| **Config** | `config.*` keys, `config.app_generators.orm` | settings module | `mergeConfigFrom`, `publishes` |
| **A renderer** | `ActionView::Template.register_template_handler :ext, Handler` | `TEMPLATES[].BACKEND`, a dotted path to an engine class | `View::addExtension($ext, $engine)`, `loadViewsFrom` |
| **Middleware** | `config.middleware.use` / `insert_before` | `MIDDLEWARE`, an ordered list | kernel groups and aliases |
| **Routes** | `Engine` mounts at a prefix the app chooses | `include()` in the URLconf | `loadRoutesFrom` |
| **CLI** | `rake_tasks`, `generators`, `console`, `runner`, `server` | `management/commands/` found by convention | `$this->commands([..])` |
| **Deferred work** | `ActiveSupport.on_load(:active_record)` | `ready()` | deferred providers |

Three findings worth keeping.

**All three have a renderer registry, and all three keep it tiny.** It is one
call taking an extension and a handler. Rails and Laravel key on the file
extension; Django keys on a backend class and lets it own a directory. None
of them makes the template engine a special case of the framework, and all
three shipped a second engine (Haml, Jinja2, Blade-then-Twig) because of it.

**All three separate "declare" from "use".** Rails has `initializer` order,
Django has `ready()` after the registry is full, Laravel has `register` then
`boot`. brocanth already has this and goes further: `configure`, `register`,
`after_config`, `configure_router`, `before_start`, each completing for every
instance before the next starts.

**Only Laravel's publishing has no brocanth equivalent, and brocanth does
not need one.** `vendor:publish` copies a package's files into the app so the
developer can edit them. brocanth writes the file once, at `create`, and the
developer owns it from then on. The generator *is* the publish step.

---

## 3. The inventory

Twenty-six crates. Twenty-one offer a module; twelve of those keep it beside
the crate behind an off-by-default `scaffold` feature, which is the pattern.

| Crate | Kind (§4) | Build-time seams | Run-time seams |
| --- | --- | --- | --- |
| `brocanth-cache` | service | `ConfigSections`, `AppPlugins` | `Plugin`, `Services` |
| `brocanth-storage` | service | `ConfigSections`, `GitIgnore` | `Plugin`, `Services` |
| `brocanth-model` | service + lifetime | `ConfigSections`, `CommandsMod` | `Plugin`, `ModelDispatch`, shutdown |
| `brocanth-config` | service | `ConfigSections`, `AppPlugins` | `Plugin` |
| `brocanth-jobs` | lifetime | `ConfigSections`, `CommandsMod` | `Plugin`, both shutdown phases |
| `brocanth-realtime` | lifetime | `ConfigSections` | `Plugin`; **no transport** |
| `brocanth-middleware` | edge | `PluginsMod`, `PluginsLayers`, `AppSetup` | `MiddlewareCatalog`, `HookSource` |
| `logging` (in-CLI) | edge | `PluginsMod`, `PluginsLayers`, `AppContext` | `MiddlewareCatalog`, `ProvidesContext` |
| `sessions` (in-CLI) | edge | `PluginsMod`, `AppContext` | `ProvidesContext` |
| `brocanth-auth` | edge + bundle | `ConfigSections`, `AppContext` | `Plugin`, `ProvidesContext` |
| `brocanth-policy` | edge | `AppUses` | `ModelHook` |
| `brocanth-input` | boundary | `PagesItems` | the page signature |
| `brocanth-schema` | boundary | `PagesItems` | — |
| `brocanth-openapi` | boundary + bundle | page file at `/openapi.json` | `RouteSource` (unmounted) |
| `brocanth-client` | boundary + tooling | `CommandsMod` | reads `pages::described()` |
| `brocanth-resource` | bundle | **blocked** | `RouteSource`, nothing mounts it |
| `brocanth-admin` | bundle | not on the menu | `RouteSource`, nothing mounts it |
| `brocanth-events` | service | — | `ModelEvent` carrier |
| `brocanth-assets` | **renderer-like** | written by hand in `create.rs` | — |
| `rsx` | **renderer** | **written by hand in `create.rs`** | — |
| `rsx-tailwind` | tooling | `CommandsMod`, `NodeDependencies`, `MiseTasks` | — |
| `rsx-htmx` | renderer + edge | written by hand | `partial_depth` on `Ctx` |
| `commands` (in-CLI) | tooling | `CommandsMod`, `binary` | — |
| `brocanth-test` | tooling | dev-dependency only | — |
| **`jt`** | **renderer** | **nothing to write into** | — |

Three rows say "written by hand". `rsx::Rsx::contribute()`, `Tailwind::contribute()`
and `Htmx::contribute()` are each one comment: *"Still written by `create.rs`.
Moving here next."* The templates module is the oldest thing in the generator
and the last one that is not a module.

---

## 4. Seven kinds

A kind is not a category somebody invented. It is a recurring set of seams,
and naming it is useful only because the set recurs.

### 4.1 Renderer

**Turns a file under `src/pages` into part of a response.** `rsx` makes HTML.
`jt` makes JSON. `brocanth-assets` is close enough to share the seam: it
reads a directory at build time and embeds what it finds.

What it needs to declare: an extension, a loader line, whether it has
layouts and what one is called, and a content type. What it must *not* need:
a change to `brocanth-pages`.

**Seams: none exist.** §5.

### 4.2 Edge

**Wraps the request, takes config, and usually publishes a value onto the
per-request context.** The user's own example: a request logger. Also
sessions, auth, CSRF, the five Gorilla hooks in `brocanth-middleware`.

Seams: `MiddlewareCatalog` to *offer* a layer, `Slot::PluginsLayers` to make
the offer findable by name, `Slot::AppSetup` to mount it at a pattern, and
`ProvidesContext<T>` to publish a value.

**The offer and the mount work. The context does not reach a handler.** §6.

### 4.3 Service

**Takes config, publishes a value, and is called by handlers.** The cache is
the clean case: a config section, a `Plugin` that builds one, a `Services`
entry, and nothing else. Storage and config itself are the same shape.

Seams: `Slot::ConfigSections`, `Plugin::register`, `Services`. All work.

### 4.4 Subsystem with a lifetime

**A service that owns a thread, a pool or a queue, and so owns a shutdown.**
Jobs is the reason `ShutdownPhase` has two phases: a runner told "stop" must
first stop *taking* work and only then finish what it holds.

Seams: `Plugin::before_start`, `ShutdownPhase::Begin`, `ShutdownPhase::Drain`.
All work, and the ordering is reverse-registration, which is dependency
order.

### 4.5 Boundary

**Reshapes what arrives or what leaves, and describes itself.** `zod` through
`brocanth-input` checks a body; `brocanth-openapi` publishes what the check
requires; `brocanth-client` turns the same table into a command line.

Seam: the page's signature, read by `pages!`, emitted as `pages::described()`.
This one is new and it works. It is also the template for the others: **one
declaration, several readers, no author.**

### 4.6 Route bundle

**Ships screens the application mounts at a prefix it chooses.** The admin
has twelve, OpenAPI has two, `brocanth-resource` has five per entity.

Seam: `RouteSource` plus `App::mount_routes::<T>("/admin")`. The trait, the
prefix suggestion, the name filter and `unmounted_route_sources` are all
written and tested. **`create.rs` emits no mount**, so no generated project
can take the offer. This is what blocks `brocanth-resource` and the admin
from the menu.

### 4.7 Tooling

**Adds a subcommand, a task, or a dev dependency.** `brocanth css`, the jobs
worker, migrations, `api` from `brocanth-client`.

Seams: `Slot::CommandsMod`, `Slot::MiseTasks`, `Slot::NodeDependencies`,
`Scaffold::binary`. The first works. The other two are declared and unread.

---

## 5. The renderer seam

### What is hardcoded

Four places, all naming `rsx` as a literal:

| Where | What it says |
| --- | --- |
| `brocanth-pages/src/lib.rs:223` | `root.join(current).join("layout.rsx")` |
| `brocanth-pages/src/lib.rs:420` | `rsx::Template::render_to_string(..)` |
| `brocanth-cli/src/commands/create.rs:2446` | `pub mod views { rsx::load_dir!("src/pages"); }` |
| `brocanth-pages/src/table.rs` | which extensions are pages and which are views |

`brocanth-pages/src/naming.rs` even reimplements rsx's `[id]` → `_id_` rule
by hand, with a comment saying the two have to agree. That comment is the
seam, written as a warning instead of as an interface.

### What `jt` needs, and what it already has

`jt` and `rsx` have the **same build-time shape**, which is the whole reason
this is cheap:

```rust
rsx::load_dir!("src/pages");   // a tree of pub mods, one per file
jt::load_dir!("src/views");    // a tree of pub mods, one per file
```

Both are proc macros. Both read a directory relative to `CARGO_MANIFEST_DIR`.
Both `include_bytes!` each file so cargo rebuilds. Both turn `[id]` into
`_id_`. The difference is only what a template *is*: `rsx` makes an `Element`,
`jt` makes a value with a `ToJson`, and `jt` has no layouts because JSON has
no document to wrap.

### The proposal

A manifest declares a renderer, and `brocanth-pages` reads the declaration
instead of a literal.

```rust
// in a module's manifest
.renders(
    Render::new("jt", "jt::load_dir!")
        .content_type("application/json")
        .no_layout()
        .render_with("jt::to_string"),
)
```

Which produces, in `src/pages/mod.rs`:

```rust
pub mod views   { rsx::load_dir!("src/pages"); }   // from the rsx module
pub mod payload { jt::load_dir!("src/pages"); }    // from the jt module
```

and one new slot for the lines:

```rust
/// `src/pages/mod.rs`, one `pub mod` per renderer, each wrapping a
/// `load_dir!`. Written in declared order, so a project with two
/// renderers reads top to bottom.
PagesRenderers,
```

`brocanth-pages` takes the set of renderers as a macro argument rather than
a literal — `pages!("src/pages", describe, renderers = [..])` — and the
layout walk at line 223 asks each renderer for its layout name instead of
joining `"layout.rsx"`.

Three properties this keeps:

1. **Two renderers in one project is the normal case, not a conflict.** A
   page tree holds `index.rsx` beside `users.jt`. They are different
   extensions, so the table never has to choose.
2. **A renderer that is not chosen costs nothing.** No dependency, no
   `pub mod`, no line in the table.
3. **The extension decides the content type.** `handle` in a `.jt` page
   answers `application/json` without saying so, the same way a `.rsx` page
   answers `text/html` without saying so.

### The part worth more than the seam

**A `.jt` template is an output schema.** `jt-core` holds the AST: it knows
the keys, the nesting, which fields are `?:`, and which values are loops.
Step 4 of the surface work derives a page's `gives` from `Result<Json<T>, _>`
and a `zod::Node`. A `.jt` page could produce the same `Node` from the
template itself.

That closes the last hand-written line in the document: today a response
schema needs a `#[derive(Entry)]` struct *and* the code that fills it. With
`jt`, the template that writes the JSON is the thing that describes it.

This is a second step and not a condition of the first. The seam is worth
building for the extension alone.

---

## 6. The context seam, which is broken

The user's second kind — *"middleware which needs config and injects into
context, such as a request logger"* — does not work end to end today, and the
failure is quiet.

`plugin_context.rs` is good work. The installed provider set lives in the
application's type, so `cx.get::<RequestId, _>()` either compiles or names
the missing type. `Slot::AppContext` lets a module write a line that
publishes a value.

**But the generated `Ctx` is a plain struct** (`create.rs:2184`): a verb, a
path, a query, a body, params and headers. No plugin context, no `extract`.
The `AppContext` lines land inside `plugins::announce!`, which runs **once at
startup**. A handler cannot read what a layer published for *this* request.

Three generated modules told the developer to call `cx.extract()` from a
handler. That line does not compile. The prose was corrected; the gap is the
type.

**What is needed:** `Ctx` carries the per-request `Context<E>`, and
`ctx.get::<RequestId>()` is bounded by the same `Has` trait, so a handler
reading a value no layer publishes fails the build with the module to add.

Until then the "edge" kind is half a kind: it can wrap a request and it
cannot hand anything forward.

---

## 7. Every seam, and its state

### Build time — `Slot`, 16 variants

| Slot | State |
| --- | --- |
| `AppUses`, `AppPlugins`, `AppSetup`, `AppContext` | read |
| `PluginsMod`, `PluginsLayers` | read |
| `PagesItems`, `CommandsMod`, `ConfigSections` | read |
| `GitIgnore`, `AgentsSections` | read |
| `CargoExtra` | **declared, never read** |
| `AppItems` | **declared, never read** |
| `AppBeforeRouter` | **declared, never read** |
| `MiseTasks` | **declared, never read** — `rsx-tailwind` wants it |
| `NodeDependencies` | **declared, never read** — `rsx-tailwind` wants it |
| `PagesRenderers` | **missing** — §5 |
| `AppMounts` | **missing** — §4.6, for `mount_routes` |

A module that writes into an unread slot is recorded and discarded in
silence. That has already happened once, with a `.gitignore` line.

### Run time — the plugin API

| Seam | State |
| --- | --- |
| `Plugin` lifecycle, five stages | works |
| `Services`, typed cross-plugin lookup | works |
| `MiddlewareCatalog`, offer-not-mount | works |
| `HookSource`: request, response, error | works, and costs nothing unregistered |
| `ModelEvent` / `ModelHook` | works |
| `plugin_order`, declared `after`/`before` | works |
| `ShutdownPhase::Begin` / `Drain` | works |
| `ProvidesContext<T>` | works at startup; **unreachable from a handler** (§6) |
| `RouteSource` + `mount_routes` | works; **never generated** (§4.6) |
| A renderer registry | **does not exist** (§5) |

---

## 8. What to do, in order

1. **Make `rsx` a real module.** Move the four hardcoded places behind a
   `Render` declaration and fill the three empty `contribute()` bodies. No
   new capability; the generated output must not change by one byte, which
   the golden snapshots already check.
2. **Add `jt` as the second renderer.** This is the test of step 1. If it
   takes more than a manifest and a `Cargo.toml` line, step 1 is not done.
3. **Give `Ctx` the plugin context and an extractor.** Unblocks the edge
   kind, and with it the request logger the user named.
4. **Add `Slot::AppMounts` and emit a mount.** Unblocks `brocanth-resource`
   and the admin, both of which are written and unreachable.
5. **Read the five unread slots, or delete them.** `MiseTasks` and
   `NodeDependencies` have a waiting customer in `rsx-tailwind`.

Steps 1 and 2 are one piece of work and should be one branch. Step 3 is
independent. Step 4 is independent. Step 5 is small and can ride with any of
them.

**The claim this document makes is falsifiable:** after step 2, adding a
third renderer should be a manifest and a dependency. If it is not, the seam
is in the wrong place.
