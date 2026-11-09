# Module catalog and roadmap

Every module `brocanth create` and `brocanth add` offer, on one page, and
what to build next. There are 46 modules. Each name links to its own page
in `docs/modules/`, which lists the files it writes and its settings.

This page is a summary. When it disagrees with a module page or the code,
the code wins: `modules::docs_check` holds the pages to the manifests, and
nothing holds this page.

## How to read the tables

- **Main category.** Each module is listed once, under its main category.
  `Also` names its other categories. See `docs/module-map.md` for what each
  category may write.
- **Default.** `yes` means a non-interactive `brocanth create my-app`
  chooses it. The wizard presets choose other sets.
- **Needs / part of.** A module that is part of another is refused without
  its parent. A module that needs another is refused without it.
- **Static-site safe.** The module has `outside_runtime`, so the `static`
  preset can take it. Every other module needs a running server, and
  `static` refuses it.

## The modules

### Middleware (17)

| Module | What it does | Also | Default | Needs / part of | Notes |
|---|---|---|---|---|---|
| [`analytics`](modules/analytics.md) | Page views counted on the server: no cookie, no script, no third party; rows in your Postgres |  |  | needs `db`: the views are rows in the database the `db` module opens |  |
| [`auth`](modules/auth.md) | Argon2id accounts and signed sessions; no provider, so no HTTP client | Enhancer |  |  |  |
| [`beacon`](modules/beacon.md) | A few lines of script that report each page view to an analytics server; no cookie |  |  |  | static-site safe |
| [`i18n`](modules/i18n.md) | Translations checked at build time; a locale per request from the query, a cookie or `Accept-Language` |  |  |  | static-site safe; every exported page is in the `default` locale |
| [`i18n-routing`](modules/i18n-routing.md) | A locale segment in the path (/fr/about), a redirect from a path without one | Renderer |  | part of `i18n`, off until you tick it | with `meta`: hreflang alternates and a canonical URL with the segment |
| [`gtag`](modules/gtag.md) | Google Analytics 4 behind a consent banner; nothing leaves before the visitor accepts |  |  |  | static-site safe |
| [`logging`](modules/logging.md) | A line per request; the core assumes no log layer |  |  |  |  |
| [`middleware`](modules/middleware.md) | Proxy headers, method override, canonical host, content types, CORS |  | yes |  |  |
| [`middleware-csrf`](modules/middleware-csrf.md) | A double-submit cookie, checked on every verb that is not a read |  |  | part of `middleware`, ticked with its parent |  |
| [`middleware-rate-limit`](modules/middleware-rate-limit.md) | A fixed window per address, session or user, per group of routes; 429 with Retry-After |  |  | part of `middleware`, off until you tick it |  |
| [`middleware-security-headers`](modules/middleware-security-headers.md) | HSTS, nosniff, a referrer policy, no framing, a CSP that runs dp-swap by hash |  | yes | part of `middleware`, ticked with its parent |  |
| [`observability`](modules/observability.md) | A request id on every request and a SIGTERM that fails /readyz first; logs, probes, metrics and traces below | Enhancer, Routes | yes |  |  |
| [`observability-logs`](modules/observability-logs.md) | A JSON line per request on stdout in production, text elsewhere; `tracing` and nothing else |  | yes | part of `observability`, ticked with its parent |  |
| [`observability-tracing`](modules/observability-tracing.md) | An OTLP span per request and per job, W3C traceparent in and out; needs a collector |  |  | part of `observability`, off until you tick it |  |
| [`policy`](modules/policy.md) | Typed predicates over an entity's columns; no ORM, no driver, no core |  |  |  |  |
| [`sessions`](modules/sessions.md) | A cookie, and a `Session` the context hands a handler |  |  |  |  |
| [`validation`](modules/validation.md) | A zod-shaped vocabulary; a handler takes Valid<T> or does not compile |  |  |  |  |

### Renderer (3)

| Module | What it does | Also | Default | Needs / part of | Notes |
|---|---|---|---|---|---|
| [`jt`](modules/jt.md) | `.jt` templates compiled into the binary; a JSON answer without a struct per shape | Routes |  |  | static-site safe |
| [`rsx`](modules/rsx.md) | `.rsx` templates compiled into the binary; without it a page builds a String |  |  |  | static-site safe |
| [`rsx-htmx`](modules/rsx-htmx.md) | A click swaps what changed; the server renders from the shared layout down |  |  | part of `rsx`, ticked with its parent | static-site safe |

### Routes (10)

