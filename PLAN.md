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

**Cut from v1, explicitly:** WebSockets, SSE, i18n, multi-tenancy, soft
delete, metrics, tracing, pubsub, notifications, factories, fixtures, debug
toolbar, S3, and any deployment tooling beyond a Dockerfile.

**Three things on that list shipped anyway, and the list is wrong rather than
the work.** OAuth and JWT arrived with `brocanth-auth`, because a framework
whose first real application needs a login cannot cut the login. Cron arrived
with `brocanth-jobs`, where a recurring schedule is a few hundred lines beside
a queue that already exists and a separate crate later would duplicate the
worker. Each carries its dependency in the plugin rather than the core, which
is the test that matters.

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

Status: all four are built. See G2 in `docs/framework-gaps.md`: the
client is `brocanth_middleware::dp::SCRIPT`, the headers `dp::Directive`,
the fragments `dp::Oob`, the 422 `brocanth_input::rerender`.

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
- **Application hooks** — `configure`, `register`, `after_config`,
  `configure_router`, `before_start`, then `start` once the socket is open,
  then `begin_shutdown` and `shutdown`. Two shutdown phases, because a
  connection pool and a job runner need different things from the word "stop";
  ADR 2.
- **Router contributions** — a plugin describes a *bundle* of routes relative
  to a prefix it does not choose, and the application mounts the bundle in one
  line at a prefix it does. Twelve admin screens, one decision, still the
  application's; ADR 3.
- **Request hooks** — before dispatch and after the response, which is what a
  logger, a metrics plugin and an ORM's connection-per-request all need.
- **Response hooks** — rewriting headers and body, in reverse declared order,
  which is what compression, partial-navigation headers and ETag need.
- **Model hooks** — the ORM declares an event *kind* and what it carries; the
  core forwards a value it never looks inside, so the seam exists without the
  core knowing an ORM does.
- **Error hooks** — a plugin recognises its own error type and renders it, so a
  validation failure re-renders the form and an auth failure redirects, with
  the core knowing neither concept.

Order between two plugins at one point is **declared, not accidental**: a hook
says `after("session")` or `before("compression")`, registration order is only
the tie-break, a missing required dependency is a build error with a
suggestion, and a cycle is a build error naming both ends.

A hook nobody registered costs nothing — no allocation, no dynamic dispatch, no
branch on an empty list — because the set of installed hook families lives in
the application's *type*, exactly as the provider set does. The price is one
explicit builder call per hooking plugin, and ADR 1 is why that price is the
right one.

That list is the whole contract. Anything a plugin cannot do through it is a
gap in the core, to be fixed by widening the seam rather than by special-casing
the plugin.

### The test that keeps it honest

A build of the core alone, with every feature off, that asserts the dependency
tree contains no TOML parser, no SQL driver, no logging framework and no async
runtime. Modularity claimed in a README rots; modularity asserted in CI does
not.

## 9. Authorization: a predicate and a witness, not one or the other

From a study of PocketBase, whose API rules are the feature people love it for.

**The universal description of those rules is wrong, and the error matters.**
They are not "an expression language evaluated per request". There is no
evaluator: `tools/search/filter.go` **compiles the string to SQL** and the
database evaluates it. That one fact is why a rule can occupy four positions
at once — a filter `AND`ed into the list query, a guard `AND`ed into the
by-id select, an admission test against a synthesised one-row CTE, and a
visibility re-check per realtime subscriber.

### Why §6's typed witness is not enough on its own

A witness is a proof about **one value you already hold**. To produce
`Vec<Verified<P, Post>>` you must load `Vec<Post>` and filter — and then
pagination is broken: you asked for thirty, got thirty, filtered to eleven,
page two overlaps page one and the total is a lie. The only honest repairs are
unbounded fetch-until-full, or pushing the predicate into the `WHERE` clause.

**List authorization is not a per-object property. It is a set
comprehension.** Any design that ignores this ships a paginator that silently
lies.

### The resolution

They are not alternatives. **A rule is a predicate; a witness is a proof that
a predicate was evaluated.** PocketBase ships only the first, §6 shipped only
the second. `brocanth-policy` ships both: a typed predicate over the entity's
column enum, with two interpretations — `condition()` into SQL for lists, and
`eval()` for a row you hold.

Three obligations fall out, each recorded because each is easy to get wrong:

1. **The two interpretations can disagree**, and PocketBase cannot have this
   bug because it has only one. Default to `condition()` only, implemented as
   a one-row select — literally what PocketBase's `CanAccessRecord` does.
   `eval()` is opt-in, and opting in generates a proptest asserting
   `eval(row) == (row ∈ query(condition))`. The obligation moves from review
   to CI.
