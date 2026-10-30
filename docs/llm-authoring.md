# Writing brocanth with an LLM

brocanth is meant to be written by a language model as much as by a person.
This document is the contract that makes that work, and it is addressed to
both: a model reading it should be able to write correct brocanth without
reading the other 44,000 lines, and a person should be able to check what the
model relied on.

Everything here is verified against this checkout. Counts were taken on
2026-10-14.

## Why a framework can be good or bad at this

A model writing code has a narrow feedback loop: it sees the source it was
given, and it sees what the toolchain prints. It does not see your running
process, your database, or the thing you meant. So the properties that make a
framework easy for a model are not a separate wish-list — they are the ones
that move failure **into that loop** and out of production:

| What a model needs | Why | What brocanth does |
|---|---|---|
| Mistakes become compile errors | The compiler is the only reviewer always in the loop | 36 proven cases; see below |
| Errors say what to do next | A model iterates on the message, not on intuition | Every CLI diagnostic carries a `help:` |
| Nothing implicit | A model cannot know a convention that is not in its context | A plugin not named in `main` contributes nothing |
| One way to do a thing | Ambiguity is where a model guesses | Routes come from files; middleware is mounted in one place |
| Self-describing state | A model cannot run a debugger | `brocanth --help` inside a project lists the project's own commands, read out of its binary |

Those five were design positions in `PLAN.md` before anyone said "LLM". The
claim here is not that brocanth was retrofitted for models; it is that a
framework that refuses to decide things behind your back is, for the same
reasons, a framework a model can write.

## The verification loop

**Run these after every change. In this order.** They are ordered by how fast
they fail and how cheap the fix is.

```
cargo test -p <crate>                # the crate you touched
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

In a scaffolded application, `brocanth serve` and then a request against it is
the end-to-end check: the route table, the middleware chain and the status
rules are only wired together once something is listening.

For the whole workspace:

```
cargo check --workspace --all-targets
cargo test --workspace
```

A full workspace check also catches a class of failure a per-crate check
cannot: a native-library conflict. Two crates wanting different
`libsqlite3-sys` versions breaks resolution for *every* crate, and
`cargo check -p <one>` will not show it.

## What cannot compile

This is the part to rely on. Each of these is a mistake that fails at build
time, and each has a test that proves it rather than a comment that claims it.

**10 `trybuild` cases** — a whole file that must not compile, with the expected
diagnostic recorded:

| Case | What rustc says |
|---|---|
| A resource with no policy | `E0046` missing `Policy` |
| A resource whose policy is not a policy | `E0277` trait bound unsatisfied |
| A resource using another entity's policy | `E0271` type mismatch |
| Opening a session without consulting the MFA policy | `E0308` expected `Admitted`, found `Verified` |
| Building an `Admitted` by hand | `E0451` fields are private |
| Spending one authentication proof twice | `E0382` use of moved value |
| Asking whether MFA is on without a proof | `E0308` expected `&Verified` |
| Raising privilege by rotating a session | `E0061` takes 1 argument |
| `#[cmd]` on an enum | names what it is, and why a struct |
| `#[cmd]` with an unknown argument | names the only argument it takes |

**And the route table refuses three more**, named here because they are the
mistakes a model makes when it treats `src/pages` like any other directory:
a handler whose name is not a verb, a page file with no handler at all, and
a second registration of a route that is already there.

**26 `compile_fail` doctests**, concentrated where a mistake would be
expensive: `brocanth-schema` (12), `brocanth-policy` (6), `brocanth-jobs` (4),
the core's context (2), the schema plugin (2).

**And three type-level guarantees without a file of their own:**

- **Asking the context for a value whose plugin was never registered does not
  compile.** The provider set lives in the application's *type*
  (`Context<E>::get::<T>()` requires `E: Has<T, I>`), so this is not a runtime
  `Option` you can forget to check.
- **Registering a hooking plugin with `.plugin(p)` instead of
  `.plugin_hooking(p)` fails the build**, naming the call to write. The plugin
  declares `const INSTALLS_HOOKS: bool = true` and `build()` checks it.
- **Two routes with the same path and verb, or two commands with the same name,
  are refused before anything is mutated** — and the command error names *every*
  collision, not the first.

### Write code that leans on this

When there is a choice between a check you could forget and a type you cannot
construct, take the type. The framework already does:

