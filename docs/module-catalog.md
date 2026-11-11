# Module catalog and roadmap

Every module `brocanth create` and `brocanth add` offer, on one page, and
what to build next. There are 71 modules. Each name links to its own page
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

### Middleware (23)

| Module | What it does | Also | Default | Needs / part of | Notes |
|---|---|---|---|---|---|
| [`analytics`](modules/analytics.md) | Page views counted on the server: no cookie, no script, no third party; rows in your Postgres |  |  | needs `db`: the views are rows in the database the `db` module opens |  |
| [`audit`](modules/audit.md) | Change history: who changed which row, when, and what; rows in your Postgres | Enhancer |  | needs `db`: the history is rows in the database the `db` module opens; `model`: it records the changes the model layer publishes |  |
| [`auth`](modules/auth.md) | Argon2id accounts and signed sessions; no provider, so no HTTP client | Enhancer |  |  |  |
| [`beacon`](modules/beacon.md) | A few lines of script that report each page view to an analytics server; no cookie |  |  |  | static-site safe |
| [`i18n`](modules/i18n.md) | Translations checked at build time; a locale per request from the query, a cookie or `Accept-Language` |  |  |  | static-site safe; every exported page is in the `default` locale |
| [`i18n-routing`](modules/i18n-routing.md) | A locale segment in the path (/fr/about), a redirect from a path without one | Renderer |  | part of `i18n`, off until you tick it | with `meta`: hreflang alternates and a canonical URL with the segment |
| [`gtag`](modules/gtag.md) | Google Analytics 4 behind a consent banner; nothing leaves before the visitor accepts |  |  |  | static-site safe |
| [`logging`](modules/logging.md) | A line per request; the core assumes no log layer |  |  |  |  |
| [`middleware`](modules/middleware.md) | Proxy headers, method override, canonical host, content types, CORS |  | yes |  |  |
| [`middleware-csrf`](modules/middleware-csrf.md) | A double-submit cookie, checked on every verb that is not a read |  |  | part of `middleware`, ticked with its parent |  |
| [`middleware-rate-limit`](modules/middleware-rate-limit.md) | A fixed window per address, session or user, per group of routes; 429 with Retry-After |  |  | part of `middleware`, off until you tick it |  |
| [`middleware-conditional-get`](modules/middleware-conditional-get.md) | A weak ETag over every page, and 304 for If-None-Match or If-Modified-Since |  |  | part of `middleware`, off until you tick it |  |
| [`middleware-compression`](modules/middleware-compression.md) | gzip and brotli per Accept-Encoding on text responses, with Vary; never a stream |  |  | part of `middleware`, off until you tick it | behind a CDN that compresses, leave it off |
| [`flash`](modules/flash.md) | One-shot messages across a redirect, in a signed cookie, drawn into the layout |  |  |  | with `rsx`: the root layout carries `<div id="flash">` |
| [`middleware-security-headers`](modules/middleware-security-headers.md) | HSTS, nosniff, a referrer policy, no framing, a CSP that runs dp-swap by hash |  | yes | part of `middleware`, ticked with its parent |  |
| [`observability`](modules/observability.md) | A request id on every request and a SIGTERM that fails /readyz first; logs, probes, metrics and traces below | Enhancer, Routes | yes |  |  |
| [`observability-logs`](modules/observability-logs.md) | A JSON line per request on stdout in production, text elsewhere; `tracing` and nothing else |  | yes | part of `observability`, ticked with its parent |  |
| [`observability-tracing`](modules/observability-tracing.md) | An OTLP span per request and per job, W3C traceparent in and out; needs a collector |  |  | part of `observability`, off until you tick it |  |
| [`policy`](modules/policy.md) | Typed predicates over an entity's columns; no ORM, no driver, no core |  |  |  |  |
| [`sessions`](modules/sessions.md) | A cookie, and a `Session` the context hands a handler |  |  |  |  |
| [`storage-routes`](modules/storage-routes.md) | Upload and download routes: uploads your guard allows, downloads behind a file token, `?thumb=WxH` |  |  | part of `storage`, off until you tick it |  |
| [`validation`](modules/validation.md) | A zod-shaped vocabulary; a handler takes Valid<T> or does not compile |  |  |  |  |
| [`webhooks-in`](modules/webhooks-in.md) | `POST /webhooks/{source}`: signature, replay window, idempotency key, then a typed handler; before the CSRF check |  |  | part of `webhooks`, ticked with its parent | with `db`: the idempotency keys are rows |

### Renderer (8)

