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

Twenty-six crates and two outside engines. Twenty-two offer a module;
twelve keep it beside the crate behind an off-by-default `scaffold`
feature, which is the pattern. `rsx` and `jt` are in the CLI, because
neither has a crate in this workspace to host one.

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
| `rsx` | **renderer** | `PagesRenderers`, five template files | — |
| `rsx-tailwind` | tooling | `CommandsMod`, `NodeDependencies`, `MiseTasks` | — |
| `rsx-htmx` | renderer + edge | written by hand | `partial_depth` on `Ctx` |
| `commands` (in-CLI) | tooling | `CommandsMod`, `binary` | — |
| `brocanth-test` | tooling | dev-dependency only | — |
| `jt` | renderer | `PagesRenderers`, `PagesRoutes`, two files | — |

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

**Seam: `Slot::PagesRenderers`.** §5.

### 4.2 Edge

**Wraps the request, takes config, and usually publishes a value onto the
per-request context.** The user's own example: a request logger. Also
sessions, auth, CSRF, the five Gorilla hooks in `brocanth-middleware`.

Seams: `MiddlewareCatalog` to *offer* a layer, `Slot::PluginsLayers` to make
the offer findable by name, `Slot::AppSetup` to mount it at a pattern, and
`ProvidesContext<T>` to publish a value.

**All four work.** §6. What a provider still cannot do is read the
request it is providing for.

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

### What was built

**`Slot::PagesRenderers`**, and no new type. The sketch here proposed a
`Render` declaration carrying an extension, a loader and a content type.
Writing it showed the extension and the content type have no reader: the
route table in `table.rs` already takes `.rs` files and ignores every other
extension, so a `.jt` file beside a `.rs` page needs nothing said about it,
and the content type is the page handler's. What was left was the line
itself, which is what every other module already writes into a slot. A
second mechanism for one line would be a second mechanism.

So `rsx` writes its own half of `src/pages/mod.rs`:

```rust
// crates/brocanth-cli/src/modules/rsx.rs
fn contribute(&self, p: &Project, s: &mut Scaffold) {
    s.at(Slot::PagesRenderers).block(format!(
        "pub const TITLE: &str = {:?};\n\n{PAGES_RSX_RS}\n", p.title
    ));
    s.file("src/pages/layout.rsx", layout_rsx(p));
    // ...
}
```

and the generator reads the slot where it used to write `rsx` by hand:

```rust
for block in built.slot(Slot::PagesRenderers) {
    out.push_str(block);
}
```

Two renderers in one project is two blocks and not a conflict.

**The two literals in `brocanth-pages` are gone.**

`layout.rsx` became `layout_in(dir)`: the layout is the file named `layout`
with any extension that is not `.rs`. `.rs` is excluded because `layout.rs`
would be a page at `/layout`, and a file cannot be both. Two layouts in one
directory is an error that names both, through the macro's own error path
rather than a panic.

`rsx::Template::render_to_string(..)` became `crate::pages::render_oob(..)`.
The other two renders in the expansion already came back through the
project's own `crate::pages::render` and `render_with`; the out-of-band one
reached past them and named the engine. It was the only place the macro knew
which engine made the page.

### What the move cost, measured

Every file a generated project gets is **byte-identical**, across all five
golden shapes, except two:

- `brocanth.toml`, the receipt, which lists files in the order they were
  written. A module's files now sort by path instead of by the order they
  were pushed.
- `.gitignore` and `src/commands/mod.rs` each lost a stray blank line.
  `Writer::line` ends the line it writes, and both readers added a second
  newline. Nobody saw it until a second module wrote into those slots.

`create.rs` lost 558 lines and 15 of its 31 `rsx` branches.

### The part worth more than the seam

**A `.jt` template is an output schema.** `jt-core` holds the AST: it knows
the keys, the nesting, which fields are `?:`, and which values are loops.
Step 4 of the surface work derives a page's `gives` from
`Result<Json<T>, _>` and a `zod::Node`. A `.jt` page could produce the same
`Node` from the template itself.

That closes the last hand-written line in the document: today a response
schema needs a `#[derive(Entry)]` struct *and* the code that fills it. With
`jt`, the template that writes the JSON is the thing that describes it.

This is a second step and not a condition of the first. A `.jt` page
appears in the OpenAPI document today with no response schema.

### `jt` attached, and the seam held

The test of §5 was whether a second renderer costs a manifest and a
dependency. It did. `crates/brocanth-cli/src/modules/jt.rs` is one
`Manifest`, one `Dep`, one block into `Slot::PagesRenderers` and two files,
plus one line in `ALL`. **Nothing in `brocanth-pages`, `create.rs` or the
route table knows it exists.**

