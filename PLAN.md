# Brocanth

A server framework for HTML-first applications in Rust.

## 1. Why this exists now

The Crystal brocanth is a 1,272-line prototype across 16 files — the 405-file
count includes vendored dependencies. It serves **GET only**, has **no
persistence layer at all**, and stopped mid-feature in February with
uncommitted work in the tree. So this is a build, not a port. What carries
over is its design, which was argued and recorded.

**The slot is genuinely open, and one finding decides it.** Loco.rs — the
project whose tagline is "Rails for Rust" — *removed server-rendered
scaffolding at 1.0*. Its own generator reference says the `--html` and
`--htmx` flags now "error with a pointer to the React SPA frontend that
replaced server-rendered views." The one Rust framework aiming at this
position walked away from it. Supporting detail: it reached 1.0 after a
twelve-month gap with no release and three months of zero commits, two
contributors account for 1,340 of ~1,400 commits, and its job queue ships
this in its own documentation — *"There is no automatic retry or backoff in
any queue driver, so without `retry` a failed job is terminal."*

The other contender, `cot`, is better engineered and says plainly it is "by no
means production-ready yet". It has the two things nobody else in Rust has —
migrations generated from Rust structs, and an admin panel — and no background
jobs of any kind, no scheduler, no i18n, no pagination, no authorization, and
two middlewares in its middleware directory.

Current versions, because most write-ups are stale: Rails **8.1.4**, Django
**6.1**, Laravel **13**, Phoenix **1.8.15** with LiveView **1.2.12**. Axum is
still **0.8.9** with no 1.0 and no due date. Salvo reached 1.0 this month;
`apalis` reached 1.0 two days ago, which is the first stable anchor the Rust
job-queue story has ever had.

## 2. Positioning, in one sentence

Rails owns the deploy path and ships no admin and no pagination. Django owns
the data and admin path and ships no deployment, no health check and no
secrets. Phoenix has the best realtime and compile-time story of the four and
no secrets management, no rate limiting and no job queue. Laravel ships the
most infrastructure and checks none of it until runtime.

**The open position is Django's schema-first data model and generator
discipline, Phoenix's compile-time checking taken several steps further,
Rails' finish, and a partial-navigation story that is already half-built in
dp-swap** — with the honest admission, printed in the README, that there is no
console and the compile loop is seconds rather than milliseconds.

## 3. Decisions carried from the Crystal original

Each was argued there, with the reason on record.

| Decision | Why | Change here |
|---|---|---|
| File-based routes with typed helpers | The filesystem is the route table; a wrong arity is a compile error | Drop the arity ceiling — it existed only because Crystal macros cannot emit variadics |
| Layouts nest by directory | Rails has no equivalent; it is a real differentiator for HTML | Pass a writer rather than returning a String per layer: three allocations become zero |
| Pattern-scoped middleware with exclusions | Nothing in Rust has this; tower layers attach per router, not per pattern | Keep, as a second matcher consulted before dispatch |
| Plugins never autowire | *"I do not want plugins to register middleware themselves. I want to manually inject middlewares since I may want to configure the pattern."* | Keep the five-hook lifecycle verbatim |
| Config is enriched by plugins, not pre-declared | *"we want the config object of the app to be enriched when the plugin register"* | Keep; fix the two-different-env-var-prefix bug |
| The framework owns rendering, not the handler | *"I do not want handle to call render… The framework should deal with headers and status code as well"* | One `IntoResponse` trait replaces ten hand-written overloads |
| Buffered render, clean 500 | No half-written responses | Make it a `Result`, so discarding the error cannot compile |
| Compile time over runtime | *"I tend to move things as much to the compilation phase as possible since it enforces rigourness"* | The thesis of the whole project |

**Three Crystal workarounds become compile errors rather than documented
traps.** Global registries, so two applications in one process collided and
its own tests needed a reset between cases. A `MAX_PARAMS = 4` ceiling that
silently dropped the fifth parameter. And `param()` returning `""` for an
unknown name, so a typo passed through unnoticed.