| Module | What it does | Also | Default | Needs / part of | Notes |
|---|---|---|---|---|---|
| [`errors`](modules/errors.md) | HTML 404 and 500 pages in your layouts, the cause in a debug build, a reporter hook | Middleware |  |  | a client that does not read HTML keeps the plain answer |
| [`jt`](modules/jt.md) | `.jt` templates compiled into the binary; a JSON answer without a struct per shape | Routes |  |  | static-site safe |
| [`meta`](modules/meta.md) | A title, a description, a canonical URL and Open Graph tags for each page | Enhancer |  |  | static-site safe; no canonical URL until `base_url` is set |
| [`rsx`](modules/rsx.md) | `.rsx` templates compiled into the binary; without it a page builds a String |  |  |  | static-site safe |
| [`rsx-htmx`](modules/rsx-htmx.md) | A click swaps what changed; the server renders from the shared layout down |  |  | part of `rsx`, ticked with its parent | static-site safe |
| [`sanitize`](modules/sanitize.md) | An allow-list HTML sanitizer and a SafeHtml type; links html5ever |  |  |  | static-site safe |
| [`pagination`](modules/pagination.md) | Offset and keyset pages, with links that keep the query; the admin and `resource` page with it too |  |  |  | static-site safe |
| [`filters`](modules/filters.md) | Typed filters and a sort from the query string, ANDed with the policy's rule in the WHERE |  |  | needs `policy`: a filter is a `Pred` over a policy entity's columns | static-site safe |

### Routes (13)

| Module | What it does | Also | Default | Needs / part of | Notes |
|---|---|---|---|---|---|
| [`admin`](modules/admin.md) | A Django-shaped backoffice at /admin, locked until you say who may in | Enhancer |  |  |  |
| [`admin-model`](modules/admin-model.md) | The admin's rows over SeaORM, one line per entity |  |  | part of `admin`, off until you tick it; needs `model`: it reads rows through the ORM, and without `model` there is none |  |
| [`auth-oauth`](modules/auth-oauth.md) | Sign-in with GitHub, Google, Microsoft or GitLab at /oauth; links an HTTP client | Enhancer |  | part of `auth`, off until you tick it |  |
| [`auth-screens`](modules/auth-screens.md) | Sign-up, sign-in, sign-out, reset, verify and TOTP pages under /account | Enhancer |  | part of `auth`, off until you tick it | needs `rsx`, `input`, `middleware-csrf`, `mail`, `flash` |
| [`observability-health`](modules/observability-health.md) | /up for liveness, /readyz runs every plugin's check and fails first on SIGTERM |  | yes | part of `observability`, ticked with its parent |  |
| [`observability-metrics`](modules/observability-metrics.md) | Prometheus text on /metrics: requests by route and status, in-flight, job queues |  | yes | part of `observability`, ticked with its parent |  |
| [`openapi`](modules/openapi.md) | An OpenAPI 3.1 document derived from src/pages, served at /openapi.json | Enhancer |  |  |  |
| [`realtime-sse`](modules/realtime-sse.md) | One long response per subscriber, frames written as they are made |  |  | part of `realtime`, off until you tick it |  |
| [`realtime-websocket`](modules/realtime-websocket.md) | A duplex socket per subscriber, frames both ways |  |  | part of `realtime`, off until you tick it |  |
| [`resource`](modules/resource.md) | Typed CRUD routes over one table, with the policy in the WHERE clause |  |  |  |  |
| [`search`](modules/search.md) | Postgres full-text and trigram search: a site index, its migration, a page at /search |  |  | needs `db` and `pagination` |  |
| [`robots`](modules/robots.md) | A robots.txt from your settings, served at /robots.txt; closed outside production |  |  |  | static-site safe |
| [`sitemap`](modules/sitemap.md) | A sitemap.xml of every page with no parameter, served at /sitemap.xml; `robots` names it |  |  |  | static-site safe |

### Enhancer (21)