| Module | What it does | Also | Default | Needs / part of | Notes |
|---|---|---|---|---|---|
| [`admin`](modules/admin.md) | A Django-shaped backoffice at /admin, locked until you say who may in | Enhancer |  |  |  |
| [`admin-model`](modules/admin-model.md) | The admin's rows over SeaORM, one line per entity |  |  | part of `admin`, off until you tick it; needs `model`: it reads rows through the ORM, and without `model` there is none |  |
| [`observability-health`](modules/observability-health.md) | /up for liveness, /readyz runs every plugin's check and fails first on SIGTERM |  | yes | part of `observability`, ticked with its parent |  |
| [`observability-metrics`](modules/observability-metrics.md) | Prometheus text on /metrics: requests by route and status, in-flight, job queues |  | yes | part of `observability`, ticked with its parent |  |
| [`openapi`](modules/openapi.md) | An OpenAPI 3.1 document derived from src/pages, served at /openapi.json | Enhancer |  |  |  |
| [`realtime-sse`](modules/realtime-sse.md) | One long response per subscriber, frames written as they are made |  |  | part of `realtime`, off until you tick it |  |
| [`realtime-websocket`](modules/realtime-websocket.md) | A duplex socket per subscriber, frames both ways |  |  | part of `realtime`, off until you tick it |  |
| [`resource`](modules/resource.md) | Typed CRUD routes over one table, with the policy in the WHERE clause |  |  |  |  |
| [`robots`](modules/robots.md) | A robots.txt from your settings, served at /robots.txt; closed outside production |  |  |  | static-site safe |
| [`sitemap`](modules/sitemap.md) | A sitemap.xml of every page with no parameter, served at /sitemap.xml; `robots` names it |  |  |  | static-site safe |

### Enhancer (10)

| Module | What it does | Also | Default | Needs / part of | Notes |
|---|---|---|---|---|---|
| [`cache`](modules/cache.md) | A cache on an unlogged Postgres table, not Redis; without it no cache at all |  |  |  |  |
| [`client`](modules/client.md) | A subcommand that calls this application's own API, built from its routes |  |  | needs `input`: the client is built from what each page takes and gives, and that table is what `input` writes; `commands`: the client is a subcommand, and a project with no command line has nowhere to put one |  |
| [`commands`](modules/commands.md) | This project's own command line, `serve` plus whatever you add |  | yes |  | static-site safe |
| [`config`](modules/config.md) | Config/<env>.toml, merged; without it nothing links a TOML parser |  | yes |  | static-site safe |
| [`db`](modules/db.md) | One Postgres pool and sea-query statements, read as `ctx.db()`; without it no SQL driver | Middleware, Renderer |  |  | with `commands` and no `model`: SQL migrations in migrations/ |
| [`input`](modules/input.md) | A form or a JSON body, read into a struct, with every fault at once | Renderer |  |  |  |
| [`jobs`](modules/jobs.md) | A queue in Postgres with retries and a dead letter; a worker in this process |  |  |  |  |
| [`model`](modules/model.md) | SeaORM, validations, migrations, over the `db` pool | Middleware, Renderer |  | needs `db`: the ORM runs over the pool the `db` plugin opens, and without `db` there is none |  |
| [`realtime`](modules/realtime.md) | One write is a map lookup, not a query per subscriber. `realtime-sse` and `realtime-websocket` carry it to a browser | Middleware |  |  |  |
| [`storage`](modules/storage.md) | Uploads on a local disk, addressed by content hash; no object store and no S3 | Middleware |  |  |  |

### External (6)

| Module | What it does | Also | Default | Needs / part of | Notes |
|---|---|---|---|---|---|
| [`ci`](modules/ci.md) | Fmt, clippy and the tests on every push; pick the provider below |  | yes |  | static-site safe |
| [`ci-github`](modules/ci-github.md) | .github/workflows/ci.yml: fmt, clippy, tests, a Postgres service when there is a database, the image |  | yes | part of `ci`, ticked with its parent | static-site safe |
| [`docker`](modules/docker.md) | A cargo-chef Dockerfile: slim Debian, a user that is not root, a HEALTHCHECK on /up |  | yes |  |  |
| [`docker-kubernetes`](modules/docker-kubernetes.md) | Deploy/kubernetes.yaml: a Deployment and a Service whose probes and grace period match the drain |  |  | part of `docker`, off until you tick it; needs `observability-health`: its liveness and readiness probes are `/up` and `/readyz` |  |
| [`rsx-tailwind`](modules/rsx-tailwind.md) | A CSS build over the class names in your templates; needs node | Enhancer |  | part of `rsx`, ticked with its parent; needs `commands`: the CSS build is `brocanth css`, a subcommand, and a project with no command line has nowhere to put it | static-site safe |
| [`static`](modules/static.md) | `export` writes every GET page and every asset to dist/; no server, so no runtime modules | Enhancer |  | needs `commands`: `export` is a subcommand of this binary | static-site safe |

## How a module reaches a project

- **Settings.** A module declares each setting once. With `config`, it is
  a key in `config/<env>.toml`. Without `config`, it is a flag
  `--<section>.<key>` and an environment variable
  `$<APP>_<SECTION>__<KEY>`. Secrets get no flag. See
  `docs/module-flags.md`.
