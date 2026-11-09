# Modules

One page per module that `brocanth create` offers. There are 47. For all
of them on one page, and what to build next, see
[module-catalog.md](../module-catalog.md). The list
and its order come from `ALL` in `crates/brocanth-cli/src/modules/mod.rs`.

Each page has the same parts:

| Part | Source |
|---|---|
| Gist, wizard question, part of, needs, after, crates, blocked | The module's `Manifest` |
| Categories | `Manifest::category`. The first is the main one, in bold. |
| Default, presets | `DEFAULT` in `commands/create.rs`, `PRESETS` in `wizard.rs` |
| Adapts to | `Manifest::adapts_to`: each module whose presence changes what this one writes |
| Conflicts | `Manifest::conflicts` on either side, and the `static` rule (`outside_runtime`), both read by `brocanth_scaffold::clash` |
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

The check also compares each page's `Categories`, `Adapts to` and
`Conflicts` rows with the manifest. To rewrite those three rows from the
manifests, run it with `BROCANTH_BLESS=1` and read the diff. The rest of a
page is not checked: when you change a manifest or a `Setting`, update the
page by hand.

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
| [`model`](model.md) | SeaORM, validations, migrations; without it no SQL driver at all | Enhancer, Middleware |  |
| [`cache`](cache.md) | a cache on an unlogged Postgres table, not Redis; without it no cache at all | Enhancer |  |
| [`jobs`](jobs.md) | a queue in Postgres with retries and a dead letter; a worker in this process | Enhancer |  |
| [`auth`](auth.md) | argon2id accounts and signed sessions; no provider, so no HTTP client | Middleware, Enhancer |  |
| [`policy`](policy.md) | typed predicates over an entity's columns; no ORM, no driver, no core | Middleware |  |
| [`storage`](storage.md) | uploads on a local disk, addressed by content hash; no object store and no S3 | Enhancer, Middleware |  |
| [`input`](input.md) | a form or a JSON body, read into a struct, with every fault at once | Enhancer, Renderer |  |
| [`client`](client.md) | a subcommand that calls this application's own API, built from its routes | Enhancer |  |
| [`middleware`](middleware.md) | proxy headers, method override, canonical host, content types, CORS | Middleware |  |
| [`middleware-csrf`](middleware-csrf.md) | a double-submit cookie, checked on every verb that is not a read | Middleware | middleware |
| [`middleware-security-headers`](middleware-security-headers.md) | HSTS, nosniff, a referrer policy, no framing, a CSP that runs dp-swap by hash | Middleware | middleware |
| [`middleware-rate-limit`](middleware-rate-limit.md) | a fixed window per address, session or user, per group of routes; 429 with Retry-After | Middleware | middleware |
| [`i18n`](i18n.md) | translations checked at build time; a locale per request from the query, a cookie or Accept-Language | Middleware |  |
| [`gtag`](gtag.md) | Google Analytics 4 behind a consent banner; nothing leaves before the visitor accepts | Middleware |  |
| [`openapi`](openapi.md) | an OpenAPI 3.1 document derived from src/pages, served at /openapi.json | Routes, Enhancer |  |
| [`validation`](validation.md) | a zod-shaped vocabulary; a handler takes Valid<T> or does not compile | Middleware |  |
| [`resource`](resource.md) | typed CRUD routes over one table, with the policy in the WHERE clause | Routes |  |
| [`admin`](admin.md) | a Django-shaped backoffice at /admin, locked until you say who may in | Routes |  |
| [`admin-model`](admin-model.md) | the admin's rows over SeaORM, one line per entity | Routes | admin |
| [`realtime`](realtime.md) | one write is a map lookup, not a query per subscriber | Enhancer, Middleware |  |
| [`realtime-sse`](realtime-sse.md) | one long response per subscriber, frames written as they are made | Routes | realtime |
| [`realtime-websocket`](realtime-websocket.md) | a duplex socket per subscriber, frames both ways | Routes | realtime |
| [`commands`](commands.md) | this project's own command line, `serve` plus whatever you add | Enhancer |  |
| [`rsx`](rsx.md) | `.rsx` templates compiled into the binary; without it a page builds a String | Renderer |  |
| [`jt`](jt.md) | `.jt` templates compiled into the binary; a JSON answer without a struct per shape | Renderer, Routes |  |
| [`rsx-tailwind`](rsx-tailwind.md) | a CSS build over the class names in your templates; needs node | External, Enhancer | rsx |
| [`rsx-htmx`](rsx-htmx.md) | a click swaps what changed; the server renders from the shared layout down | Renderer | rsx |
| [`static`](static.md) | `export` writes every GET page and every asset to dist/; no server, so no runtime modules | External, Enhancer |  |
| [`robots`](robots.md) | a robots.txt from your settings, served at /robots.txt; closed outside production | Routes |  |
| [`sitemap`](sitemap.md) | a sitemap.xml of every page with no parameter, served at /sitemap.xml | Routes |  |
| [`i18n-routing`](i18n-routing.md) | a locale segment in the path (/fr/about), a redirect from a path without one | Middleware, Renderer | i18n |
| [`meta`](meta.md) | a title, a description, a canonical URL and Open Graph tags for each page | Renderer, Enhancer |  |
| [`docker`](docker.md) | a cargo-chef Dockerfile: slim Debian, a user that is not root, a HEALTHCHECK on /up | External |  |
| [`docker-kubernetes`](docker-kubernetes.md) | deploy/kubernetes.yaml: a Deployment and a Service whose probes and grace period match the drain | External | docker |
| [`ci`](ci.md) | fmt, clippy and the tests on every push; pick the provider below | External |  |
| [`ci-github`](ci-github.md) | .github/workflows/ci.yml: fmt, clippy, tests, a Postgres service when there is a database, the image | External | ci |

## Blocked

None. A module that is on the menu and cannot be chosen gives the reason
on its page, from its manifest. `realtime-websocket` was the last; it
needed an upgrade out of the request/response shape and a frame codec,
which `brocanth-server` now has ([`docs/realtime.md`](../realtime.md)).