| Module | What it does | Also | Default | Needs / part of | Notes |
|---|---|---|---|---|---|
| [`admin-actions`](modules/admin-actions.md) | "Delete selected" on every admin list, with a confirmation page |  |  | part of `admin`, off until you tick it |  |
| [`cache`](modules/cache.md) | A cache on an unlogged Postgres table, not Redis; without it no cache at all |  |  |  |  |
| [`cache-memory`](modules/cache-memory.md) | The cache in this process's memory, for tests and development; never in production |  |  | part of `cache`, off until you tick it | `check --deploy` refuses it |
| [`cache-redis`](modules/cache-redis.md) | The cache on Redis or Valkey instead of Postgres; one more server to run |  |  | part of `cache`, off until you tick it |  |
| [`client`](modules/client.md) | A subcommand that calls this application's own API, built from its routes |  |  | needs `input`: the client is built from what each page takes and gives, and that table is what `input` writes; `commands`: the client is a subcommand, and a project with no command line has nowhere to put one |  |
| [`commands`](modules/commands.md) | This project's own command line, `serve` plus whatever you add |  | yes |  | static-site safe |
| [`config`](modules/config.md) | Config/<env>.toml, merged; without it nothing links a TOML parser |  | yes |  | static-site safe |
| [`db`](modules/db.md) | One Postgres pool and sea-query statements, read as `ctx.db()`; without it no SQL driver | Middleware, Renderer |  |  | with `commands` and no `model`: SQL migrations in migrations/ |
| [`flags`](modules/flags.md) | Feature flags: on for all, a percentage, users or groups; rows in your Postgres |  |  | needs `db`: the rules are rows in the database the `db` module opens |  |
| [`input`](modules/input.md) | A form or a JSON body, read into a struct, with every fault at once | Renderer |  |  |  |
| [`jobs`](modules/jobs.md) | A queue in Postgres with retries and a dead letter; a worker in this process |  |  |  |  |
| [`jobs-memory`](modules/jobs-memory.md) | The queue in this process's memory; for development and tests, gone at restart |  |  | part of `jobs`, off until you tick it | `check --deploy` refuses it |
| [`mail`](modules/mail.md) | Send mail through SMTP, or to the log or a folder in development |  |  |  | with `jobs`: `deliver_later` goes on the `mail` queue |
| [`auth-lockout`](modules/auth-lockout.md) | Locks an account for minutes after too many wrong passwords |  |  | part of `auth`, off until you tick it | with `auth-screens`: sign-in counts wrong passwords |
| [`model`](modules/model.md) | SeaORM, validations, migrations, over the `db` pool | Middleware, Renderer |  | needs `db`: the ORM runs over the pool the `db` plugin opens, and without `db` there is none |  |
| [`realtime`](modules/realtime.md) | One write is a map lookup, not a query per subscriber. `realtime-sse` and `realtime-websocket` carry it to a browser | Middleware |  |  |  |
| [`roles`](modules/roles.md) | Roles that grant permissions, stored with the accounts, for staff and the admin |  |  | needs `superuser`: only a superuser gives a role, and `superuser` reads the signed-in member of staff |  |
| [`storage`](modules/storage.md) | Uploads on a local disk, addressed by content hash; no object store and no S3 | Middleware |  |  |  |
| [`webhooks`](modules/webhooks.md) | Signed webhooks in both directions; pick a direction below |  |  |  |  |
| [`webhooks-out`](modules/webhooks-out.md) | Signed outgoing webhooks: endpoints and a delivery log in Postgres, retries through jobs |  |  | part of `webhooks`, off until you tick it; needs `db`: the endpoints and the log are rows; `jobs`: a delivery is a job | with `admin`: a deliveries tab |
| [`superuser`](modules/superuser.md) | Operator accounts that pass every admin check, and a createsuperuser command |  |  | needs `auth`: a superuser is an account with a flag, and `auth` holds the accounts |  |

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
2. ~~**`pagination`.**~~ Done: one public paginator
   (offset and keyset) that the admin and `resource` use. The admin's
   `per_page` stops at a ceiling (`ModelAdmin::max_per_page`, default 100).
   A keyset cursor of the wrong type is a 400 (`brocanth_policy::Sortable`).
   A keyset page has a previous link (`before=<cursor>`, `Keyset::before`,
   `KeysetPage::before`).

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
   Also done: `brocanth-i18n` provides the negotiated locale through
   `provide_for_request`, and a page reads it with `ctx.provided`.
5. **More module crates on the harness.** Twenty crates keep their own
   shapes and goldens through `brocanth_scaffold::testing` (in
   `tests/scaffold.rs`), the CLI's goldens beside them. The last five,
   ordered by the size of their generated output: `middleware`,
   `realtime`, `admin`, `db` and `jobs`.
   Remains, still on the CLI's goldens alone: `observe`, `i18n`,
   `policy`, `openapi`, `analytics`, `input`, `schema`, `gtag`,
   `resource`, `client`, from the most generated output to the least.
6. ~~**The gate covers generated projects.**~~ Done. `mise run generated`
   builds and tests every project of a crate test that calls
   `testing::build`, then the CLI's golden shapes, in one target
   directory; `mise run ci` runs it. The `scaffold` task reads its crate
   list from the manifests (it had left out `brocanth-analytics`). One
   test in the CLI holds every module's declared hooks to its plugin
   (`docs/module-config.md` §3.1). The gate is `mise run ci`; there is no
   GitHub CI.
7. ~~**A module reads another module.**~~ Done
   (`docs/module-config.md` §9): generated code reads
   `crate::modules::<key>::MODULE.<field>` where it uses the value, and
   never copies it into a setting. `i18n-routing` reads `storage-routes`'
   `path` and `admin`'s `mount_at`, and never redirects a socket or a
   stream. `sitemap`, `analytics` and `gtag` read `admin`'s `mount_at`
   for their exclusion, and the generated `robots` and `admin` tests
   follow a moved admin (§9, decisions 4 and 5). No library default names
   `/admin`. Remains: `add` does not print a field a newer module adds;
   the route table test of `src/pages/mod.rs` lists the admin's patterns
   as literals, so a move edits it too.

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
