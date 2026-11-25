# Modules

One page per module that `brocanth create` offers. There are 90. For all
of them on one page, and what to build next, see
[module-catalog.md](../module-catalog.md). The list
and its order come from `ALL` in `crates/brocanth-cli/src/modules/mod.rs`.

Each page has the same parts:

| Part | Source |
|---|---|
| Gist, wizard question, part of, crates, blocked | The module's `Manifest` |
| Categories | `Manifest::category`. The first is the main one, in bold. |
| Default, presets | `DEFAULT` in `commands/create.rs`, `PRESETS` in `wizard.rs` |
| Conflicts | `Manifest::conflicts` on either side, and the `static` rule (`outside_runtime`), both read by `brocanth_scaffold::clash` |
| Capabilities | Generated from the `Manifest` between `<!-- generated: capabilities -->` markers: needs, after, provides, requires, uses, adapts to, contributes, collects, exports, hooks (phase, required), options (build time) and settings (run time). `brocanth explain` prints the same facts for a project. |
| What it writes | The files and slots its `contribute` writes in a project with `config` |
| Settings | The `Setting` declarations. The file value is what `config/<env>.toml` starts with; the plugin's `Default` can differ. |

**Why pages and not one table:** the table in `module-map.md` answers "how
do the modules relate". A page answers "what does this one module do to my
project", and the settings table alone is too wide for one row.

**How the pages stay true:** `modules::docs_check` in `brocanth-cli` fails
when a key in `ALL` has no page here, or when a page here names no key.
Run it with:

```
cargo test -p brocanth-cli --bin brocanth -- docs_check
```

The check also compares each page's `Categories` and `Conflicts` rows and
its generated `Capabilities` table with the manifest, so a manifest edit
without a re-bless fails it. To rewrite the rows and the table from the
manifests, run it with `BROCANTH_BLESS=1` and read the diff. The prose is
not checked: when you change what a module writes, update it by hand.

## Settings, in short

A declared setting reaches a generated project three ways:

- a key in `config/<env>.toml` under `[<section>]`, with the `config` module;
- an environment variable `$<APP>_<SECTION>__<KEY>` (a dot in the key is `__`);
- a flag `--<section>.<key>=VALUE`. A secret and a list have no flag.

Precedence, lowest first: the plugin's default, the config files, the
environment, a flag. With `config`, flags are off until
`flags_with_config: true` in `src/modules/brocanth.rs`, or `flags: Flags::On`
in the module's own file. `rename` in `src/modules/<key>.rs` renames or hides
a flag. See
[`../module-flags.md`](../module-flags.md).

## The modules

A hyphenated key (`parent-child`) is part of the module before the dash.