## 4. The v1 surface

The test of v1: **one developer builds a blog with accounts, posts, comments,
image upload and a welcome email, and never leaves the documentation.**

0. **Skeleton, typed config, CLI shell.** A missing or ill-typed key fails at
   startup, not at first use.
1. **Typed route table.** Declared once; the macro emits both the dispatch
   table and one type per route. A link to a route that does not exist does
   not compile. `routes` listing, URL helpers, form actions and the
   `DP-Partial` fragment mapping all derive from this.
2. **rsx responses, layouts, static files, error pages.** Build the error page
   *here*, not at the end — it pays for itself over every step after it.
3. **Schema types, migration generation, migration CLI.** A derive emits a
   schema description; `migrate make` diffs it and **writes a file the
   developer reads, edits and commits.** Never auto-apply.
4. **Query layer and pool.** Typed against the schema structs, deliberately
   less clever than Diesel, with a raw-SQL escape hatch returning typed rows.
5. **Params, validation, form builder, CSRF, flash.** The largest single piece
   and the one that separates a framework from a router: a failed validation
   re-renders the form with values and errors preserved.
6. **Sessions, cookies, argon2id, current user, policies.**
7. **Test harness.** Transaction per test, parallel by default, in-process
   requests. Built alongside 1–6, not after.
8. **The scaffold generator.** The capstone and also the specification: if
   scaffolding produces code you would not write by hand, the framework below
   it is wrong.
9. **Jobs and mailer.** Postgres-backed, `FOR UPDATE SKIP LOCKED`, **identical
   in dev, test and production** — Rails configuring Solid Queue only in
   production is one trap, and Loco shipping a 1.0 queue with no retries is
   the other. Retries and a `failed_jobs` table on day one.
10. **The dp-swap server half.** See §5.
11. **The cheap finish.** `routes`, `/up`, graceful shutdown, Dockerfile,
    seeds, pagination, rate limiting, encrypted secrets, query console.

**Cut from v1, explicitly:** WebSockets, SSE, i18n, OAuth, JWT, multi-tenancy,
soft delete, metrics, tracing, cron, pubsub, notifications, factories,
fixtures, debug toolbar, S3, and any deployment tooling beyond a Dockerfile.

## 5. What dp-swap leaves for the server

dp-swap solves navigation and deliberately omits mutating verbs, response
headers and sockets. For a backend framework that is exactly the half that
matters:

1. **Mutating verbs** — form serialisation, CSRF injection, and the
   422-with-errors re-render path.
2. **Response-header control** — retarget, reswap, redirect, refresh and a
   trigger channel, typed so a handler cannot emit an invalid combination.
3. **Out-of-band emission** — "and also replace the cart badge."
4. **`DP-Partial` as a rendering mode.** Render the layout chain, or only the
   changed depth, chosen by the header, with `Vary` set automatically. This
   falls straight out of the route table plus layout nesting and is the
   cleanest thing in the design.

**Not** a LiveView-style stateful process per connection: process memory times
concurrent users, sticky sessions, and a round trip per keystroke at 150 ms
RTT. We are not buying it.

## 6. Where Rust wins, and where it loses

**Wins worth the project.** Route generation that is total, so a bad link does
not compile — all four incumbents resolve URLs at runtime and Phoenix's `~p`,
the best of them, only warns. Forms derived from the schema type, where
Django's `ModelForm` is the best incumbent and is runtime-reflective.
**Authorization as a typed witness**: a handler takes `Verified<EditPost,
Post>`, only a policy can construct one, and forgetting the check becomes a
compile error rather than an incident. No incumbent checks authorization at
compile time.

**Losses, planned for rather than discovered.**

1. **There is no REPL and there is no fix.** `evcxr` recompiles per statement
   and cannot attach to a running process. The query console is a mitigation,
   not a solution. Say so in the documentation.