- **Rules in the manifest.** `needs`, `part_of`, `conflicts`, `adapts_to`
  and `category` are data. `create`, `add` and the chooser check them in
  one place (`create::selection`). See `docs/modules.md`.
- **The CLI stays small.** A module crate keeps its runtime behind a
  default `runtime` feature, and the CLI takes only its `scaffold` side.
  `crates/brocanth-cli/tests/lean.rs` fails when a runtime crate reaches the
  CLI. The installed CLI is 2.4 MB with `--profile dist`.
- **Writing one.** See `docs/module-authoring.md`.

## What to build next

New modules from other frameworks are ranked in
[module-candidates.md](module-candidates.md).

Ranked: first what a user hits today, then what unblocks new modules, then
the rest. Each item says where it is recorded.

### 1. Small fixes a user can hit

Done. The docs no longer say that `brocanth` has only `create` or that a
generated project skips the hook pipeline. Every test that reads a test
database variable treats an empty value as unset. `[server]
shutdown_grace_secs` has a flag and a variable in every project
(`docs/module-flags.md`, "The framework's own settings").

### 2. New modules

1. ~~**`realtime-websocket`.**~~ Done: `brocanth-server` has the upgrade
   (`upgrade`) and the codec (feature `websocket`), and the module writes
   the socket loop and `GET /realtime/socket`. Open items are in
   `docs/realtime.md` §10: RFC 8441 over HTTP/2, `permessage-deflate`,
   subprotocols.

### 3. Module system

1. ~~**`brocanth remove <module>`.**~~ Done. It is `add` in reverse
   (`docs/modules.md`, "Removing from a project"). An edited
   `src/modules/<key>.rs` stays, with its renames and other choices, and
   so do ejected admin screens and `migrations/`.
   Remains: a remove does not touch the database, so a table of a removed
   module stays until a migration drops it.
2. ~~**Module versions.**~~ Done. `brocanth.toml` records each module's
   version, and `brocanth check` warns on a difference
   (`check/module-version`). Remains: the version does not say *what*
   changed. An upgrade that shows a module's changes as a diff needs the
   old generator, or the old output kept in the project.
3. ~~**A `ServerWrap` slot.**~~ Done: `Slot::ServerWrap` and a generated
   `ServerWrap` trait; the first line is the outermost wrap. `observability`
   and `rsx`'s static files use it (gap 1).
4. ~~**Smaller seams.**~~ Done: `Plugin::provide_for_request` sees the
   request (gap 10), and the `rsx` branches in `create.rs` went from
   sixteen to three, the starter pages, which stay by decision (gap 11).
   Next: `brocanth-i18n` can provide the negotiated locale through
   `provide_for_request`.
5. **More module crates on the harness.** `brocanth-robots` keeps its
   shapes and goldens in its own crate through `brocanth_scaffold::testing`.
   The other module crates still rely on the CLI's goldens.

### 4. Admin

Done: the `brand` and `userlinks` blocks (`Views::with_brand`,
`with_userlinks`, `AdminConfig::user_label`); extra pages
(`AdminConfig::page`, `Page::hidden`, `Panel::in_nav`); per-model
JavaScript as files (`ModelAdmin::script`, `AdminConfig::script`; no
inline code, so no hash and no nonce); `brocanth admin diff`.

What remains, from `docs/admin-customization.md`, "Open":

1. A model's script does not load on a dp-swap into its screen: dp-swap
   keeps the `<head>`. The fix is in dp-swap: merge the response's
   `<head>`.
2. Pages under one model (Django's `ModelAdmin.get_urls`).
3. A three-way `admin diff`: it needs the template of the version a copy
   came from.

### 5. Open questions to decide

1. **`gtag` consent log retention.** Rotate the file, or leave it to the
   deployment (`docs/gtag.md`)?
2. **`gtag` with another provider.** Reuse the banner for a self-hosted
   counter that sets cookies (`docs/gtag.md`)?
3. **CLI size below 2.4 MB.** Replacing `cargo_metadata` with a direct
   `cargo metadata` call saves about 100 KB; trimming clap's features saves
   less. Each costs some convenience.

### Done in the last rounds

For context, not to do: per-module settings with flags and config, the
manifest rules, `admin eject` (CLI and project command), `robots`,
`analytics` with `beacon`, `gtag`, `sitemap` (the base URL is
`[sitemap] base_url`; see `docs/robots.md`), SQL migrations for `db`
without `model` (`docs/migrations.md` §7; open: no checksum of an applied
file), `i18n` (with the `gtag` banner in the request's locale,
`docs/i18n.md` §7), `migrate` and `dbshell` as project
commands, and a CLI with no module runtime.

Also done: a test harness for out-of-tree modules
(`brocanth_scaffold::testing` with `brocanth_cli::testing::generate`,
gap 8), a chooser that groups modules by main category, a structured
`snippet::hooking_line` (gap 6), and `package.json` from
`Slot::NodeDependencies` (gap 9).