| Module | Gist | Categories | Part of |
|---|---|---|---|
| [`logging`](logging.md) | a line per request; the core assumes no log layer | Middleware |  |
| [`observability`](observability.md) | a request id on every request and a SIGTERM that fails /readyz first; logs, probes, metrics and traces below | Middleware, Enhancer, Routes |  |
| [`observability-logs`](observability-logs.md) | a JSON line per request on stdout in production, text elsewhere; `tracing` and nothing else | Middleware | observability |
| [`observability-health`](observability-health.md) | /up for liveness, /readyz runs every plugin's check and fails first on SIGTERM | Routes | observability |
| [`observability-metrics`](observability-metrics.md) | Prometheus text on /metrics: requests by route and status, in-flight, job queues | Routes | observability |
| [`observability-tracing`](observability-tracing.md) | an OTLP span per request and per job, W3C traceparent in and out; needs a collector | Middleware | observability |
| [`analytics`](analytics.md) | page views counted on the server: no cookie, no script, no third party; rows in your Postgres | Middleware |  |
| [`beacon`](beacon.md) | a few lines of script that report each page view to an analytics server; no cookie | Middleware |  |
| [`config`](config.md) | config/<env>.toml, merged; without it nothing links a TOML parser | Enhancer |  |
| [`sessions`](sessions.md) | a cookie, and a `Session` the context hands a handler | Middleware |  |
| [`db`](db.md) | one Postgres pool and sea-query statements, read as ctx.db(); without it no SQL driver | Enhancer, Middleware, Renderer |  |
| [`db-embedded`](db-embedded.md) | a Postgres server started from the binary, for development with no Postgres installed; never in production | Enhancer | db |
| [`db-embedded-prod`](db-embedded-prod.md) | a Postgres server started from the binary, in production too: one binary, one data directory, an archive pinned by hash | Enhancer | db |
| [`model`](model.md) | SeaORM, validations, migrations; without it no SQL driver at all | Enhancer, Middleware |  |
| [`flags`](flags.md) | feature flags: on for all, a percentage, users or groups; rows in your Postgres | Enhancer |  |
| [`audit`](audit.md) | change history: who changed which row, when, and what; rows in your Postgres | Middleware, Enhancer |  |
| [`cache`](cache.md) | a cache on an unlogged Postgres table, not Redis; without it no cache at all | Enhancer |  |
| [`cache-memory`](cache-memory.md) | the cache in this process's memory, for tests and development; never in production | Enhancer |  |
| [`cache-redis`](cache-redis.md) | the cache on Redis or Valkey instead of Postgres; one more server to run | Enhancer |  |
| [`jobs`](jobs.md) | a queue in Postgres with retries and a dead letter; a worker in this process | Enhancer |  |
| [`jobs-memory`](jobs-memory.md) | the queue in this process's memory; for development and tests, gone at restart | Enhancer | jobs |
| [`mail`](mail.md) | send mail through SMTP, or to the log or a folder in development | Enhancer |  |
| [`auth`](auth.md) | argon2id accounts and signed sessions; no provider, so no HTTP client | Middleware, Enhancer |  |
| [`auth-oauth`](auth-oauth.md) | sign-in with GitHub, Google, Microsoft or GitLab at /oauth; links an HTTP client | Routes, Enhancer | auth |
| [`auth-screens`](auth-screens.md) | sign-up, sign-in, sign-out, reset, verify and TOTP pages under /account | Routes, Enhancer | auth |
| [`auth-lockout`](auth-lockout.md) | locks an account for minutes after too many wrong passwords | Enhancer | auth |
| [`auth-otp`](auth-otp.md) | sign in with an eight-digit code mailed to the address, under /account/otp | Routes, Enhancer | auth |
| [`auth-mfa`](auth-mfa.md) | who must use a second factor, forced TOTP set-up, recovery codes, a QR code and login alerts | Routes, Middleware, Enhancer | auth |
| [`superuser`](superuser.md) | operator accounts that pass every admin check, and a createsuperuser command | Enhancer |  |
| [`roles`](roles.md) | roles that grant permissions, stored with the accounts, for staff and the admin | Enhancer |  |
| [`impersonate`](impersonate.md) | staff act as a customer, with a banner on every page, the operator on the session and an audit entry | Routes, Middleware, Enhancer |  |
| [`policy`](policy.md) | typed predicates over an entity's columns; no ORM, no driver, no core | Middleware |  |
| [`rls`](rls.md) | Postgres row-level security declared in Rust; `rls make` writes the migration | Enhancer |  |
| [`storage`](storage.md) | uploads on a local disk, or in Postgres with `db`, addressed by content hash; no S3 | Enhancer, Middleware |  |
| [`storage-routes`](storage-routes.md) | POST /files and GET /files/{address}?token=: uploads your guard allows, downloads behind a file token | Middleware | storage |
| [`storage-thumbnails`](storage-thumbnails.md) | ?thumb=WxH on the file routes, in listed sizes, rendered once each; links four image decoders | Middleware | storage |
| [`image`](image.md) | thumbnails converted on request (&fmt=webp), a pixel cap against decompression bombs, a srcset helper | Renderer |  |
| [`input`](input.md) | a form or a JSON body, read into a struct, with every fault at once | Enhancer, Renderer |  |
| [`client`](client.md) | a subcommand that calls this application's own API, built from its routes | Enhancer |  |
| [`middleware`](middleware.md) | proxy headers, method override, canonical host, content types, CORS | Middleware |  |
| [`middleware-csrf`](middleware-csrf.md) | a double-submit cookie, checked on every verb that is not a read | Middleware | middleware |
| [`middleware-security-headers`](middleware-security-headers.md) | HSTS, nosniff, a referrer policy, no framing, a CSP that runs dp-swap by hash | Middleware | middleware |
| [`middleware-rate-limit`](middleware-rate-limit.md) | a fixed window per address, session or user, per group of routes; 429 with Retry-After | Middleware | middleware |
| [`middleware-conditional-get`](middleware-conditional-get.md) | a weak ETag over every page, and 304 for If-None-Match or If-Modified-Since | Middleware | middleware |
| [`middleware-compression`](middleware-compression.md) | gzip and brotli per Accept-Encoding on text responses, with Vary; never a stream | Middleware | middleware |
| [`flash`](flash.md) | one-shot messages across a redirect, in a signed cookie, drawn into the layout | Middleware |  |
| [`i18n`](i18n.md) | translations checked at build time; a locale per request from the query, a cookie or Accept-Language | Middleware |  |
| [`gtag`](gtag.md) | Google Analytics 4 behind a consent banner; nothing leaves before the visitor accepts | Middleware |  |
| [`openapi`](openapi.md) | an OpenAPI 3.1 document derived from src/pages, served at /openapi.json | Routes, Enhancer |  |
| [`validation`](validation.md) | a zod-shaped vocabulary; a handler takes Valid<T> or does not compile | Middleware |  |
| [`resource`](resource.md) | typed CRUD routes over one table, with the policy in the WHERE clause | Routes |  |
| [`admin`](admin.md) | a Django-shaped backoffice at /admin, locked until you say who may in | Routes |  |
| [`admin-model`](admin-model.md) | the admin's rows over SeaORM, one line per entity | Routes | admin |
| [`admin-actions`](admin-actions.md) | "Delete selected" on every admin list, with a confirmation page | Enhancer | admin |
| [`backoffice`](backoffice.md) | the admin as a service of its own: a second binary that serves only the admin | Middleware, External |  |
| [`webhooks`](webhooks.md) | signed webhooks: verified deliveries in, signed events out; pick a direction below | Enhancer |  |
| [`webhooks-in`](webhooks-in.md) | POST /webhooks/{source}: signature, replay window and idempotency key, then your typed handler | Middleware | webhooks |
| [`webhooks-out`](webhooks-out.md) | signed outgoing webhooks: endpoints and a delivery log in Postgres, retries through jobs | Enhancer | webhooks |
| [`oauth`](oauth.md) | an OAuth 2.0 client for third-party APIs (not sign-in); pick its parts below | Enhancer |  |
| [`oauth-client`](oauth-client.md) | connect pages at /connections; tokens sealed in Postgres, refreshed once, sent as a bearer | Routes, Enhancer | oauth |
| [`oauth-client-memory`](oauth-client-memory.md) | the connected accounts in this process's memory; for tests and development, gone at restart | Enhancer | oauth |
| [`realtime`](realtime.md) | one write is a map lookup, not a query per subscriber | Enhancer, Middleware |  |
| [`realtime-sse`](realtime-sse.md) | one long response per subscriber, frames written as they are made | Routes | realtime |
| [`realtime-websocket`](realtime-websocket.md) | a duplex socket per subscriber, frames both ways | Routes | realtime |
| [`commands`](commands.md) | this project's own command line, `serve` plus whatever you add | Enhancer |  |
| [`rsx`](rsx.md) | `.rsx` templates compiled into the binary; without it a page builds a String | Renderer |  |
| [`jt`](jt.md) | `.jt` templates compiled into the binary; a JSON answer without a struct per shape | Renderer, Routes |  |
| [`rsx-tailwind`](rsx-tailwind.md) | a CSS build over the class names in your templates; needs node | External, Enhancer | rsx |
| [`rsx-htmx`](rsx-htmx.md) | a click swaps what changed; the server renders from the shared layout down | Renderer | rsx |
| [`sanitize`](sanitize.md) | an allow-list HTML sanitizer and a SafeHtml type; links html5ever | Renderer |  |
| [`markdown`](markdown.md) | Markdown to cleaned HTML: GFM tables and footnotes, heading ids, TOML front matter; links html5ever | Renderer |  |
| [`static`](static.md) | `export` writes every GET page and every asset to dist/; no server, so no runtime modules | External, Enhancer |  |
| [`robots`](robots.md) | a robots.txt from your settings, served at /robots.txt; closed outside production | Routes |  |
| [`sitemap`](sitemap.md) | a sitemap.xml of every page with no parameter, served at /sitemap.xml | Routes |  |
| [`content`](content.md) | Markdown files with typed front matter, checked by the build, listed at /posts; no database | Enhancer, Routes |  |
| [`feed`](feed.md) | an Atom feed of the posts at /feed.xml, with absolute URLs from `--feed.site_url` | Routes |  |
| [`pagination`](pagination.md) | offset and keyset pages, with links that keep the query; no dependency | Renderer |  |
| [`filters`](filters.md) | typed filters and a sort from the query string, ANDed with the policy's rule in the WHERE | Renderer |  |
| [`search`](search.md) | Postgres full-text and trigram search: a site index, its migration, a page at /search | Routes |  |
| [`i18n-routing`](i18n-routing.md) | a locale segment in the path (/fr/about), a redirect from a path without one | Middleware, Renderer | i18n |
| [`humanize`](humanize.md) | readable values for a page: "3 minutes ago", 1.5 MB, 2nd, 1,234; no dependency | Renderer |  |
| [`meta`](meta.md) | a title, a description, a canonical URL and Open Graph tags for each page | Renderer, Enhancer |  |
| [`errors`](errors.md) | HTML 404 and 500 pages in your layouts, the cause in a debug build, a reporter hook | Renderer, Middleware |  |
| [`debug`](debug.md) | development tools, refused by `check --deploy`; pick them below | Middleware |  |
| [`debug-toolbar`](debug-toolbar.md) | a panel on each HTML page in a debug build: timing, SQL statements, hooks, settings | Middleware | debug |
| [`docker`](docker.md) | a cargo-chef Dockerfile: slim Debian, a user that is not root, a HEALTHCHECK on /up | External |  |
| [`docker-kubernetes`](docker-kubernetes.md) | deploy/kubernetes.yaml: a Deployment and a Service whose probes and grace period match the drain | External | docker |
| [`ci`](ci.md) | fmt, clippy and the tests on every push; pick the provider below | External |  |
| [`ci-github`](ci-github.md) | .github/workflows/ci.yml: fmt, clippy, tests, a Postgres service when there is a database, the image | External | ci |
| [`ci-security`](ci-security.md) | .github/workflows/security.yml: cargo audit and cargo deny on each push and each day; deny.toml | External | ci |

## Blocked

None. A module that is on the menu and cannot be chosen gives the reason
on its page, from its manifest. `realtime-websocket` was the last; it
needed an upgrade out of the request/response shape and a frame codec,
which `brocanth-server` now has ([`docs/realtime.md`](../realtime.md)).