2. **Compile time is the budget and every guarantee spends it.** sqlx macros
   measured at 67.5% of total compile time. Track it as a release-blocking
   metric from the first commit and publish the number.
3. **Hot reload does not exist outside Dioxus.** `cargo-watch` is archived.
   Scope the promise to templates and assets, which is most of the benefit.
4. **Trait-bound errors will be the worst feature.** `#[debug_handler]` exists
   because Axum's bounds are unreadable, needs nightly to emit its own hint,
   and does not support generics. Define our own middleware trait and adapt
   tower behind it rather than exposing `Service`/`Layer`.
5. **Macro spans are budgeted work, not a detail.** askama's main complaint is
   errors pointing at generated code; rsx already solves this and every macro
   here must too.

**One limit the type system cannot cross:** a struct diff cannot tell a rename
from a drop-plus-add, and cannot express a backfill. Django asks, and lets you
edit the file. Do the same. The generator proposes; the developer commits.

## 7. The ORM: SeaORM 2.0.4

Decided, against Diesel, sqlx, Toasty, cornucopia, ormlite, rbatis and welds.
Three reasons, in order of weight.

**1. It is the only candidate with lifecycle hooks.** A Rails-like framework
without `before_save` is not Rails-like. On any of the others we would build a
hook system over a library with no concept of one, which means every write
must route through our wrapper and any direct call silently bypasses the
framework. That is a leak that can never be fully closed. SeaORM's hooks live
inside the write path, and in 2.0 they are natively async and take a
connection, so a hook can query. Diesel has nothing here, by deliberate design
rather than by omission.

**2. The entity registry makes generators and an admin UI tractable.** With
the `entity-registry` feature the derive emits an `inventory::submit!`, so
every model in the binary can be enumerated at runtime, and
`Schema::json_schema_from_entity` emits names, types, nullability, uniqueness
and primary key per entity. That is a scaffold generator's input and an admin
form builder, handed over. Diesel structurally cannot provide it: `table!`
emits independent modules with no registry, and `all_columns` returns a
heterogeneous tuple rather than a collection, so "walk this table's columns"
is trait gymnastics rather than a loop. Building generators on Diesel means
parsing `schema.rs` with `syn` ourselves, forever.

**3. Market evidence.** Loco 1.2.0 depends on `sea-orm ^2.0`; `cot` 0.7.0
depends on `sea-query ^1.0`, so it built its own ORM on SeaORM's query-builder
foundation. SeaQL is a team, not one person.

**Measured compile cost**, since the folklore is usually asserted rather than
checked: SeaORM costs about 6 seconds and 145 more crates on a cold build. But
incremental `cargo check` of the schema crosses over at roughly fifty tables —
at a hundred, Diesel is 4.93 s against SeaORM's 3.02 s, because Diesel emits
pairwise `TableNotEqual` impls and is quadratic in table pairs. Join depth is
the real cliff: five six-table joins took a Diesel crate from 1.05 s to
3.44 s, and compile time is not on the Diesel 2.4 roadmap.

**The honest counter-argument.** SeaORM 2.0 is three months old, shipped after
43 release candidates, and its MSRV jumped to 1.94. `schema-sync` is
self-described as "experimental and exempt from semver". The risk is schedule,
not ceiling — Diesel's missing pieces are design positions, not unshipped
features. Mitigation: pin the version, wrap `schema-sync` behind our own
interface so a breaking change costs one adapter, and keep migrations
hand-written until it stabilises.

### Four traps to design around from day one

1. **`.limit(n).find_with_related()` limits joined rows, not parents**, so
   "ten users with their posts" silently returns fewer than ten users. Rails'
   `includes` splits the query; ours must route limited eager loads through
   the two-query Loader automatically. A silent wrong answer, and not
   prominently documented.
2. **Hooks are bypassed by `update_many`, `delete_many` and `insert_many`.**
   Rails has the same split with `update_all`, so the semantics are familiar,
   but the framework must either forbid them or document it loudly.