```rust
// Not this -- `is_allowed` is a bool you can ignore.
if policy.is_allowed(&user, &post) { update(post); }

// This. `Verified` has no public constructor, is not Clone, and `update`
// takes it by value, so the check cannot be skipped, reused, or dropped.
let verified = admission.resolve(rows)?;
let write = verified.update(dialect, changes)?;
```

## What can still go wrong

The honest other half. **These need a human or a test, because the compiler
will not catch them**, and a model should flag them rather than assume them.

1. **A policy that compiles but permits everything.** `Pred::Always` is a legal
   policy. `pred.is_always()` exists so a test can assert against it — write
   that test.
2. **An absent value given the permissive reading.** `Bound::or_allow(None)`
   really is "allow", and it is spelled out in the diff on purpose. It compiles
   and it is sometimes right; read it every time.
3. **A command in a third-party crate that the linker drops.** Nothing references
   the crate, so nothing keeps it, and the command is silently absent from
   `--help`. The fix is `use their_crate as _;` and it is the one failure in
   the command system that is silent.
4. **Values in a cache or a blob store are bytes.** No schema, no versioning.
   Put the format version in the key.
5. **Adding a file under `src/pages` does not rebuild on its own.** Nothing
   referenced it yet, so cargo cannot see it — a new page does not route and
   a new `layout.rsx` does not wrap. `touch src/pages/mod.rs`. Editing or
   deleting an existing one is noticed, because every file the macros used is
   `include_bytes!`d by the expansion. A file added under `assets/` or
   `public/` is the same case, and the fix is `touch src/assets.rs`.
6. **Test suites that need a database skip themselves.** They say so loudly on
   stderr, but a green run is not a run — check whether
   `BROCANTH_TEST_DATABASE_URL` was set before believing a Postgres suite
   passed.

## The shape of the framework

Only what a model needs to place code correctly.

**The core is a router handing a request and a context to a handler.** It has
exactly three dependencies and a test that fails the build if a fourth kind
arrives. It has no logger, no database, no sessions, no templating and no TOML.

**Everything else is a plugin that must be named in `main`.** A plugin that is
not named is not compiled, not linked, and contributes nothing: no dependency
in the tree, no field on the context, no bytes in the binary. So when asked
"where does X come from", the answer is always a line in `main` — and if it is
not there, X is not in the binary.

**A plugin offers middleware; the application mounts it.** A plugin never
mounts itself, and the mount is a line in the application's own `src/app.rs`.

**A route is a file, and nothing else says so.** `src/pages/about.rs` is
`/about`; `src/pages/users/[id].rs` is `/users/{id}`. Adding a route is
writing the file: `brocanth_pages::pages!("src/pages")` in `src/pages/mod.rs`
reads the directory while the crate compiles and emits the `mod` lines and
`router()`. There is no list, so a table that disagrees with the filesystem is
not a state the project can be in. **Do not add a `mod` line or a
`routes.file(..)` row** — the first is a duplicate module, the second a
duplicate route, and both are build errors.

The verb comes off the handler's name — `handle` is GET, then `handle_head`,
`handle_post`, `handle_put`, `handle_patch`, `handle_delete`,
`handle_options`. The page's source is parsed to find them. A `pub fn
handle_*` outside that list is a build error naming the list; a page with no
handler is a build error; a private `fn handle_*` is a helper.

**A command is a struct.** `#[cmd]` on a `clap::Args` struct makes it a
subcommand of the project's own binary, registered at link time. There is
no list to add it to.

**A view is a `.rsx` file beside the page it renders**, when the project asked
for one. `--with rsx` writes `src/pages/index.rsx` next to `src/pages/index.rs`
and `rsx::load_dir!("src/pages")` compiles the tree into the binary, so a
template needs no registration: the directory is the registry. A component is a
function returning `Element`; its parameters are struct fields at the call
site, so a wrong one is a build failure naming the right ones. One macro
reaches them from a handler: `html!(ctx, views::index::Index { title })`,
which returns the handler's own `Result`. It takes the context because the
request can ask for part of a document; see `rsx-htmx` below.

**A page never names its layout, and neither does anything else.**
`layout.rsx` wraps every page in its directory and below, as `layout.cr` does
in Crystal's page_router. `html!` reads that off the directory tree while the
crate compiles: it knows which file it was written in, walks up to `src/`, and
wraps the page in every `layout.rsx` it passes. Layouts stack root to leaf,
there is no chain to keep in step, and deleting a `layout.rsx` removes exactly
that layer. A layout takes `children: Element`, and may also take
`layout: &str`, which `html!` fills with the URL prefix that layout covers.
Those two are the only parameters it fills. `render` is the same thing with no
layout, for a fragment.