```rust
fn contribute(&self, _project: &Project, s: &mut Scaffold) {
    s.at(Slot::PagesRenderers).block(PAGES_JT_RS);
    s.file("src/pages/status.rs", STATUS_RS);
    s.file("src/pages/status.jt", STATUS_JT);
    s.at(Slot::PagesRoutes).line("/status");
}
```

Two renderers in one project is two `pub mod`s over one tree:

```rust
pub mod views   { rsx::load_dir!("src/pages"); }   // compiles .rsx
pub mod payload { jt::load_dir!("src/pages"); }    // compiles .jt
```

and a real request:

```
$ curl -i localhost:3000/status
HTTP/1.1 200 OK
content-type: application/json

{"name":"api","version":"0.1.0","ok":true,"parts":["pages","router"]}
```

Three things the attachment cost that the sketch did not predict.

**`jt` named modules by a different rule.** It replaced `-` and `.` and
nothing else, so `[id].jt` would have panicked in `format_ident!`. A page
tree holds `users/[id].rs`, and a route and its template have to land on
the same module path. `jt` now uses rsx's rule, which `brocanth-pages`
already reimplements. Three implementations agree on purpose.

**`sibling()` could not find a workspace.** `--brocanth-path` resolves a git
dependency to a checkout beside the repository. `rsx` and `zod` are one
package at their root; `jt` is a workspace whose package is `crates/jt`,
which is how this repository is laid out too. Cargo's error for a path
pointing at a virtual manifest names neither the flag nor the fix.

**`Slot::PagesRoutes` had to exist.** A route is a file, so a module that
writes a page adds a route, and `src/pages/mod.rs` carries a test asserting
the whole table. §6b.

## 6b. The bug `jt` found

`brocanth create --with openapi` has been writing projects whose own test
suite fails, since the day that module landed.

```
---- pages::tests::the_table_is_the_shape_of_the_directory ----
  left: ["/", "/about", "/openapi.json", "/openapi.yaml", "/users/{id}"]
 right: ["/", "/about", "/users/{id}"]
```

The generated test lists every route the project has, because a route is a
file and the list is the only other record of it. The list was a literal.
`openapi` writes two pages, so from its first day a project that chose it
did not pass `cargo test`.

Nothing caught it because **`the_scaffold_compiles` ran `cargo build`**. A
generated project carries a test suite and the generator never ran it.

Both are fixed: `Slot::PagesRoutes` carries a module's routes into the
list, and the compile test runs `cargo test`.

A second one, found by reading the response `jt` produces:

```
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
content-type: application/json
```

`sent()` seeds `text/html` and its comment says a page's own header "is
applied after and therefore wins". `with_header` appends, because
`set-cookie` has to repeat, so both went on the wire and the client picked.
`brocanth-input`'s `Json<T>` had the same bug and its test passed, because
the header was present — twice. `Response::without_header` now exists and
`sent` drops the seeded type when the page set one.

Both bugs are the same shape: **a claim in a comment that no test read
back.**

## 6. The context seam, which reaches a handler now

The user's second kind — *"middleware which needs config and injects into
context, such as a request logger"* — did not work end to end. The failure
was quiet.

`plugin_context.rs` was already good work: the installed provider set lives
in the application's type, so `cx.get::<RequestId, _>()` either compiles or
names the missing type. What was missing was the last hop. The generated
`Ctx` was a plain struct — verb, path, query, body, params, headers — and
the `Slot::AppContext` lines landed inside `plugins::announce!`, which runs
**once at startup**. Five generated modules told the developer to call
`cx.extract()` from a handler, and that line did not compile.

### What it took

`App` cannot name the builder's type, and `Ctx` has to. So the generator
writes the provider list out, once, from the same ordered list `register!`
uses:

```rust
// src/plugins/mod.rs, written
type WithLogging = After<crate::plugins::logging::Logging, Base>;
type WithSessions = After<crate::plugins::sessions::Sessions, WithLogging>;
pub type Installed = WithSessions;
```

`After<P, E>` is new in `brocanth::plugin_context`, and it exists for
legibility: spelled out, one entry is
`<<P as Plugin>::Provides as Provided>::Install<E>`, and three of those
nested is a 200-character type whose rustfmt wrapping a generator would
have to reimplement. One alias per plugin keeps every line short.

The list cannot drift from the chain, because one call writes both:

```rust
// brocanth_scaffold::snippet
pub fn plugin(s: &mut Scaffold, project: &Project, key: &str, ty: &str) {
    s.at(Slot::AppPlugins).block(plugin_line(project, key, ty));
    s.at(Slot::PluginsInstalled).line(format!("crate::plugins::{key}::{ty}"));
}
```

Eleven modules call it. If the two lists ever disagree the project does not
compile, which is the right failure.