2. **`Verified` must carry its condition**, not merely mark a type. Check-then-
   write has a TOCTOU window; the write must be `UPDATE … WHERE id = ? AND
   <condition>`, and zero rows affected is an authorization failure rather
   than a no-op.
3. **`Verified` must be consumed, not produced.** `#[must_use]` is not enough —
   you can construct one and drop it. The write API takes it.

### The vulnerability class this deletes

PocketBase validates a rule with an **empty request object**, and an
unresolvable `@request.*` path logs a debug line and becomes `NULL`. Its `!=`
compiles to `IS NOT` over `COALESCE`. So:

```
@request.auth.rolee != "banned"              ← a typo
→ COALESCE(NULL,'') IS NOT COALESCE('banned','')
→ '' IS NOT 'banned'
→ TRUE                                        ← access granted
```

**A misspelled field in a negated rule grants access, silently, forever.** It
is structural, not a bug. A typed predicate over `Option<T>` forces the author
to say what absence means, which is a decision PocketBase makes for them in
the dangerous direction.

### Steal `:changed`

`@request.body.status:changed = false || @request.auth.role = "admin"` is
*field-level* write authorization in the same language as row-level. Pundit
keeps `permitted_attributes` as a separate concept; Django uses form fields.
Here it becomes `Policy::writable() -> FieldMask`, checked against the dirty
tracking the model layer already has. Nearly free, and almost nobody has it.

### The honest scoreboard

PocketBase's rules attach to the **data**, so they are unforgettable on the
generated CRUD path and **entirely absent everywhere else** — the moment a hook
calls `app.Save()` you are outside the rule system, and its own documentation
says so. The most-praised feature stops applying exactly when an application
stops being CRUD.

A witness attaches to the **function signature**, so it is unforgettable
everywhere and must be written everywhere. Attaching the policy to the entity
gives us the first; expressing the result as a witness gives us the second.
That is a tie where PocketBase is strongest and a win where it is weakest.

What we lose, and cannot get back: **rules editable at runtime.** PocketBase
lets you fix a production authorization bug in a browser in ten seconds. We
never will.

## 10. Realtime, and the reason the typed predicate pays for itself

PocketBase broadcasts to **all** connected clients in chunks — there is no
topic index — and for each client runs `SELECT (1) FROM t WHERE id = ? AND
<rule>`. At a thousand connections with six hundred subscribers, one write is
six hundred point queries: 1.2–6 ms on in-process SQLite, and **about 180 ms
on Postgres**, which does not work at all.

The fix is the strongest practical argument for the whole typed design:

> **A predicate you can inspect can be inverted into an audience. A string you
> can only execute.**

Most policies partition an audience coarsely — public, any authenticated user,
the owner, members of a team. Because a policy is a *value*, its shape can be
matched to derive an audience key, and broadcast becomes a map lookup with
zero queries. Only an opaque policy falls back to per-subscriber evaluation.
Two to three orders of magnitude, and it is what makes realtime on Postgres
possible rather than merely slow.