3. **`schema-sync` only adds.** It never drops or alters, and merely logs a
   warning when a column's type differs. It is Django's `migrate` for the
   additive eighty per cent and nothing for the destructive twenty.
4. **Hooks need `#[async_trait]` on the impl.** A bare `async fn` fails with a
   lifetime error that does not explain itself.

### What we must build on top — about five to seven engineer-months

Declarative validations, since hooks are the enforcement point and not the
vocabulary — Loco pulls in `validator` for exactly this. Migration generation
from a model diff. Polymorphic associations. Factories, because **no
maintained factory crate exists for any Rust ORM**. N+1 detection in
development, which nobody has; Toasty's `Deferred<T>`, where reading an
unloaded relation panics rather than quietly querying, is the idea worth
stealing. Scopes and soft delete. The eager-load correctness fix. Dirty
tracking, which `ActiveValue` already mostly provides. Counter caches.

On Diesel the same list is ten to fourteen months, and some of it stays
permanently leaky.

## 8. Modularity: nothing ships unless it is asked for

The governing constraint, and it outranks convenience everywhere it applies.

**The core is a router handing a request and a context to a controller.** That
is the whole of it. No logger, no database, no sessions, no templating, no
TOML — the Crystal original does not even assume a log layer, and neither does
this.

Everything else is a **plugin that must be named in `main`**. A plugin that is
not imported is not compiled, not linked, and contributes nothing: no
dependency in the tree, no column in the config, no field on the context, no
bytes in the binary. "Convention over configuration" is Rails' good idea;
"the framework decided for you and you cannot see where" is its bad one, and
the line between them is whether the decision is written in your `main`.

### What this means concretely

1. **Each capability is its own crate**, and the core depends on none of them.
   The dependency arrow points from the plugin to the core, always. A plugin
   that needs another plugin depends on it directly, in the open.
2. **Feature flags gate compilation, not behaviour.** A feature that is off
   must mean the code is absent, not that a branch returns early. If turning a
   feature off leaves its dependencies in `Cargo.lock`, the split is wrong.
3. **The context is built from what was imported.** Its provider set lives in
   the application's *type*, so a handler asking for a value whose plugin was
   never registered does not compile. This is already built, and it is the
   mechanism that makes the rest safe: modularity enforced by the type system
   rather than by discipline.
4. **The ORM is recommended and absent.** SeaORM is the recommendation in §7;
   it is not a dependency of the core and never will be. The model layer is a
   plugin that registers hooks like any other, and an application that stores
   nothing pays nothing for it. A different ORM is a different plugin against
   the same extension points.
5. **Config is a plugin too.** The core takes already-parsed values. TOML, the
   environment-scoped file resolution, and the merge order are a crate an
   application opts into — a service configured entirely from its environment
   should not link a TOML parser.

### The extension points the core owns

The core cannot know what plugins exist, so it publishes seams rather than
features:

- **Context providers** — a plugin contributes typed values to the context.
- **Middleware offers** — a plugin *offers* a layer; only the application
  mounts it, at a pattern the application chooses. This was the Crystal
  author's explicit instruction and it survives: *"I do not want plugins to
  register middleware themselves."*
- **Lifecycle hooks** — `configure`, `register`, `after_config`,
  `configure_router`, `before_start`, in that order.
- **Request hooks** — before dispatch and after the response, which is what a
  logger, a metrics plugin and an ORM's connection-per-request all need.
- **Shutdown hooks** — so a connection pool can drain.

That list is the whole contract. Anything a plugin cannot do through it is a
gap in the core, to be fixed by widening the seam rather than by special-casing
the plugin.

### The test that keeps it honest

A build of the core alone, with every feature off, that asserts the dependency
tree contains no TOML parser, no SQL driver, no logging framework and no async
runtime. Modularity claimed in a README rots; modularity asserted in CI does
not.