Without `--with rsx` there is no `html!`, no `views` module and no template
engine in the tree — check `Cargo.toml` before writing markup.

**Two choices sit inside rsx, and are named for it**: `rsx-tailwind` and
`rsx-htmx`. Either one brings `src/assets.rs`, where
`brocanth_assets::assets!("assets", "public")` embeds every static file —
`assets/` fingerprinted and cached for a year, `public/` verbatim with an
ETag. Reference one with `assets::url("css/site.css")`, never by hand, and
never add a `.css` to a page by writing a path. A basename starting with `_`
is an input and is not published.

**`rsx-htmx` makes a layout skippable.** `<Link to={routes::…::path(7)}>`
renders a plain `<a href dp-boost>`, so `to` takes a route helper and a dead
link does not compile. A layout marks what a swap may replace with
`dp-layout={layout}` on the element wrapping exactly the children — nothing
else, or that element is replaced too. A request carrying `DP-Partial: n`
drops the outermost `n` layouts, which `html!` resolves as a `match` over the
chain it already knows. A handler never reads the header.

### The crates, and what each is for

| Crate | Reach for it when |
|---|---|
| `brocanth` | the core: routing, context, plugins, hooks |
| `brocanth-config` | TOML/JSON/YAML/env config, with provenance per value |
| `brocanth-policy` | a typed predicate: row-level authorization, as SQL or in memory |
| `brocanth-resource` | typed CRUD routes derived from an entity and a policy |
| `brocanth-events` | publishing row changes; the model↔realtime seam |
| `brocanth-realtime` | broadcasting changes to subscribers by audience |
| `brocanth-auth` | OAuth2, passwords, sessions, MFA, impersonation |
| `brocanth-storage` | blobs, content-addressed, local disk or Postgres |
| `brocanth-cache` | a cache in Postgres on an unlogged table |
| `brocanth-jobs` | a persistent queue and cron |
| `brocanth-model` | the model layer: SeaORM, validations, migrations |
| `brocanth-command` | the `#[cmd]` attribute and the command runner |
| `brocanth-admin` | a Django-shaped backoffice over the schema |
| `brocanth-schema` | zod-shaped validation, feeding OpenAPI |
| `brocanth-server` | the HTTP server, and its bridge to the core |
| `brocanth-pages` | `pages!` and `html!`: the route table and the layouts, read off `src/pages` |
| `brocanth-assets` | `assets!`: static files embedded, fingerprinted or verbatim |
| `brocanth-i18n` | translations: catalogs checked at build time, locale negotiation, CLDR plurals, numbers and dates |

`rsx` is the one dependency in this list that is not a brocanth crate. It has
its own repository, so `--brocanth-path` does not reach it and the generated
manifest fetches it by git.

**Postgres is the base.** SQLite is supported for smaller single-machine
setups and is a cargo feature, not a different crate.

## Conventions that are not in the code

A model cannot infer these from the source, so they are written here.

- **Never `cargo fmt` the whole workspace.** It is not fmt-clean, and doing so
  buries a real change in hundreds of unrelated whitespace lines. Format only
  the files you wrote.
- **A comment explains *why*, next to the decision.** This codebase does not
  write `// increment i`. It writes the reason a choice was made and what was
  chosen against, because that is the thing a reader cannot recover.
- **A test name is a sentence describing the guarantee.**
  `a_stalled_subscriber_does_not_stall_the_broadcaster`, not `test_broadcast`.
- **A limit gets written down rather than discovered.** If something cannot be
  done, say so in the docs where someone would look for it, with what it would
  take.
- **Assert on work done, not on wall-clock time.** A test that measures
  elapsed milliseconds flakes on a loaded machine. Count reads, count
  allocations, use a barrier so the test deadlocks instead of flaking.

## When you are stuck

Read, in this order:

1. `PLAN.md` — the argument the project is built on. §8 is modularity and
   governs everything; §9 is authorization; §10 is realtime.
2. The crate's own `src/lib.rs` doc comment. Every crate opens with what it is
   for and what it refuses to do.
3. `docs/getting-started.md` for the CLI, `docs/async-handlers.md` for how a
   route awaits, `docs/pocketbase-surface.md` for what brocanth is measured
   against and what it declines.

If a seam you need does not exist, **that is a gap in the core to be widened,
not a plugin to special-case** — `PLAN.md` §8 says so explicitly. Say that
rather than working around it.
