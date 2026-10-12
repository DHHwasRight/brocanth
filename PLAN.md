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

## 7. Open

The ORM. The instruction is a full-fledged ORM rather than a query builder,
which abandons the Crystal plan of building models on `sq`. The constraint on
record from the earlier analysis: *"Don't rebuild sq as an ORM-that-hydrates-
everything. You have the chance to make the model layer stream rows into
structs without SeaORM's intermediate `Vec<(String, Value)>` step."* The
survey is still running; nothing in §4 steps 3–4 should be written until it
lands, because the model layer gates jobs, auth, generators and scaffolding.
