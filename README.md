<h1 align="center">brocanth</h1>

<p align="center">A Rust server framework for HTML-first apps and JSON APIs. Postgres first, day-2 ready.</p>

---

> **Status.** Most of this code is vibe coded: written with an LLM, as a
> thought exercise. We use it for internal products where mission criticality
> is not needed. Nothing is on crates.io. Read it, take ideas from it, but do
> not bet a business on it.

## Why

The org name comes from RailsConf 2026. brocanth takes four positions DHH
argues for a web stack at their word, in Rust, to see how far they get:

- **HTML over the wire.** The server renders. The client swaps fragments
  (dp-swap, an htmx-shaped script).
- **No build step** for the front end. The one exception is opt-in:
  `rsx-tailwind` needs node.
- **One database.** Postgres holds the data, the jobs, the cache, the files
  and the realtime fan-out.
- **One framework hands you the whole stack**: routes, models, migrations,
  policies, jobs, auth, an admin.

We add one goal of our own. brocanth wants to be a Rust equivalent of
[d2s](https://github.com/IxDay/d2s), the "Day 2 Stack": ready for production,
devops and SRE from day one, and still simple. The
[day-2 checklist](#day-2-checklist) says how far that goal is from done.

brocanth is not only for HTML. The same router serves a JSON API with typed
input, OpenAPI 3.1, row-level policies and typed CRUD routes, and no
templates at all.

Two rules shape the code:

- **Nothing ships unless you ask for it.** The core is a router that hands a
  request and a context to a handler. Every other capability is a plugin
  that you name in `main`. A module that you do not pick is not in the
  source, the binary or `Cargo.lock` (PLAN.md §8).
- **Mistakes are compile errors.** A link to a missing route, a handler that
  reads an unregistered plugin, or a resource with no policy does not
  compile. This rule also makes the code easy for a model to write.
  `docs/llm-authoring.md` lists 36 such cases.

## What you get

The workspace has 36 crates under `crates/` and two examples under
`examples/` (`demo-app`, `nested-cards`). The **Module** column gives the
`--with` key that puts the crate in a new project.

**Core and server**

| Capability | Crate | Module |
|---|---|---|
| Router, plugin hooks, two-phase shutdown | `brocanth` | always |
| HTTP server on Tokio, streaming bodies | `brocanth-server` | always |
| File-based routes and nested layouts from `src/pages` | `brocanth-pages` | always |
| The whole route table in one `routes!` file (an alternative) | `brocanth-routes` | |
| Layered config with provenance and secret redaction | `brocanth-config` | `config` |
| Proxy headers, method override, canonical host, CORS, CSRF | `brocanth-middleware` | `middleware`, `middleware-csrf` |
| Form or JSON body read into a struct, all faults at once | `brocanth-input` | `input` |
| Schemas checked at compile time and at run time (`Valid<T>`) | `brocanth-schema` | `validation` |
| OpenAPI 3.1 document at `/openapi.json`, with a drift check | `brocanth-openapi` | `openapi` |
| A CLI client for your own API, built from the route table | `brocanth-client` | `client` |
| Project commands with `#[cmd]` | `brocanth-command`, `brocanth-command-macros` | `commands` |
| Message catalogs checked at build time, CLDR plurals | `brocanth-i18n` | no module yet |

**Data**

| Capability | Crate | Module |
|---|---|---|
| Model layer over SeaORM: validations, eager loading, N+1 detection, dirty tracking | `brocanth-model` | `model` |
| Stable field ids and a rename-aware migration diff | `brocanth-migrate` | via `model` |
| `RowChange` and `ChangeSink`: the seam between writes and subscribers | `brocanth-events` | via `model` |
| Typed CRUD routes over one table, policy in the `WHERE` | `brocanth-resource` | `resource` |
| Cache on unlogged Postgres tables, with tags | `brocanth-cache` | `cache` |
| File storage, content-addressed, with image variants | `brocanth-storage` | `storage` |

**Auth and policy**

| Capability | Crate | Module |
|---|---|---|
| argon2id accounts, signed sessions, OAuth 2.0 / OIDC with PKCE, MFA, OTP, impersonation | `brocanth-auth` | `auth` |
| Cookie sessions | (generated code) | `sessions` |
| Typed predicates: SQL for lists, evaluation for one row; `policy!` | `brocanth-policy`, `brocanth-policy-macros` | `policy` |

**Background work**

| Capability | Crate | Module |
|---|---|---|
| Jobs: retries with backoff, dead letter, scheduling, recurring work, worker pool | `brocanth-jobs` | `jobs` |
| The job contract: two lanes, a lease, a `Backend` trait, the outbox relay | `brocanth-queue` | |
| Postgres backend: `FOR UPDATE SKIP LOCKED`, outbox, partitioned archive | `brocanth-queue-postgres` | |
| In-process backend, the reference for the suite | `brocanth-queue-memory` | |
| The contract as a test suite for any backend | `brocanth-queue-conformance` | |
| Mail: one trait, an in-memory backend, SMTP behind a feature | `brocanth-mail` | no module yet |
| Realtime broker by topic and audience, multi-process over `LISTEN/NOTIFY` | `brocanth-realtime` | `realtime` |

**UI**

| Capability | Crate | Module |
|---|---|---|
| `.rsx` HTML templates compiled into the binary | [rsx](https://github.com/DHHwasRight/rsx) | `rsx` |
| `.jt` JSON templates compiled into the binary | [jt](https://github.com/DHHwasRight/jt) | `jt` |
| Partial navigation (dp-swap, a vendored script from [htmx](https://github.com/DHHwasRight/htmx)) | (generated code) | `rsx-htmx` |
| Tailwind CSS build over template class names | (generated code) | `rsx-tailwind` |
| Fingerprinted static assets, embedded at compile time | `brocanth-assets` | with `rsx` |
| JS and CSS bundles from `build.rs` (rolldown, lightningcss) | `brocanth-bundle` | |
| Django-shaped backoffice at `/admin` | `brocanth-admin`, `brocanth-admin-model` | `admin` |

**Ops and tooling**

| Capability | Crate | Module |
|---|---|---|
| `brocanth` CLI: `create`, `add`, `migrate`, `dbshell`, `check` | `brocanth-cli` | |
| The contract a scaffold module implements | `brocanth-scaffold` | |
| HTTP test client and HTML reader | `brocanth-test` | |
| One request log line per request | (generated code) | `logging` |

The menu also lists `admin-model`, `realtime-sse` and `realtime-websocket`.
They are blocked, and each row says why.

## Quick start

You need Rust 1.97 or later (`mise.toml` pins 1.98) and, for the `model`,
`jobs` and `cache` modules, a Postgres server.

Install the CLI from a checkout. Nothing is on crates.io yet.

```sh
git clone https://github.com/DHHwasRight/brocanth
cd brocanth
cargo install --path crates/brocanth-cli
```

Make an HTML app. `--brocanth-path` takes the core crate's directory, not the
checkout root.

```sh
brocanth create blog \
  --with logging,config,commands,model,sessions,rsx,rsx-htmx \
  --brocanth-path /path/to/brocanth/crates/brocanth
```

Without `--with` you get `logging,config,commands`. `--core-only` gives a
router and nothing else. Without a name, `brocanth create` opens a chooser
with four starting shapes: an HTML application, a JSON API, a worker, and
nothing.

Make an API-only service. There are no templates and no pages module:

```sh
brocanth create api \
  --with logging,config,commands,input,validation,policy,resource,openapi \
  --brocanth-path /path/to/brocanth/crates/brocanth
```

Run it and test it:

```sh
cd api
brocanth serve            # listens on 127.0.0.1:3000
curl http://127.0.0.1:3000/openapi.json
cargo test
```

Grow it later, and check it before a deploy:

```sh
brocanth add jobs                 # the same generator, one more module
brocanth migrate make add_posts   # diff src/models, write a SeaORM migration
brocanth check --deploy           # secrets, cookies, CSRF, the database
brocanth dbshell                  # psql on this environment's database
brocanth --help                   # also lists the project's own commands
```

Every project gets an `AGENTS.md`. It tells a model what the project
contains and what it does not. `docs/getting-started.md` walks through all
of this with real output.

## Postgres first, then out

A small app runs on one binary and one Postgres. Each piece sits behind a
trait. When one piece outgrows Postgres, you move that piece and nothing
else.

| Piece | On Postgres today | The seam | The way out |
|---|---|---|---|
| Data | SeaORM, Postgres by default | `brocanth-model` | SQLite for a single machine (`features = ["sqlite"]`) |
| Jobs | `brocanth-jobs`: `SKIP LOCKED`, `LISTEN` to wake workers | its store (Postgres or SQLite) | `brocanth-queue` (below) |
| Queue | `PgQueue`, `PgOutbox` in `brocanth-queue-postgres` | `Backend`, `Outbox`, `Inspect` | The `Bulk` lane moves to another `Backend`. The outbox relays jobs that were enqueued in a transaction. The contract is written against SQS limits. |
| Cache | `PgCache` on unlogged tables | `Cache` (tags, `CacheExt`) | Another `Cache` impl. The route cache and `TagInvalidator` stay the same. |
| Files | `Postgres` in `brocanth-storage` (`postgres` feature), bytes in `bytea` | `Storage` | `LocalDisk` exists. An object store would be another `Storage` impl. |
| Realtime | `NotifySink` over `LISTEN/NOTIFY` (`listen-notify` feature) | `ChangeSink`, `Notifier` | Another `ChangeSink`. The broker and the subscribers stay the same. |
| Accounts | `PgStore` in `brocanth-auth` (`postgres` feature) | `AuthStore` (+ `MfaStore`, `OtpStore`, ...) | Another `AuthStore` impl. `MemoryStore` is for tests. Both pass one conformance suite. |

The queue shows the pattern best. A job has a lane. A `Transactional` job
commits with the data that it refers to and stays in your database. A `Bulk`
job is idempotent and goes to whichever `Backend` you run. If you enqueue a
`Bulk` job inside a transaction, it goes into the outbox table first. A relay
then moves it to the backend. So the move to a dedicated broker does not
change one `enqueue` call. Read `crates/brocanth-queue/README.md` and
`docs/DECISIONS.md` for the design.

Today the only `Backend` implementations are `PgQueue` and `Memory`. The SQS
backend that the contract is written for does not exist yet. `brocanth-jobs`
does not run on `brocanth-queue` yet. It shares only the `Inspect` read side,
so one admin panel can show both.

## Day-2 checklist

The d2s concerns, against what the code does today.

- [x] **Config from the environment (12-factor).** Sources, highest
  precedence first: environment variables (`MY_APP_SERVER__PORT`),
  `$MY_APP_CONFIG`, `/etc/<app>/<env>.*`, `./<app>_<env>.*`,
  `./config/<env>.*`, then plugin defaults. Deep merge. Every value records
  its source, and a dump redacts secrets. An unclaimed section is an error.
- [x] **Deploy checks.** `brocanth check --deploy` reads the config and the
  source without a build or a database. It reports insecure cookies, unset
  signing keys, default secrets, CSRF off, debug logging, a temporary
  database, and more. Each finding has an id and a `help:` line. It exits 1
  on an error, so it runs in CI.
- [x] **Migrations you read before you run.** `brocanth migrate make` diffs
  the models against a snapshot by stable field id, so a rename stays a
  rename. It writes a file and applies nothing.
- [x] **Background jobs that do not lose work.** Retries with backoff and
  jitter, a dead-letter table, requeue, scheduled and recurring jobs. The
  same backend runs in dev, test and production.
- [x] **Shutdown in two phases, in the framework.** Each plugin's
  `begin_shutdown` hook closes its gate. `App::shutdown` drains every plugin in reverse registration
  order inside one shared grace period and returns a `ShutdownReport`
  (PLAN.md ADR 2). `Stopper` lets in-flight connections finish. Open
  streams drain through `StreamGate`.
- [x] **Tests without a server.** `brocanth-test` drives a built `App` in
  process. Postgres tests make their own schema.
- [x] **Structured logs on stdout.** `brocanth-observe` writes one JSON or text line per request and per `tracing` event, with a request id (JSON by default in production).
- [x] **Health probes.** `/up` asks nothing; `/readyz` runs each plugin's `ready_when` check and fails before shutdown drains.
- [x] **Metrics.** `/metrics` serves Prometheus text: requests by route template and status class, in-flight, and job queue depth and runs.
- [x] **Tracing.** The `otel` feature exports a span per request and per job run over OTLP, with W3C `traceparent` in and out.
- [ ] **Graceful shutdown on SIGTERM in a generated project.** The generated
  `serve` does not listen for signals and does not call `App::shutdown`.
  You must wire it yourself.
- [ ] **Container image.** There is no Dockerfile and no generated one.
- [ ] **Apply migrations from the CLI.** There is no `migrate up`. You call
  SeaORM's migrator from your own code or a project command.
- [x] **Security headers.** `brocanth_middleware::SecurityHeaders` sends
  HSTS (over HTTPS only), nosniff, Referrer-Policy, framing, a CSP that
  allows dp-swap's inline scripts by hash, and Permissions-Policy.
- [x] **Rate limiting.** `brocanth_middleware::RateLimit` counts a fixed
  window per IP, session or user for each route group and answers 429 with
  `Retry-After` and `RateLimit-*`. In memory, or in one UNLOGGED Postgres
  table.
- [x] **A Postgres `AuthStore`.** `PgStore` (`postgres` feature); a generated project still starts on `MemoryStore`.
- [ ] **A non-Postgres queue backend** (SQS) and an **object-store
  `Storage`** (S3).
- [ ] **A realtime transport.** The broker works. The SSE and WebSocket
  modules are listed and blocked.
- [ ] **`brocanth remove`**, to take a module out again.

## Docs

| Read | For |
|---|---|
| [`PLAN.md`](PLAN.md) | The design. §8 is modularity, §12 the build order, §13 the ADRs. |
| [`docs/getting-started.md`](docs/getting-started.md) | Install, create, run, add pages, templates, commands. |
| [`docs/llm-authoring.md`](docs/llm-authoring.md) | The contract for writing brocanth with a model, and the crate map. |
| [`docs/framework-gaps.md`](docs/framework-gaps.md) | The ranked gaps (G1–G14) and their status. |
| [`docs/modules.md`](docs/modules.md) | How a crate reaches a generated project: `Slot`s, the `scaffold` feature, the CLI registry. |
| [`docs/tooling.md`](docs/tooling.md) | Fixtures, `dbshell`, `check --deploy`, prefetch. |
| [`docs/migrations.md`](docs/migrations.md) | `migrate make` and stable field ids. |
| [`docs/policy.md`](docs/policy.md) | Predicates, `condition()`, `Verified`, `policy!`. |
| [`docs/input.md`](docs/input.md) | Reading a request body into a struct. |
| [`docs/middleware.md`](docs/middleware.md) | The middleware hooks. |
| [`docs/observability.md`](docs/observability.md) | Logs, probes, metrics and traces: `brocanth-observe`. |
| [`docs/cache.md`](docs/cache.md) | Tags, route cache, fragments, invalidation. |
| [`docs/auth-store.md`](docs/auth-store.md) | Where accounts live: `MemoryStore`, `PgStore`, the conformance suite. |
| [`docs/async-handlers.md`](docs/async-handlers.md) | How a route gets to `.await`, and the cost. |
| [`docs/surface.md`](docs/surface.md) | How a handler's signature becomes the OpenAPI document and the client. |
| [`docs/backoffice.md`](docs/backoffice.md) | The admin. |
| [`docs/i18n.md`](docs/i18n.md) | Catalogs, locales, plurals. |
| [`docs/pocketbase-surface.md`](docs/pocketbase-surface.md) | What brocanth measures itself against, and what it declines. |
| [`crates/brocanth-queue/docs/DECISIONS.md`](crates/brocanth-queue/docs/DECISIONS.md) | The queue design, its defects, and its open items. |

To work on brocanth itself: `mise run ci` runs build, test and clippy. Set
`BROCANTH_TEST_DATABASE_URL` to run the Postgres tests. `CLAUDE.md` has the
rest.

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