`App` then keeps a factory rather than a context, because `App::context()`
is per-request by design:

```rust
type ContextFor = Box<dyn Fn() -> Context<plugins::Installed> + Send + Sync>;
```

The built application moves into that closure at the end of `build()`.
Nothing else needs it, and it is what keeps the providers reachable once
`build` has returned a struct that cannot name their type.

### What a handler writes

```rust
let session = ctx.provided::<Session, _>().expect("a dispatched request");
```

Ask for a type nothing installs and the build stops:

```text
error[E0277]: the trait bound `Base: Has<NobodyProvides, _>` is not satisfied
  --> src/pages/about.rs:62:32
   |
62 |         let _ = ctx.provided::<NobodyProvides, _>();
   |                                ^^^^^^^^^^^^^^ the trait `Has<NobodyProvides, _>`
   |                                               is not implemented for `Base`
```

**Why `Option` and not `&T`.** The compile-time claim is untouched: the
`Has` bound is the whole check, and it fires on the type. The `Option` is
about where the `Ctx` came from, not about `T`. A request dispatched by
`src/app.rs` always carries a context, so it is always `Some`; a `Ctx`
built by hand in a test has no application behind it and therefore no
providers, and `None` says that rather than panicking. Returning `&T` would
have meant a panic on exactly the path the module exists to make safe.

`the_scaffold_compiles` appends both assertions to the generated `about.rs`
and runs them, so the claim is checked from inside a real project.

### What is still a provider and not a layer

`Plugin::provide(&self, &mut ContextBuilder)` does not see the request. A
value that depends on the request — a session read from a cookie, a current
user — cannot be built there yet. That is the next piece of this seam, and
it is a change to the `Plugin` trait rather than to the generator.

A value a *layer* writes is a different thing again, and it cannot have
this guarantee: a layer is mounted at a pattern, so a type it publishes is
present on some paths and not others. `try_get` is the honest reader for
that, and it answers `Option` for a reason that is about the path.

---

## 7. Every seam, and its state

### Build time — `Slot`, 16 variants

| Slot | State |
| --- | --- |
| `AppUses`, `AppPlugins`, `AppSetup`, `AppContext` | read |
| `PluginsMod`, `PluginsLayers` | read |
| `PluginsInstalled` | read — the provider list, §6 |
| `PagesItems`, `CommandsMod`, `ConfigSections` | read |
| `GitIgnore`, `AgentsSections` | read |
| `CargoExtra` | **declared, never read** |
| `AppItems` | **declared, never read** |
| `AppBeforeRouter` | **declared, never read** |
| `MiseTasks` | read — `rsx-tailwind` was its first customer |
| `NodeDependencies` | **declared, never read** — `rsx-tailwind` wants it |
| `PagesRenderers` | read — the renderer seam, §5 |
| `PagesRoutes` | read — a module's page joins the table's own test, §6b |
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
| `ProvidesContext<T>` | works, and reaches a handler through `Ctx::provided` (§6) |
| `Plugin::provide` seeing the request | **missing** — a provider cannot read a cookie (§6) |
| `RouteSource` + `mount_routes` | works; **never generated** (§4.6) |
| A renderer registry | **does not exist** (§5) |

---

## 8. What to do, in order

1. ~~**Make `rsx` a real module.**~~ **Done.** `Slot::PagesRenderers`, the
   three `contribute()` bodies filled, and `brocanth-pages` no longer names
   `rsx`. `Slot::MiseTasks` and `Slot::GitIgnore` got their first real
   customer on the way, and `Slot::CommandsMod` its second.

   **Sixteen `rsx` branches are still in `create.rs`**, and they are the
   ones the slot model does not reach: the three `@DEPTH_*` markers
   substituted into `Ctx`, the `app.rs` and `main.rs` pieces for static
   files, the `brocanth-assets` dependency, and the `AGENTS.md` sections,
   which are interpolated at a position rather than appended. Each needs a
   slot that does not exist. None of them blocks a second renderer: `jt`
   adds no field to `Ctx`, serves no static file and mounts nothing.
2. ~~**Add `jt` as the second renderer.**~~ **Done.** A manifest, a
   dependency, one block and two files. It cost three things the sketch did
   not predict, all of them written up in §5: `jt` named modules by its own
   rule, `sibling()` could not resolve a workspace checkout, and
   `Slot::PagesRoutes` had to exist. It also found two live bugs (§6b).
3. ~~**Give `Ctx` the plugin context and an extractor.**~~ **Done.**
   `Ctx::provided::<T, _>()`, bounded by `Has`, over a provider list the
   generator writes from the same ordered list `register!` uses. What
   remains of this seam is `Plugin::provide` seeing the request (§6).
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
