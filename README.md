<h1 align="center">brocanth</h1>

<p align="center">A Rust server framework for HTML-first apps and JSON APIs. Postgres first, day-2 ready.</p>

---

> **Status.** Most of this code is vibe coded: written with an LLM, as a
> thought exercise. We use it for internal products where mission criticality
> is not needed. Nothing is on crates.io. Read it, take ideas from it, but do
> not bet a business on it.

## What brocanth is

A Cargo workspace of 44 crates under `crates/` and two examples under
`examples/` (`demo-app`, `nested-cards`). The core is a router that hands a
request and a context to a handler. Everything else is a plugin that you
name in `main`. The `brocanth` CLI writes a project from the modules you
choose.

The org name comes from RailsConf 2026. brocanth takes four positions that
DHH argues for a web stack, in Rust, to see how far they get:

- **HTML over the wire.** The server renders. The client swaps fragments
  (dp-swap, an htmx-shaped script).
- **No build step** for the front end. The one exception is opt-in:
  `rsx-tailwind` needs node.
- **One database.** Postgres holds the data, the jobs, the cache, the files
  and the realtime fan-out.
- **One framework for the whole stack**: routes, models, migrations,
  policies, jobs, auth, an admin.

We add one goal of our own. brocanth wants to be a Rust equivalent of
[d2s](https://github.com/IxDay/d2s), the "Day 2 Stack": ready for production,
devops and SRE from day one, and still simple. The
[day-2 checklist](#day-2-checklist) says how far that goal is from done.

brocanth is not only for HTML. The same router serves a JSON API with typed
input, OpenAPI 3.1, row-level policies and typed CRUD routes, and no
templates at all.

Two rules shape the code:

- **Nothing ships unless you ask for it.** A module that you do not pick is
  not in the source, the binary or `Cargo.lock` (PLAN.md §8).
- **Mistakes are compile errors.** A link to a missing route, a handler that
  reads an unregistered plugin, or a resource with no policy does not
  compile. This rule also makes the code easy for a model to write. See
  [`docs/llm-authoring.md`](docs/llm-authoring.md).

## Getting started in five minutes

You need Rust 1.97 or later (`mise.toml` pins 1.98). The `model`, `jobs`
and `cache` modules also need a Postgres server in production; `model`
starts on a SQLite file in development.

**1. Install the CLI** from a checkout. Nothing is on crates.io yet.

```sh
git clone https://github.com/DHHwasRight/brocanth
cd brocanth
cargo install --path crates/brocanth-cli
```

**2. Create a project with the wizard.** Run `create` with no name in a
terminal. `--brocanth-path` points the generated `Cargo.toml` at your
checkout. It takes the core crate's directory, not the checkout root.

```sh
brocanth create --brocanth-path /path/to/brocanth/crates/brocanth
```

The chooser has two tabs over one answer. **Wizard** asks one question per
screen. **Expert** shows every module at once, and is also the review
screen. `tab` switches them.

The first question is the shape. Each shape ticks a set of modules, which
you then edit, and skips the questions that do not apply:

| Shape | Ticks | Skips |
|---|---|---|
| An HTML application | `observability` (logs, health, metrics), `docker`, `ci-github`, `config`, `commands`, `model`, `sessions`, `rsx`, `rsx-tailwind`, `rsx-htmx` | |
| A JSON API | `observability` (logs, health, metrics), `docker`, `ci-github`, `config`, `commands`, `model`, `validation`, `openapi` | Pages |
| Something that does work | `observability` (logs, health, metrics), `docker`, `ci-github`, `config`, `commands`, `model`, `jobs`, `cache` | Pages |
| A static site | `static`, `ci-github`, `config`, `commands`, `rsx`, `rsx-tailwind`, `rsx-htmx`, `robots` | Data, Behaviour, Doors |
| Nothing yet | nothing | |

The questions after the shape: Foundations, Data, Pages, Behaviour, Doors,
Operations. The list of shapes is `PRESETS` in
`crates/brocanth-cli/src/wizard.rs`.

**The static-site shape** keeps the page router and drops the server.
`static` refuses every module that needs a running process, so the chooser
offers only templates, CSS, config, commands and CI with it. The project
gets an `export` command that requests every GET route once and writes the
answers and the assets to `dist/`. `--base /my-site` prefixes every
root-relative URL, for a GitHub Pages project site. See
[`docs/static-export.md`](docs/static-export.md).

**3. Or create it without the wizard.** With a name, `create` runs straight
through. `--with` names the modules. Without `--with`, you get
`observability` with logs, health and metrics, `config`, `commands`,
`docker`, `ci` with `ci-github`, `middleware` and `middleware-security-headers`.
`--core-only` gives a router and nothing else.

```sh
brocanth create blog \
  --with config,commands,model,sessions,rsx,rsx-htmx \
  --brocanth-path /path/to/brocanth/crates/brocanth

brocanth create site \
  --with static,config,commands,rsx,rsx-tailwind,rsx-htmx,ci,ci-github \
  --brocanth-path /path/to/brocanth/crates/brocanth
```

A flag never adds a module for you. When a module needs another one,
`--with` refuses and names the fix.

**4. Run it.**

```sh
cd blog
cargo run -- serve        # listens on 127.0.0.1:3000
cargo test
```

With the `commands` module the binary has subcommands, so `serve` is one of
them; `brocanth serve` does the same from the project directory. Without
`commands`, `cargo run` serves. A static site builds with
`cargo run -- export`.

**5. Grow it**, and check it before a deploy:

```sh
brocanth add jobs                 # the same generator, one more module
brocanth migrate make add_posts   # with `model`: the project's own command diffs src/models
brocanth migrate up               # and applies, in the project's binary
brocanth check --deploy           # secrets, cookies, CSRF, the database
brocanth dbshell                  # with `db`: the project's command, psql or sqlite3
brocanth admin eject list         # copy a shipped admin screen into the project, as yours
brocanth admin diff               # after an upgrade: each copy against the shipped screen
brocanth --help                   # also lists the project's own commands
```

Every project gets an `AGENTS.md`. It tells a model what the project
contains and what it does not. [`docs/getting-started.md`](docs/getting-started.md)
walks through all of this with real output.

## Crate map

Each crate has a README. The **Module** column gives the `--with` key that
puts the crate in a new project.

| Crate | What it does | Module |
|---|---|---|
| [`brocanth`](crates/brocanth/README.md) | The core: a router that hands a request and a context to a handler, plugin seams, two-phase shutdown | always |
| [`brocanth-server`](crates/brocanth-server/README.md) | The HTTP entrypoint, on Tokio | always |
| [`brocanth-pages`](crates/brocanth-pages/README.md) | `src/pages` read at compile time: the route table and the layouts | always |
| [`brocanth-routes`](crates/brocanth-routes/README.md) | `routes!`: the whole route table in one file, an alternative to `src/pages` | |
| [`brocanth-assets`](crates/brocanth-assets/README.md) | Static files embedded at compile time: fingerprinted assets and a verbatim public directory | with `rsx-tailwind` or `rsx-htmx` |
| [`brocanth-bundle`](crates/brocanth-bundle/README.md) | JS and CSS bundles from `build.rs` (rolldown, lightningcss) | |
| [`brocanth-config`](crates/brocanth-config/README.md) | Layered config (TOML, JSON, YAML, environment, flags) with provenance and secret redaction | `config` |
| [`brocanth-middleware`](crates/brocanth-middleware/README.md) | Proxy headers, method override, canonical host, content types, CORS, CSRF, security headers, rate limiting | `middleware` and its children |
| [`brocanth-gtag`](crates/brocanth-gtag/README.md) | Google Analytics 4 behind a consent banner: nothing sent before consent, privacy signals honoured, URLs scrubbed | `gtag` |
| [`brocanth-input`](crates/brocanth-input/README.md) | A form or JSON body read into a struct, with every fault at once | `input` |
| [`brocanth-schema`](crates/brocanth-schema/README.md) | Schemas checked by the compiler and at run time; a handler takes `Valid<T>` | `validation` |
| [`brocanth-openapi`](crates/brocanth-openapi/README.md) | An OpenAPI 3.1 document from the route table, with a drift check | `openapi` |
| [`brocanth-robots`](crates/brocanth-robots/README.md) | A robots.txt and a sitemap.xml from builders, checked before they are served | `robots`, `sitemap` |
| [`brocanth-meta`](crates/brocanth-meta/README.md) | A title, a description, a canonical URL, share tags and hreflang for each page | `meta` |
| [`brocanth-errors`](crates/brocanth-errors/README.md) | HTML 404 and 500 pages, a development page with the cause, and the reporter hook | `errors` |
| [`brocanth-client`](crates/brocanth-client/README.md) | A command line that calls the application's own API, built from its routes | `client` |
| [`brocanth-command`](crates/brocanth-command/README.md) | Project commands: define one with `#[cmd]` and the CLI finds it | `commands` |
| [`brocanth-command-macros`](crates/brocanth-command-macros/README.md) | The `#[cmd]` attribute, re-exported by `brocanth-command` | with `commands` |
| [`brocanth-i18n`](crates/brocanth-i18n/README.md) | Message catalogs checked at build time, locale negotiation, CLDR plurals | |
| [`brocanth-model`](crates/brocanth-model/README.md) | The model layer over SeaORM: validations, eager loading, N+1 detection, dirty tracking | `model` |
| [`brocanth-migrate`](crates/brocanth-migrate/README.md) | Stable field ids, a schema snapshot and a rename-aware migration diff | the `migrate` tool, with `model` |
| [`brocanth-events`](crates/brocanth-events/README.md) | `Change` and `ChangeSink`: the seam between writes and subscribers | with `model` |
| [`brocanth-resource`](crates/brocanth-resource/README.md) | Typed CRUD routes over one table, with the policy in the `WHERE` | `resource` |
| [`brocanth-cache`](crates/brocanth-cache/README.md) | A cache on unlogged Postgres tables, with tags | `cache` |
| [`brocanth-storage`](crates/brocanth-storage/README.md) | File storage: one trait, local disk, content-addressed names, image variants | `storage` |
| [`brocanth-auth`](crates/brocanth-auth/README.md) | argon2id accounts, server-side sessions, OAuth 2.0 and OIDC with PKCE | `auth` |
| [`brocanth-policy`](crates/brocanth-policy/README.md) | Typed predicates: SQL for a list, evaluation for one row | `policy` |
| [`brocanth-policy-macros`](crates/brocanth-policy-macros/README.md) | The `policy!` macro, re-exported by `brocanth-policy` behind `macro` | |
| [`brocanth-jobs`](crates/brocanth-jobs/README.md) | Background jobs: retries with backoff, dead letter, scheduling, a worker pool | `jobs` |
| [`brocanth-queue`](crates/brocanth-queue/README.md) | The job contract: two lanes, a lease, a `Backend` trait, the outbox | |
| [`brocanth-queue-postgres`](crates/brocanth-queue-postgres/README.md) | The Postgres backend: `FOR UPDATE SKIP LOCKED`, outbox relay, partitioned archive | |
| [`brocanth-queue-memory`](crates/brocanth-queue-memory/README.md) | The in-process backend, the reference for the suite | |
| [`brocanth-queue-conformance`](crates/brocanth-queue-conformance/README.md) | The contract as a test suite for any backend | |
| [`brocanth-mail`](crates/brocanth-mail/README.md) | Mail: one trait, an in-memory backend, SMTP behind a feature | |
| [`brocanth-realtime`](crates/brocanth-realtime/README.md) | A realtime broker by topic and audience; `realtime-sse` and `realtime-websocket` carry it to a browser | `realtime` |
| [`brocanth-static`](crates/brocanth-static/README.md) | Writes the application to files: every GET page, partials per layout depth, the assets | `static` |
| [`brocanth-admin`](crates/brocanth-admin/README.md) | A Django-shaped backoffice at `/admin` | `admin` |
| [`brocanth-admin-model`](crates/brocanth-admin-model/README.md) | The admin's data source over SeaORM, one line per entity | `admin-model` (blocked) |
| [`brocanth-observe`](crates/brocanth-observe/README.md) | Logs on stdout, a request id, `/up` and `/readyz`, Prometheus metrics, OTLP traces | `observability` and its children |
| [`brocanth-analytics`](crates/brocanth-analytics/README.md) | Page views counted on the server: a daily visitor hash, no cookie, no third party; a beacon for static sites | `analytics`, `beacon` |
| [`brocanth-cli`](crates/brocanth-cli/README.md) | The `brocanth` CLI: `create`, `add`, `check`, `admin eject`, `admin diff`; it hands every other command to the project | |
| [`brocanth-scaffold`](crates/brocanth-scaffold/README.md) | The contract a scaffold module implements | |
| [`brocanth-test`](crates/brocanth-test/README.md) | An HTTP test client and an HTML reader for a built `App` | |

The templates come from sibling repositories:
[rsx](https://github.com/DHHwasRight/rsx) for `.rsx` (`rsx`) and
[jt](https://github.com/DHHwasRight/jt) for `.jt` (`jt`).

## Modules

A module is a choice that `brocanth create` offers. It implements
`brocanth_scaffold::Module`: a `Manifest` (key, gist, parent, needs, after,
categories, conflicts, adapts to) and a `contribute` that writes files and lines into named slots. The CLI
lists every module in one array, `ALL` in
`crates/brocanth-cli/src/modules/mod.rs`. A module lives beside the crate it
writes, behind a `scaffold` feature that is off by default.

[`docs/modules/`](docs/modules/README.md) has one page per module key.

**Five categories.** Each manifest declares its own with
`Manifest::category`; the first is the main one. A test fails when a
module writes into a slot its categories do not open
([`docs/module-map.md`](docs/module-map.md)):

| Category | What it does | Examples |
|---|---|---|
| Middleware | Runs around a request, and can put an item on the context | `sessions`, `middleware-csrf`, `policy` |
| Renderer | Makes a response body: a file type in `src/pages`, or a function | `rsx`, `jt`, `rsx-htmx` |
| Enhancer | Adds a capability: a plugin with a lifecycle, a subcommand, settings | `model`, `jobs`, `cache`, `commands` |
| Routes | Answers at a path | `admin` at `/admin`, `resource` at `/api`, `openapi` |
| External | Does not touch the code: build, ship, check | `docker`, `ci-github`, `static` |

**Five relations.** A child is *part of* a parent (`rsx-tailwind` is part
of `rsx`). A module *needs* another and the flags refuse it alone
(`client` needs `input` and `commands`). A module comes *after* another, so
two modules that write one slot do not depend on array order. A module
*conflicts* with another, and `create`, `add` and the chooser refuse the
pair (`logging` and `observability-logs` both write a line per request).
A module *adapts to* another when it writes different code beside it
(`cache` reads its settings from the config file when `config` is
chosen). The manifest declares each one, so a module from another
repository needs no edit to the CLI.
See "Manifest rules" in [`docs/modules.md`](docs/modules.md).

**Settings.** A module declares each setting once, with a `Setting`. A
generated project then reads it from three places, lowest precedence first:

1. `config/<env>.toml`, under `[<section>]`, with the `config` module;
2. the environment, `$<APP>_<SECTION>__<KEY>`;
3. a flag, `--<section>.<key>=VALUE`. A secret (a database URL, a signing
   secret) and a list have no flag, because every user on the host can read
   a process's arguments.

With `config`, the flags are off until you set `WITH_CONFIG = true` in
`src/flags.rs`. `RENAMES` in the same file renames a flag
(`("sessions.cookie", "cookie")`) or hides it (`("storage.root", "")`).
The project owns `src/flags.rs`, so `brocanth add` keeps your renames.
`cargo run -- settings` (or `--settings` without `commands`) prints each
layer and the merge. See [`docs/module-flags.md`](docs/module-flags.md).

To write a module, read [`docs/module-authoring.md`](docs/module-authoring.md)
and [`docs/modules.md`](docs/modules.md).

## The admin

`admin` mounts a Django-shaped backoffice at `/admin`. It ships locked: no
models, the `DenyAll` policy and an anonymous principal, so `/admin`
answers 403. A generated test asserts that. To open it, say who is asking
in the `principal` function of `src/plugins/admin/mod.rs`, a file you own,
and register a policy and your models.

Tune it at the lowest level that does the job. Each level up costs more
to keep in step with new versions:

| Level | What you change | How |
|---|---|---|
| Settings | titles, the "View site" link | `[admin]` keys `title`, `header`, `index_title`, `site_url`, or `--admin.<key>` |
| Theme | colours, fonts, radii | `[admin] stylesheet`: one CSS file linked after the shipped one |
| Assets | CSS and JavaScript files, every screen or one model's | `AdminConfig::script`, `ModelAdmin::{stylesheet, script}` |
| Replace a block | the brand or the user links | `Views::{with_brand, with_userlinks}` |
| Wrap or replace a screen | one model, or every model | `ModelAdmin::{list_view, ...}`, `Views::with_<screen>` |
| Replace the layout | the whole shell | `Views::with_layout` |
| Eject a screen | a copy of a shipped screen, as your own file | `brocanth admin eject list [--model invoice]`, `brocanth admin diff` |
| Add a page | a page of your own with the admin's chrome | `AdminConfig::page` |

[`docs/admin-customization.md`](docs/admin-customization.md) has all eight
levels, with Django's way beside each.
[`docs/backoffice.md`](docs/backoffice.md) says what the admin contains and
what is missing.

## Postgres first, then out

A small app runs on one binary and one Postgres. Each piece sits behind a
trait. When one piece outgrows Postgres, you move that piece and nothing
else.

| Piece | On Postgres today | The seam | The way out |
|---|---|---|---|
| Data | SeaORM, Postgres by default | `brocanth-model` | SQLite for a single machine (`features = ["sqlite"]`) |
| Jobs | `brocanth-jobs`: `SKIP LOCKED`, `LISTEN` to wake workers | its store (Postgres or SQLite) | `brocanth-queue` (below) |
| Queue | `PgQueue`, `PgOutbox` in `brocanth-queue-postgres` | `Backend`, `Outbox`, `Inspect` | The `Bulk` lane moves to another `Backend`. The contract is written against SQS limits. |
| Cache | `PgCache` on unlogged tables | `Cache` (tags, `CacheExt`) | Another `Cache` impl |
| Files | `Postgres` in `brocanth-storage` (`postgres` feature) | `Storage` | `LocalDisk` exists. An object store would be another `Storage` impl. |
| Realtime | `NotifySink` over `LISTEN/NOTIFY` | `ChangeSink`, `Notifier` | Another `ChangeSink` |
| Accounts | `PgStore` in `brocanth-auth` (`postgres` feature) | `AuthStore` and friends | Another `AuthStore` impl. `MemoryStore` is for tests. |

A job has a lane. A `Transactional` job commits with its data and stays in
your database. A `Bulk` job is idempotent and goes to whichever `Backend`
you run, through the outbox when you enqueue it inside a transaction. So a
move to a dedicated broker does not change one `enqueue` call. Today the
only backends are `PgQueue` and `Memory`, and `brocanth-jobs` shares only
the `Inspect` read side with `brocanth-queue`. Read
[`crates/brocanth-queue/docs/DECISIONS.md`](crates/brocanth-queue/docs/DECISIONS.md).

## Day-2 checklist

The d2s concerns, against what the code does today. Most items are modules:
opt-in, and the wizard's **Operations** question can untick them.

- [x] **Config from the environment (12-factor).** Environment, config
  files, plugin defaults, deep merged. Every value records its source, and
  a dump redacts secrets. An unclaimed section is an error.
- [x] **Deploy checks.** `brocanth check --deploy` reads the config and the
  source without a build or a database. Each finding has an id and a
  `help:` line, and it exits 1 on an error, so it runs in CI.
- [x] **Migrations you read before you run.** `brocanth migrate make` diffs
  the models against a snapshot by stable field id, so a rename stays a
  rename. `migrate up | down [N] | status` applies them.
- [x] **Background jobs that do not lose work.** Retries with backoff and
  jitter, a dead-letter table, scheduled and recurring jobs.
- [x] **Two-phase shutdown.** `serve` stops accepting, drains, and runs
  `App::shutdown` inside `[server] shutdown_grace_secs` (25 s, or
  `--server.shutdown_grace_secs`). With
  `observability-health`, `/readyz` fails first.
- [x] **Logs, probes, metrics, traces** (`observability-logs`, `-health`,
  `-metrics`, `-tracing`). JSON on stdout, `/up` and `/readyz`, Prometheus
  on `/metrics`, OTLP spans with W3C `traceparent`.
- [x] **Container image and deployment** (`docker`, `docker-kubernetes`).
- [x] **CI** (`ci-github`): fmt, clippy, tests, a Postgres service when the
  project has a database, and the image.
- [x] **CSRF, security headers, rate limiting** (`middleware-csrf`,
  `middleware-security-headers`, `middleware-rate-limit`).
- [x] **A Postgres `AuthStore`** (`PgStore`), used when the project has
  `jobs` or `cache`.
- [x] **Tests without a server.** `brocanth-test` drives a built `App` in
  process.
- [ ] **A non-Postgres queue backend** (SQS) and an **object-store
  `Storage`** (S3).
- [x] **A realtime transport.** `realtime-sse` writes server-sent events
  over the broker into the project. `realtime-websocket` writes a duplex
  WebSocket over it (`docs/realtime.md`).
- [x] **`brocanth remove`**, to take a module out again.

## Tests and the gate

The toolchain is pinned by `mise.toml`. Run the gate through mise before a
commit:

```sh
mise run ci          # build, clippy -D warnings, tests
cargo fmt --check
```

- **Postgres tests** skip when no database is set. Set
  `BROCANTH_TEST_DATABASE_URL` to run them all. Each test makes its own
  schema. `BROCANTH_QUEUE_REQUIRE_POSTGRES=1` turns a missing database into
  a failure.
- **Golden files.** `crates/brocanth-cli/tests/golden/` holds every file
  the generator writes for each shape. After a generator change, bless
  them and read the diff before you commit:

  ```sh
  BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- generator_writes
  ```

- **Module pages.** `cargo test -p brocanth-cli --bin brocanth -- docs_check`
  fails when a module key has no page in `docs/modules/`, or a page names
  no key.
- **Screens.** Test a screen with `brocanth-test` (`TestClient`, the HTML
  reader). Assert on behaviour, not on CSS class names.

[`CLAUDE.md`](CLAUDE.md) has the rest of the conventions.
[`docs/tooling.md`](docs/tooling.md) says how to keep `target/` small.

## Docs

[`docs/README.md`](docs/README.md) lists every page. Start with these:

| Read | For |
|---|---|
| [`PLAN.md`](PLAN.md) | The design. §8 is modularity, §12 the build order, §13 the ADRs. |
| [`docs/getting-started.md`](docs/getting-started.md) | Install, create, run, add pages, templates, commands. |
| [`docs/modules/`](docs/modules/README.md) | One page per module: what it writes, its settings. |
| [`docs/module-map.md`](docs/module-map.md) | The modules by category, with the dependency graph. |
| [`docs/module-flags.md`](docs/module-flags.md) | Settings, flags, environment variables, renames. |
| [`docs/llm-authoring.md`](docs/llm-authoring.md) | The contract for writing brocanth with a model. |
| [`docs/framework-gaps.md`](docs/framework-gaps.md) | The ranked gaps (G1 to G14) and their status. |
| [`docs/admin-customization.md`](docs/admin-customization.md) | How to tune the admin. |

## Sibling projects

All in [DHHwasRight](https://github.com/DHHwasRight).

- [**rsx**](https://github.com/DHHwasRight/rsx): compile-time HTML templates. A `.rsx` file is Rust with markup bodies.
- [**jt**](https://github.com/DHHwasRight/jt): compile-time JSON templates. JSON with holes, compiled into straight-line writes.
- [**zod**](https://github.com/DHHwasRight/zod): schemas that a literal must satisfy at compile time and input must satisfy at run time.
- [**strata**](https://github.com/DHHwasRight/strata): layered async caching that reads like memoization.
- [**htmx**](https://github.com/DHHwasRight/htmx): dp-swap, partial page updates in TypeScript with no dependencies.
- [**prerender**](https://github.com/DHHwasRight/prerender): a static site generator that builds each route as a full document and as fragments.
- [**depoth**](https://github.com/DHHwasRight/depoth): a static content framework. Templates compile; content does not.

## License

No license yet. There is no `LICENSE` file, and the workspace sets no
license. Only the four `brocanth-queue*` crates declare `license = "MIT"` in
their `Cargo.toml`.