Three further corrections to the cost model: index by topic rather than by
client (PocketBase's own source carries a `@todo` about this); bound the
per-client queue with an explicit overflow policy, because an unbuffered
channel lets one slow reader block the broadcaster; and cap the client count,
which PocketBase does not do at all.

**Ownership.** The core stays at three dependencies and no async runtime, and
does not need widening: `Layer<C, O>` is generic over the output, so the core
has no opinion about what a response is and therefore none about streaming.
A tiny `brocanth-events` crate carries `Change` and `ChangeSink`; the model
layer publishes, realtime subscribes, and neither depends on the other. The
same trait over `LISTEN/NOTIFY` buys multi-process realtime — which PocketBase
cannot have at any price, its broker being in-memory beside an embedded
database.

**The payload should be a fragment, not JSON.** PocketBase pushes records
because it has no renderer. We have one, and §5.4 already makes `DP-Partial` a
rendering mode. The native payload here is an HTML fragment addressed at a
dp-swap target, arriving out of band over SSE — and the fragment emitter caches
one render per audience class, so the two ideas pay for each other.

## 11. Renames: the limit §6 called uncrossable is crossable

§6 records that "a struct diff cannot tell a rename from a drop-plus-add".
PocketBase can, and the mechanism is worth taking.

**Its fields carry a persisted `id`, and the collection diff is by id rather
than by name.** The id is seeded from the name but stored, and the editor
round-trips it, so a rename arrives as "field `text1234567` changed its name"
rather than as a drop and an add.

Our editor is a text editor, so the identity has to live in the text:
`#[brocanth(id = "01J…")]`, written once by the scaffolder and never edited by
hand. Then `migrate make` diffs by id and can emit a rename. Same idea,
adapted to the fact that a human is doing the editing.

## 12. What we decline from PocketBase, and the price of §8

**Declined outright.** Dynamic collections — runtime schema and compile-time
types cannot both be the source of truth, and this is a refusal rather than a
deferral. The JavaScript hook VM — a framework whose thesis is compile-time
checking does not ship an interpreter. Database-stored settings. Backups,
where `pg_dump` plus an object store is the operator's job and a worse wrapper
is negative value.

**The headline demo is impossible and the README must not pretend otherwise.**
"Open a browser, create a collection, store data, thirty seconds" cannot be
matched when the schema is Rust types. The best version is ten to sixty
seconds and a rebuild.

**The price of modularity, stated rather than discovered.** A PocketBase-shaped
surface is about ten plugins with roughly fifteen real contact points between
them — realtime×model, admin×policy, openapi×schema, auth×model, and so on.
Each is an interface somebody designs, documents and versions. At a week
apiece that is **a quarter of the total engineering spent purely on seams
PocketBase does not pay for.** It is worth paying. It should be budgeted.

**Estimated size of the PocketBase-shaped surface: 38–42 engineer-weeks**, on
top of the five to seven months §7 already budgets for the model layer.

### Build order

1. The core's missing request and shutdown hooks — §8 promises both and the
   `Plugin` trait has neither.
2. `brocanth-policy` tier one: the predicate algebra, `condition()`,
   `Verified` carrying its condition, `FieldMask`.
3. `brocanth-events`: `Change` and `ChangeSink`, about a hundred lines, which
   unblocks realtime, audit and cache invalidation at once.
4. `brocanth-storage`, independent of the above and parallelisable.
5. `brocanth-resource`: typed CRUD routes from an entity and a policy.
6. `brocanth-realtime`.
7. Auth completion: OTP, MFA as a factor set, impersonation, login alerts.
8. Stable field identity and a rename-aware `migrate make` (§11).
9. The admin against row-level policies — **the item that always overruns**.
10. The `policy!` macro, deliberately late, after a real application has used
    the algebra, so the syntax is informed rather than imitated.

**And one more assertion in the same spirit as the dependency test: a build of
`brocanth-resource` with no policy registered must not compile.**
Authorization claimed in a README rots. Authorization asserted by the type
checker does not.

## 13. Decision records

Numbered, dated by the commit that introduced them, and written at the moment
the decision was contested rather than afterwards. An ADR is here because the
obvious answer was wrong and the reason is not obvious.

### ADR 1 — Hooks are registered explicitly. No distributed slice.

**Status:** accepted. **Supersedes:** nothing. **Touches:** §8, all of it.

**The question.** The core publishes seams; plugins fill them. Rust's standard
mechanism for compile-time registration is the distributed slice — `inventory`
(1.0, `ctor`-based) or `linkme` (link-section based). Both let a plugin
*anywhere in the dependency graph* submit an entry that the binary collects,
with nothing written in `main`. That is the ergonomic answer, and §8 says the
opposite: **a plugin must be named in `main`**, because *"what your binary
contains is written where you can read it."*

**The argument that the two are compatible, and why it is not enough.** A
distributed slice only collects from crates that are actually linked, and a
crate is only linked because something imported it, so — the argument runs —
the submissions in your binary are exactly the ones your imports asked for, and
`main` is still the manifest.

It is not enough, for three reasons.

1. **The import need not be yours.** Linking follows the *transitive* graph. A
   plugin three levels down that depends on `brocanth-metrics` links it, and
   `brocanth-metrics`' submissions land in your binary. Your `main` names a
   blog engine; your request pipeline grows a hook you never asked for and
   cannot see. That is precisely the Rails failure §8 names — *"the framework
   decided for you and you cannot see where"* — reached through the back door.
2. **The registry is invisible to the compiler.** A distributed slice is a
   run-time array. §6's entire premise is route generation that is *total*: a
   link to a route that does not exist must not compile. Routes collected into
   a slice can only be validated at startup, which turns the one guarantee this
   project exists for back into a boot-time panic. The same applies to the
   context provider list, which is already a type-level list precisely so a
   handler asking for an uninstalled value does not build.
3. **It is fragile exactly where we ship.** The failure modes are documented
   and real: `linkme` needs `-C link-dead-code` or `#[used(linker)]` to survive
   section garbage collection on some linkers; neither crate collects across a
   `dylib`/`cdylib`/`staticlib` boundary, because the section belongs to the
   linkage unit; `ctor` ordering is unspecified relative to other static
   initialisers; and under incremental compilation a crate that only *submits*
   does not always force the consumer's collection site to be regenerated, so a
   stale slice survives a rebuild. Each of those is a bug that reproduces on
   one machine and not the next, in a framework whose selling point is that the
   compiler checks the wiring.

**The decision.** Explicit registration, everywhere, with the type system
carrying what the compiler needs to know:

* `AppBuilder<E, H>` carries two type-level records — `E`, the installed
  context providers, and `H`, the installed hook families. Both widen only
  through a call the application wrote.
* A plugin that hooks is registered with `.plugin_hooking(p)` rather than
  `.plugin(p)`. That call, and nothing else, is what puts a hook family in `H`.
* A plugin that contributes routes is mounted with
  `app.mount_routes::<P>(prefix)`. Nothing mounts itself.

**What it costs, stated plainly.**

1. **One extra line per hooking plugin, and a wrong call that is silent in
   principle.** `.plugin(p)` on a hooking plugin would install nothing. Rust
   has no negative bounds and no way to ask "does `P` also implement
   `HookSource`", so the plugin declares `const INSTALLS_HOOKS: bool = true`
   and `build()` fails with the call to write instead. A const, not a type,
   because associated type *defaults* are still unstable (`#29661`) and adding
   a required associated type to `Plugin` would break every plugin already
   written against it.
2. **Two builder methods where one would read better.** `plugin` and
   `plugin_hooking`, plus `_as`/`_named` variants of each. The alternative —
   one method that always widens `H` — would make every application pay the
   branch, which is the thing the record exists to remove.
3. **The ORM must enumerate its own models.** §7.2 counts SeaORM's
   `entity-registry` — which *is* an `inventory::submit!` — as a reason to
   choose it. That stays true and is not a contradiction: the submission
   happens inside the ORM plugin's own crate, collected by the ORM plugin's own
   code, for a list the application already opted into by naming the ORM plugin
   in `main`. The rule is about the *core's* seams. A plugin may use whatever
   it likes inside itself; what it may not do is reach into the application's
   pipeline without the application's call.
4. **A plugin generic over the provider list cannot require a context item at
   compile time.** `E: Has<T, I>` needs the position index `I`, which is
   unconstrained in an impl that does not mention it (E0207). A plugin written
   against one application's concrete list can use `Context::get`; a plugin
   generic over `E` uses `Context::try_get`, or declares the dependency at
   build time with `Lifecycle::require_service` / `require_instance`, which
   fails the build naming the plugin to add. Build-time rather than
   compile-time, and the message says what to do.

**The fallback, if this is ever wrong.** If explicit registration proves
intolerable at scale, the escape is a *generated* registration list — a build
script or a macro that reads `Cargo.toml` and emits the `.plugin(..)` calls
into a file the developer can open and read — not a slice the compiler cannot
see. The property to preserve is readability of the manifest, not the manual
typing of it.

### ADR 2 — Shutdown is two phases, in reverse registration order.

**Status:** accepted.

One phase cannot serve both a connection pool and a job runner. Told "stop"
once, a runner that blocks until its queue drains blocks while the queue is
still being filled, and a pool that closes at once takes the connection the
in-flight job is holding. So `begin_shutdown` closes the gate and returns, and
`shutdown` drains inside a grace period shared by every plugin and readable as
`ShutdownCx::remaining`.

Reverse registration order, because registration order *is* dependency order:
the ORM is registered after the pool because it needs the pool, so it must be
finished with the pool before the pool closes. And one failure does not cancel
the rest — a shutdown that stops at the first error leaks exactly the resources
it was called to release. `App::shutdown` returns a `ShutdownReport`, never an
`Err`.

### ADR 3 — The unit of a route decision is the bundle, not the route.

**Status:** accepted. **Tension with:** §3, *"Plugins never autowire."*

The middleware rule — *"I do not want plugins to register middleware
themselves"* — is right and does not transfer unchanged to routes. An admin
plugin has twelve screens. Making the application write twelve `mount` lines
turns a principle into a chore, a chore gets a macro wrapped round it, and then
the principle is gone.

So a plugin describes its routes *relative to a prefix it does not choose*, and
the application takes the whole bundle in one line at a prefix it does:
`app.mount_routes::<AdminPlugin>("/backoffice")?`. Three properties keep this
on the right side of the rule: nothing mounts itself and
`App::unmounted_route_sources()` lists what was offered and not taken; the
prefix is always the application's, and `RouteSource::PREFIX` is only a
suggestion; and a subset is still one line, through `mount_routes_where`, with
no cooperation needed from the plugin. Two plugins claiming one path and verb
is an error naming both — never last-one-wins, which is registration order
deciding behaviour, which is the thing all of this is against.
