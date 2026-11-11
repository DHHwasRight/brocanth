# Module candidates

New modules worth building, from a scan of Django, Rails, Next.js and
PocketBase and their most-used packages (2026-11). Each candidate is
something those frameworks ship or their users install most, that
brocanth does not cover yet. The full tables, with sources, follow the
ranking.

The audits in `docs/audit/` are older. Many of their "missing" items are
now built (CSRF, CSP, rate limit, i18n, sitemap, robots, WebSocket,
migration diffing, uploads, route caching). This page replaces their
"what next" parts.

## Combined ranking

Rank = value for an HTML-first Rust application, then effort (S, M, L),
then what the module unblocks. Both scans put `mail` first.

| # | Key | What it does | Maps to | Effort |
|---|---|---|---|---|
| 1 | `mail` | **Built** ([page](modules/mail.md)). Sends mail from a project; `deliver_later` through `jobs`. | Action Mailer, django.core.mail, react-email, PocketBase mail | S |
| 2 | `flash` | **Built** ([page](modules/flash.md)). Messages that survive a redirect after a form POST | contrib.messages, Rails flash | S |
| 3 | `auth-screens` | **Built** ([page](modules/auth-screens.md)). Sign-up, sign-in, reset, verify, TOTP screens over `brocanth-auth`, which has the flows and no pages. Needs `mail` and `flash`. | Devise, allauth, Rails 8 auth generator | M |
| 4 | `auth-oauth` | **Built** ([page](modules/auth-oauth.md)). Provider settings and the callback route; the provider code is in `brocanth-auth`. | Auth.js, better-auth, allauth social, PocketBase OAuth2 | M |
| 5 | `meta` | Title, description, canonical and Open Graph per page; every page shares `pages::TITLE` today. Base for OG images and hreflang. **Built:** `brocanth-meta`, `docs/modules/meta.md`. | Next.js metadata, next-seo, django-meta | M |
| 6 | `storage-routes` | **Built** ([page](modules/storage-routes.md)). Upload and download routes, `?thumb=`, protected file tokens over `storage` | Active Storage, uploadthing, PocketBase files | S |
| 7 | `errors` | HTML 404 and 500 pages in the layout chain, a dev error page, a reporter hook; a missing page answers plain `Not Found` today. **Built:** `brocanth-errors`, `docs/modules/errors.md`. | Next.js not-found/error, Django 404/500 templates, Sentry | M |
| 8 | `compression` | gzip and brotli on responses. **Built** as `middleware-compression`. | GZipMiddleware, Rack::Deflater | S |
| 9 | `conditional-get` | Dynamic `ETag` and 304. **Built** as `middleware-conditional-get`. | ConditionalGetMiddleware, `fresh_when` | S |
| 10 | `superuser` | **Built** ([page](modules/superuser.md)). Operator accounts and a `createsuperuser` command, so the admin is usable without writing a policy first | createsuperuser, PocketBase superusers | S |
| 11 | `webhooks-in` | **Built** ([page](modules/webhooks-in.md)). Incoming webhooks: signature check, replay window, idempotency key | Stripe/svix receivers, PocketHost | S |
| 12 | `sanitize` | Allow-list HTML sanitizer; unblocks rich text, comments, Markdown. **Built**: crate `brocanth-sanitize`. | bleach/nh3, Rails sanitize | S |
| 13 | `pagination` | One public paginator, offset and keyset. **Built** (`brocanth-pagination`); admin and `resource` page with it. | pagy, kaminari, Django Paginator | S |
| 14 | `auth-lockout` | **Built** ([page](modules/auth-lockout.md)). Locks an account after failed logins | django-axes, Devise lockable | S |
| 15 | `i18n-routing` | `/fr/...` paths and hreflang; a query or cookie locale is not indexed. **Built:** in `brocanth-i18n`, `docs/modules/i18n-routing.md`. | next-intl, i18n_patterns | S |
| 16 | `search` | Postgres full-text and trigram search, with a migration and a search page. **Built** (`brocanth-search`). | pg_search, contrib.postgres.search, PocketBase FTS plugins | M |
| 17 | `filters` | Typed query-string filters compiled into the `policy` `WHERE`. **Built** (`brocanth-filters`). | django-filter, ransack | M |
| 18 | `webhooks-out` | **Built** ([page](modules/webhooks-out.md)). Signed outgoing webhooks with retries, over row changes, `jobs` and the outbox | svix, PocketHost webhooks | M |
| 19 | `audit` | Change history over `RowChange`, with an admin history page. **Built** (`brocanth-audit`). | paper_trail, django-simple-history | M |
| 20 | `roles` | **Built** ([page](modules/roles.md)). Groups and permissions for staff and admin access | django.contrib.auth groups, pundit roles | M |
| 21 | `flags` | Feature flags with an admin panel. **Built** (`brocanth-flags`). | Flipper, django-waffle, Vercel flags | M |
| 22 | `admin-actions` | Bulk actions with a confirm page, then inlines. **Built**, without inlines (`brocanth-admin`). | Django admin actions, ActiveAdmin batch actions | M |

Next after these: `markdown` with `content` and `feed`, `image`,
`auth-otp` and `auth-mfa`, `debug-toolbar`, `impersonate`,
`notifications`, `ci-security` (`cargo audit`), `humanize`.

**Needs a decision first:** `storage-s3`. It is high value, but PLAN.md §4
says no S3. **Declined by PLAN.md §12:** backups, a JavaScript hook VM,
dynamic collections.

## Django and Rails scan


Date: 2026-11-09. brocanth read at `25e908a` (44 modules, `docs/module-catalog.md`).

#### Basis

- Builds on `docs/audit/django.md` (D1–D88) and `docs/audit/rails.md` (R1–R95).
  Those audits read `5017473`. Many of their "no" rows are now done: CSRF,
  security headers and CSP, rate limit, i18n, sitemap, robots, admin on the menu,
  SSE and WebSocket, migration diffing, multipart (`brocanth-input/src/multipart.rs`),
  a Postgres `AuthStore` (`brocanth-auth/src/postgres.rs`), route and fragment
  cache (G8), `dbshell`, fixtures, `check --deploy` (G9).
- Verified in the code for this list:
  - `brocanth-mail` exists (Memory and SMTP backends, a plugin). It has **no
    module** in `crates/brocanth-cli/src/modules/mod.rs` `ALL`.
  - `brocanth-auth` has OAuth providers, TOTP, MFA, recovery codes and
    impersonation. The `auth` module writes only `src/plugins/auth/mod.rs` on
    `MemoryStore`. It writes no sign-in, sign-up or reset screen. Passkeys are a
    named, unimplemented `FactorKind`.
  - No flash API (only a comment in `brocanth-auth/src/session.rs`).
  - No compression (`brocanth-middleware/src/lib.rs:72` says so). No dynamic
    `ETag` hook in any crate.
  - No S3 backend, by decision (`brocanth-storage/src/storage.rs:8`).
  - No full-text search, no audit history, no feature flags, no tenancy, no HTML
    sanitizer, no `cargo audit` / `cargo deny` in the CI module.

#### Popularity sources

- Django Developers Survey 2025 (4,600+ answers): DRF 49%, django-debug-toolbar 27%,
  Celery 26%, django-cors-headers 19%, django-filter 18%.
  <https://djangoproject.com/weblog/2025/oct/31/django-developers-survey-2025-results>
- Wagtail, "2025 state of Django's top packages": django-allauth 705k/month, Celery
  928k, django-ninja, Wagtail; high-download "unsung": django-cors-headers,
  django-model-utils, django-storages, django-filter.
  <https://next.wagtail.org/blog/the-2025-state-of-djangos-top-packages/>
- 2024 Rails Community Survey (2,709 answers): Devise in about 50% of apps, Pundit
  45%, Stimulus now ahead of React, Brakeman 31% for security scans.
  <https://socket.dev/blog/highlights-from-the-2024-rails-community-survey>,
  <https://blog.planetargon.com/blog/series/2024-ruby-on-rails-community-survey-results>

#### Legend

- **Covered**: yes / partly / no, with the brocanth module or crate.
- **Category**: as in `docs/module-map.md`.
- **Effort**: S = up to 3 days, M = 1–2 weeks, L = more.
- **Value**: 1–5 for an HTML-first Rust app.

#### Candidates

##### Already covered (for reference)

| Candidate | What it does | Django / Rails | Covered | Category | Effort | Value | Reason |
|---|---|---|---|---|---|---|---|
| `jobs` | Postgres queue, retries, cron, panel | Celery, celery-beat, flower; Active Job, Solid Queue, Sidekiq, good_job, mission_control-jobs | yes (`jobs`, `jobs-memory`) | Enhancer | – | 5 | Goes further than Solid Queue: cron and dead letter built in. One engine since 2026-11-09: a layer over `brocanth-queue`, on the `db` pool, with a memory child for development |
| `cache` | Postgres cache, tags, route and fragment cache | Django cache, `cache_page`; Solid Cache, fragment caching | yes (`cache`, `cache-memory`, `cache-redis`) | Enhancer | – | 5 | Done (G8). Backends built: memory for tests and development, Redis/Valkey; one conformance suite |
| `realtime` | Broker plus SSE and WebSocket | Channels; Action Cable, Solid Cable, Turbo Streams | yes (`realtime-*`) | Enhancer | – | 4 | Done (G1) |
| `admin` | Backoffice over the models | contrib.admin; rails_admin, Avo, ActiveAdmin, Administrate | yes (`admin`, `admin-model`) | Routes | – | 5 | Missing: actions, inlines, history (see `audit`, `admin-actions` below) |
| `middleware` CORS | CORS headers | django-cors-headers; rack-cors | yes (`middleware`) | Middleware | – | 3 | Done |
| `middleware-csrf` | Double-submit token | CsrfViewMiddleware; `protect_from_forgery` | yes | Middleware | – | 5 | Done |
| `middleware-security-headers` | HSTS, CSP by hash, no framing | SecurityMiddleware, django-csp; secure_headers, CSP DSL | yes | Middleware | – | 5 | Done |
| `middleware-rate-limit` | Fixed window per address, session or user | django-ratelimit; rack-attack, Rails `rate_limit` | yes | Middleware | – | 4 | No lockout after failed logins (see `auth-lockout`) |
| `observability` | Request id, logs, /up, /readyz, metrics, OTLP | django-health-check, structlog, django-prometheus; lograge, `/up`, yabeda | yes | Middleware | – | 5 | Done |
| `openapi` / `resource` | Typed CRUD JSON and an OpenAPI document | DRF, django-ninja, drf-spectacular; jbuilder, rswag | yes (`resource`, `openapi`) | Routes | – | 3 | API is secondary in an HTML-first app |
| `policy` | Row rules compiled to SQL | django-guardian, django-rules; Pundit, CanCanCan | yes (`policy`) | Middleware | – | 5 | Answers "which rows", not "which people" (see `roles`) |
| `i18n` | Build-checked catalogs, locale per request | django i18n, `LocaleMiddleware`; rails-i18n | yes (`i18n`) | Middleware | – | 4 | Done (G9) |
| `sitemap`, `robots` | `/sitemap.xml`, `/robots.txt` | contrib.sitemaps; sitemap_generator | yes | Routes | – | 3 | Done |
| `analytics` | Server-side page views in Postgres | django-analytical; Ahoy | yes (`analytics`) | Middleware | – | 3 | Done |
| `validation` / `input` | Typed forms, multipart, re-render on 422 | Forms, ModelForm; strong params, `form_with` | yes | Enhancer | – | 5 | Done (G2, G11) |
| `rsx-tailwind`, assets | CSS build, fingerprinted assets | staticfiles, whitenoise; Propshaft, tailwindcss-rails | yes | External | – | 4 | Done |
| `rsx-htmx` (dp-swap) | Swaps, mutating verbs, out-of-band | django-htmx; Turbo, Stimulus | yes | Renderer | – | 5 | Done (G2) |
| `docker`, `ci` | Dockerfile, GitHub Actions | – ; Rails 8 Dockerfile, `bin/ci` | yes | External | – | 4 | Done; Kamal is a separate row below |

##### Partly covered or not covered

| Candidate | What it does | Django / Rails | Covered | Category | Effort | Value | Reason |
|---|---|---|---|---|---|---|---|
| `mail` | Send mail through SMTP or a recorder; `deliver_later` through `jobs`; rsx templates | django.core.mail, django-anymail, `MAILERS`; Action Mailer, letter_opener | partly: crate `brocanth-mail`, no module | Enhancer | S | 5 | The crate is written; reset and verify mail cannot leave a generated app |
| `mail-preview` | Dev screen that lists and renders each mail | django-mail-viewer; mailer previews, letter_opener_web | no | Routes | S | 3 | GET-only dev screen; cheap once `mail` exists; could be part of `mail` |
| `auth-screens` | Sign-up, sign-in, sign-out, reset, verify, TOTP screens in rsx, on the Postgres store | django-allauth, contrib.auth views, django-otp; Devise views, Rails 8 auth generator | no: `auth` writes a plugin only | Routes | M | 5 | Devise is in half of Rails apps; the auth crate has 8,875 lines and no page |
| `flash` | One-shot messages across a redirect, with levels | contrib.messages; `flash` | no | Middleware | S | 5 | Every POST-redirect-GET form needs it; sessions already exist |
| `compression` | gzip and brotli on text responses, `Vary: Accept-Encoding` | GZipMiddleware; Rack::Deflater, Thruster | yes (`middleware-compression`) | Middleware | S | 4 | HTML pages compress 5–10x; the route cache already expects it |
| `conditional-get` | Dynamic `ETag` and `Last-Modified`, 304 | ConditionalGetMiddleware, `@condition`; `fresh_when`, `stale?` | yes (`middleware-conditional-get`) | Middleware | S | 4 | The hook contract was designed for it; dp-swap GETs gain most |
| `audit` | Row history from `RowChange`; who, when, diff; admin history page | django-simple-history, django-auditlog, admin `LogEntry`; paper_trail, audited | yes (`audit`) | Enhancer | M | 4 | Model changes already publish (G4); a sink and a table finish it |
| `roles` | Groups, permissions, staff flag; `require_role` on a route | contrib.auth groups and permissions; rolify, Devise + Pundit roles | no | Enhancer | M | 4 | `policy` covers rows; admin and staff screens ask "which people" |
| `auth-lockout` | Lock or slow an account after N failed logins; notify by mail | django-axes; Devise lockable, rack-attack throttle | partly: rate limit by address only | Middleware | S | 4 | Credential stuffing defence beside the existing rate limiter |
| `search` | Postgres full-text (`tsvector`, ranking) and trigram on a model | contrib.postgres.search, django-watson; pg_search, searchkick | yes (`search`) | Routes | M | 4 | Done: a site index, its migration and /search |
| `filters` | Typed list filters and sort from the query string, compiled to `WHERE` | django-filter, admin `list_filter`; ransack | yes (`filters`) | Renderer | M | 4 | Done: a `Pred` ANDed with the policy's rule |
| `pagination` | One general paginator: offset and keyset, page links in rsx | Paginator; pagy, kaminari | yes (`pagination`) | Renderer | S | 4 | Done: admin and resource use it |
| `flags` | Feature flags per user, group, percentage; admin panel | django-waffle, django-flags; Flipper | yes (`flags`) | Enhancer | M | 4 | Safe rollout without a deploy; cache and admin are ready |
| `errors` | Error reporting to Sentry or a log, with context; HTML 404/500 pages; dev error page | Django technical 500, `ADMINS` mail, sentry-sdk; `Rails.error`, exceptions_app, better_errors | partly: `ErrorHook` seam only | Enhancer | M | 4 | No HTML error page and no reporter today |
| `debug-toolbar` | Dev panel: SQL queries, N+1, timings, context per request | django-debug-toolbar, django-silk; rack-mini-profiler, bullet | partly: `brocanth-model` probe | Middleware | M | 4 | 27% of Django users name it a favourite; probe already finds N+1 |
| `storage-s3` | S3-compatible backend for `storage` | django-storages; Active Storage S3 service | no, by decision (later crate) | Enhancer | M | 4 | Most deployments have no persistent disk |
| `attachments` | A file column on a model: attach, purge, variants by name | FileField, ImageField; `has_one_attached` | no | Enhancer | M | 4 | Joins `storage` and `model`; admin file widget needs it |
| `sanitize` | Allow-list HTML sanitizer (ammonia) as a template filter | bleach / nh3; `sanitize` helper | yes (`sanitize`) | Renderer | S | 4 | Needed before any user HTML or Markdown reaches a page |
| `rich-text` | Rich text field: an editor island, sanitized storage, attachments | django-ckeditor-5, Wagtail RichText; Action Text (Trix) | no | Renderer | L | 3 | Needs `sanitize` and `attachments`; one JS island |
| `impersonate` | Staff signs in as a user, with a banner and an audit line | django-hijack; pretender, devise_masquerade | partly: `brocanth-auth/src/impersonation.rs`, no module | Routes | S | 3 | Logic exists; needs an admin button and a banner |
| `notifications` | In-app notifications plus delivery by mail and realtime | django-notifications-hq; Noticed | no | Enhancer | M | 3 | Common SaaS need; composes `mail`, `realtime`, `jobs` |
| `feeds` | RSS and Atom from a list | contrib.syndication; `atom_feed` builder | no | Routes | S | 3 | Small; fits content sites and the `static` preset |
| `redirects` | Old-URL to new-URL table, checked on 404 | contrib.redirects, django-redirects; – | no | Middleware | S | 3 | Keeps SEO after URL changes; admin edits rows |
| `humanize` | `time_ago`, `intcomma`, `filesizeformat`, pluralize, number and currency formats | contrib.humanize; ActionView number and date helpers | no | Renderer | S | 3 | Small helpers every page uses; locale from `i18n` |
| `slugs` | Unique slug column, history of old slugs, redirect | django-autoslug; friendly_id | no | Enhancer | S | 3 | Readable URLs are an HTML-first habit |
| `soft-delete` | `deleted_at`, default exclusion, restore | django-safedelete, model-utils; discard, paranoia | no | Enhancer | S | 3 | Common; must stay a `policy` condition, not a filter after load |
| `encrypted-fields` | Encrypt a column at rest, deterministic option for lookup | django-fernet-fields; `encrypts` | no | Enhancer | M | 3 | Compliance; key comes from `config` secrets |
| `secrets` | Encrypted credentials file per environment | django-environ (env only); Rails credentials | partly: `config` declines secrets by design | Enhancer | M | 3 | Decided against in `brocanth-config`; revisit only if users ask |
| `tenancy` | Tenant from subdomain or path; scope every query | django-tenants, django-tenant-schemas; acts_as_tenant, apartment | no | Middleware | L | 3 | SaaS need; must go through `policy` conditions |
| `import-export` | CSV export and import of a model in the admin | django-import-export; ActiveAdmin CSV, active_admin_import | no | Routes | M | 3 | Common admin ask; plain HTML forms |
| `admin-actions` | Bulk actions with a confirm page; inlines | ModelAdmin actions, inlines; ActiveAdmin batch actions | yes (`admin-actions`); inlines no | Routes | M | 4 | Biggest gap of the existing admin |
| `api-keys` | Personal API tokens: hashed, scoped, revocable | DRF TokenAuthentication, django-rest-knox, simplejwt; `has_secure_token` | no | Middleware | M | 3 | For the `client` and API users; secondary in HTML-first |
| `passkeys` | WebAuthn sign-in and second factor | django-otp-webauthn, django-allauth MFA; devise-passkeys, webauthn-ruby | no: named `FactorKind::Passkey` | Enhancer | M | 3 | Auth crate planned for it; needs a small JS island |
| `captcha` | Honeypot and Turnstile or hCaptcha on public forms | django-recaptcha, django-simple-captcha; invisible_captcha | no | Middleware | S | 3 | Spam on sign-up and contact forms |
| `pg-insights` | Admin panel: slow queries, index use, bloat, locks | django-pg-extras; PgHero | no | Routes | M | 3 | Postgres is the base; read-only screens |
| `sql-explorer` | Saved SQL queries and charts for staff | django-sql-explorer; Blazer | no | Routes | M | 2 | Useful, but a large attack surface |
| `money` | Money type with currency and exact decimal | django-money; money-rails | no | Enhancer | S | 2 | A `rust_decimal` newtype; small audience |
| `tags` | Tags on any model, tag cloud, filter | django-taggit; acts-as-taggable-on | no | Enhancer | M | 2 | Needs generic relations, which brocanth has none of |
| `deploy-kamal` | `config/deploy.yml` for Kamal or a similar tool | – ; Kamal 2 | no | External | S | 3 | Docker exists; one file finishes the deploy story |
| `ci-security` | `cargo audit` and `cargo deny` steps in CI | bandit, pip-audit; Brakeman, bundler-audit | no | External | S | 3 | 31% of Rails teams run Brakeman; two CI lines |
| `dev-loop` | `brocanth dev`: watch, rebuild, restart, CSS watch together | `runserver` autoreload; `bin/dev`, Procfile.dev | partly: `cargo watch` by hand, `css -W` | External | S | 3 | Every user wants one command |
| `inbound-mail` | Receive mail through a webhook and route it to a handler | django-anymail inbound; Action Mailbox | no | Routes | L | 2 | Few apps need it |
| `pages-cms` | Editable static pages or a CMS tree | contrib.flatpages, Wagtail, django CMS | no | Routes | L | 2 | Wagtail is popular, but a CMS is a product, not a module |
| `pwa` | Web manifest and a service worker route | – ; Rails 8 PWA files | no | Routes | S | 2 | Small; few HTML-first apps need offline |
| `maintenance` | 503 page behind a flag, staff pass through | django-maintenance-mode; turnout | no | Middleware | S | 2 | Easy; `/readyz` already covers drains |

Out of scope, not listed as candidates: `contrib.gis` (D77), `contenttypes` generic relations (D39),
`contrib.sites`, Action Cable channels with `perform()` (R41, PLAN.md §5), Hotwire Native.

#### Top 15 not yet covered, ranked

Rank = value first, then effort, then what it unblocks.

1. **`mail`** (S, 5). The crate exists. A module plus `deliver_later` on `jobs` finishes the reset flow.
2. **`flash`** (S, 5). Every form that redirects after a POST needs it. Sessions exist.
3. **`auth-screens`** (M, 5). Devise and allauth are the top auth choices. The crate has every flow, no page. Needs `mail` and `flash`.
4. **`compression`** (S, 4). gzip and brotli on HTML. Highest return per line in the HTTP layer.
5. **`conditional-get`** (S, 4). Dynamic `ETag` and 304. dp-swap GETs gain the most.
6. **`sanitize`** (S, 4). Safe user HTML. Unblocks `rich-text`, comments, Markdown.
7. ~~**`pagination`**~~ (S, 4). Built.
8. **`auth-lockout`** (S, 4). django-axes / Devise lockable beside the rate limiter.
9. ~~**`filters`**~~ (M, 4). Built.
10. ~~**`search`**~~ (M, 4). Built.
11. **`audit`** (M, 4). paper_trail / simple-history on top of `RowChange`, plus the admin history page.
12. **`roles`** (M, 4). Groups and permissions for staff and admin access.
13. **`errors`** (M, 4). HTML 404/500, a dev error page, a reporter with context.
14. **`flags`** (M, 4). Flipper / waffle with an admin panel.
15. **`admin-actions`** (M, 4). Bulk actions with a confirm page, then inlines.

Next after these: `debug-toolbar`, `storage-s3` with `attachments`, `impersonate`,
`notifications`, `ci-security`, `humanize`.

## Next.js and PocketBase scan


Date: 2026-11-09. Repo state: `main` at `25e908a`, read-only.

#### Method

- Inputs from the repo: `docs/module-catalog.md` (the module list and
  roadmap), `docs/audit/nextjs.md` (N1–N51), `docs/audit/pocketbase.md`
  (P1–P51), `docs/pocketbase-surface.md`, and the status table in
  `docs/framework-gaps.md` (G1–G14). Before I used an audit verdict, I
  checked it against the tree. The audits are from 2026-10-26, and some of
  their rows are now done: streaming, dp-post, cache tags, the
  `resource`/`admin` modules, `migrate make`, rate limiting, i18n and
  prefetch.
- Popularity: npm weekly downloads from `api.npmjs.org/downloads/point/last-week/<pkg>`,
  fetched 2026-11-09. They are rounded below. A download count measures
  installs. It does not measure demand for a server-side module. Use it only
  as a signal.
- PocketBase ecosystem: `github.com/benallfree/awesome-pocketbase`, PocketHost
  webhooks launch (`pockethost.io/blog/webhooks-launch`), the FTS plugin in
  `rodydavis/pocketbase-plugins`, discussion #2199 (search).
- Out of scope, with the reasons in PLAN.md §12 and the audits: dynamic
  collections, the JS hook VM, settings stored in the database, backups, the
  filter-string grammar, RSC and any client component runtime.

##### npm signals (weekly downloads)

| Package | /week | Package | /week |
|---|---|---|---|
| zod | 454M | sharp | 161M |
| react-hook-form | 62M | stripe | 33.7M |
| posthog-js | 25.0M | @prisma/client / drizzle-orm | 24.4M / 48.5M |
| better-auth | 18.7M | resend | 17.8M |
| @sentry/nextjs | 16.6M | @vercel/blob | 11.5M |
| @vercel/analytics | 10.8M | algoliasearch | 9.7M |
| svix (webhooks) | 9.7M | next-intl | 9.6M |
| next-auth | 9.4M | react-email | 8.0M |
| @trpc/server | 7.9M | nuqs | 7.8M |
| satori | 7.8M | @vercel/speed-insights | 7.1M |
| @clerk/nextjs | 3.7M | @t3-oss/env-nextjs | 3.7M |
| inngest | 3.4M | @vercel/og | 3.4M |
| @upstash/ratelimit | 3.1M | @next/mdx | 2.2M |
| flags (Vercel) | 2.0M | @vercel/edge-config | 1.4M |
| meilisearch | 1.0M | unleash-client | 1.0M |
| next-sitemap | 0.8M | @upstash/qstash | 0.8M |
| next-seo | 0.6M | launchdarkly-node-server-sdk | 0.6M |
| uploadthing | 0.26M | pocketbase (JS SDK) | 0.29M |
| @lemonsqueezy/lemonsqueezy.js | 0.17M | contentlayer | 0.03M (abandoned) |

#### Candidate table

Category follows `docs/module-map.md`. Value: 1 = low, 5 = high, for an
HTML-first Rust app.

| Candidate (key) | What it does | Next.js / PocketBase pieces | Covered? | Category | Effort | Value | Reason |
|---|---|---|---|---|---|---|---|
| `mail` | `Mailer` trait, SMTP or memory backend, settings, a dev outbox page | resend, react-email; PB mailer + SMTP settings (P43) | partly: `brocanth-mail` crate exists, no module, auth does not call it | Enhancer | S | 5 | Reset and verify flows are scaffolded and cannot deliver mail |
| `mail-templates` (part of `mail`) | rsx email templates with typed placeholders, text alternative, `/dev/mail` preview | react-email; PB templates `{APP_NAME}` `{TOKEN}` `{OTP}` | no | Renderer | M | 4 | Every auth flow needs one; rsx already renders HTML |
| `auth-oauth` | Scaffold provider login: GitHub, Google, Apple, OIDC; callback route; account linking | next-auth, better-auth, Clerk; PB OAuth2 (P20) | partly: `brocanth-auth/src/oauth/*` written; `auth` writes "No OAuth provider" | Routes | M | 5 | Most requested auth feature; code is done, only the scaffold is missing |
| `auth-otp` | Email one-time code / magic link login, with an enumeration defence | Auth.js email provider, Clerk; PB OTP (P21) | partly: `otp.rs` written, not scaffolded | Routes | S | 4 | Passwordless login is common; needs `mail` |
| `auth-mfa` | TOTP second factor, enrollment and challenge screens, recovery codes | Clerk, better-auth 2FA; PB MFA (P22) | partly: `mfa.rs`, `totp.rs` written, not scaffolded | Routes | M | 4 | The type system already prevents a skipped factor; expose it |
| `auth-impersonate` | Staff sign in as a user, with a banner and restricted actions | Clerk impersonation; PB impersonate (P24) | partly: `impersonation.rs`, not scaffolded | Enhancer | S | 3 | Support teams need it; small once the admin exists |
| `auth-alerts` | Email on login from a new device | PB auth alerts (P47) | partly: `alerts.rs`, not scaffolded | Enhancer | S | 2 | Needs `mail`; nice, not urgent |
| `superuser` | Operator accounts apart from app users; `createsuperuser` command; admin policy uses them | PB `_superusers` (P50), `superuser upsert` CLI (P39) | partly: admin defaults to `DenyAll`; no operator account or command | Enhancer | S | 4 | Without it every new admin needs a hand-written policy |
| `api-tokens` | Personal access tokens / API keys: hashed, scoped, revocable, `Authorization: Bearer` | Clerk M2M, Unkey, better-auth api-key plugin; PB API auth tokens | no | Middleware | M | 4 | `openapi` + `client` exist; machine callers have no credential |
| `orgs` | Organizations, members, roles, invitations; policy subject carries the org | Clerk Organizations, better-auth organization plugin | no | Enhancer | L | 4 | Every B2B SaaS needs tenancy; fits `brocanth-policy` `Exists` |
| `meta` | Per-page `<title>`, description, canonical, Open Graph, Twitter, JSON-LD, hreflang | Next `metadata`/`generateMetadata` (N31); next-seo | no: one `pages::TITLE` in `rsx.rs` LAYOUT_RSX | Renderer | M | 5 | A site with one title cannot ship; SEO is the HTML-first selling point |
| `og-image` | Render a PNG social card from a template at `/og/{page}.png`, cached | `@vercel/og`, satori, `opengraph-image.tsx` (N33, N34) | no | Routes | L | 3 | High SEO value, but a text-to-raster engine is a week of work |
| `icons` | Favicon set, apple-touch-icon, `manifest.webmanifest` from settings | Next `icon.*`, `apple-icon`, `manifest.ts` (N33) | partly: `public/favicon.ico` served verbatim | Routes | S | 3 | Cheap; every production site needs the set |
| `image` | `/img/{hash}?w=&fmt=` resize and re-encode for `assets/`, `Accept` negotiation, a `srcset` helper | `next/image` (N35), sharp | partly: `brocanth-storage/src/variants.rs` resizes uploads only | Routes | M | 4 | Largest page-weight win; decoder already chosen in storage |
| `storage-routes` (part of `storage`) | **Built.** Multipart upload route, download route, `?thumb=WxH`, `?token=` for protected files | PB file API (P26–P28); uploadthing, @vercel/blob | partly: `brocanth-input/src/multipart.rs`, storage tokens, variants; module writes no route | Routes | S | 5 | Every hard part is done; the HTTP edge is missing |
| `storage-s3` | S3-compatible backend (R2, MinIO), presigned URLs | PB S3 (P29), pocketbase-presigned-urls; @vercel/blob | no: declined in PLAN.md §4 (Postgres `bytea` instead) | Enhancer | M | 4 | Containers have no durable disk; revisit the §4 decision |
| `not-found` | `not-found.rsx` and `error.rsx` per directory, rendered in the layout | `not-found.tsx`, `forbidden()`, `unauthorized()` (N7) | no: a miss answers plain `Not Found` | Renderer | S | 4 | Day-one polish; reuses the layout walk in `html!` |
| `markdown` | `.md` pages in `src/pages/` with frontmatter and a layout, through `Slot::PagesRenderers` | `@next/mdx` (N50), Contentlayer | no | Renderer | M | 4 | Docs and blogs are what a server-rendered framework does best |
| `content` | Typed content collections: `content/posts/*.md`, frontmatter checked at build, list and detail pages | Contentlayer (abandoned), Astro content collections, next-mdx-remote | no | Enhancer | M | 4 | Contentlayer is dead and the need stays; compile-time checks fit brocanth |
| `feed` | RSS / Atom / JSON Feed from a content collection or a query | blog starters, next-sitemap peers | no | Routes | S | 3 | Small; pairs with `content` and `sitemap` |
| `search` | Postgres full-text search: `tsvector` column, migration, ranked query, a search page; Meilisearch adapter later | algoliasearch, meilisearch; PB FTS plugin, discussion #2199 | yes (`search`; no Meilisearch adapter yet) | Routes | M | 4 | Done: Postgres FTS, a site index and /search |
| `flags` | Feature flags in Postgres: boolean and percentage rollouts, per user or org, admin panel, `ctx.flag()` | flags / `@vercel/flags`, Edge Config, LaunchDarkly, Unleash | no | Enhancer | M | 4 | Safe deploys; server-side flags fit HTML-first, no client SDK |
| `webhooks-out` | **Built**, in `brocanth-webhooks`. Signed outbound webhooks from `RowChange`: endpoints table, HMAC (Standard Webhooks), retries via jobs, delivery log | svix; PB webhooks plugin, PocketHost webhooks | no (queue outbox in `brocanth-queue-postgres` is the base) | Enhancer | M | 4 | Integrations ask for it first; events + jobs + outbox already exist |
| `webhooks-in` | **Built**, in `brocanth-webhooks`, as a hook before the CSRF check. A route type for inbound webhooks: signature check (Stripe, GitHub, Standard Webhooks), replay window, idempotency key, CSRF exempt | Next route handlers for Stripe/Clerk webhooks | partly: `middleware-csrf` has an exempt list only | Middleware | S | 4 | Every payment or SaaS integration needs it; easy to get wrong |
| `billing` | Stripe Checkout + Customer Portal + subscription table synced from webhooks; plan gates in policy | stripe, Lemon Squeezy; PocketBase+Stripe builds | no | External | L | 3 | High demand, but vendor coupling and an HTTP client; build on `webhooks-in` |
| `audit-log` | Record who changed what: `RowChange` + subject into a table, admin panel, retention | PB logs (P31); Next has none | partly: `brocanth-events` seam, auth audit notes | Enhancer | M | 4 | Compliance asks for it; the event seam makes it cheap |
| `logs-store` | Request log in a table (unlogged or separate DB), `/admin/logs` with stats, retention | PB logs API + dashboard (P3, P31) | no: `logging` and `observability-logs` print only | Routes | M | 3 | Small teams without a log stack want it; OTLP covers bigger teams |
| `error-tracking` | Report panics and 5xx with request context to Sentry or GlitchTip (Sentry protocol) | `@sentry/nextjs`, `instrumentation.ts` `onRequestError` (N39) | partly: `ErrorHook`, `observability-tracing` (OTLP) | External | S | 4 | 16.6M/week shows demand; `ErrorHook` is the seam |
| `crons` | Declare recurring jobs in one file; admin panel with last run and "run now" | Vercel Cron; PB cron + `/api/crons` (P34) | partly: `Queue::recurring` in `brocanth-jobs`, no list or run-now | Routes | S | 3 | Jobs exist; the operator view is missing |
| `web-vitals` (part of `beacon`) | Collect LCP/INP/CLS with a few lines of script into `analytics` | `@vercel/speed-insights` | partly: `beacon` reports page views only | Middleware | S | 2 | Useful; dp-swap pages are already light |
| `product-analytics` | Server-side event capture to PostHog / Plausible, no client SDK | posthog-js, `@vercel/analytics` | partly: `analytics`, `beacon`, `gtag` | External | S | 2 | Mostly covered by `analytics` |
| `i18n-routing` (part of `i18n`) | Locale prefix in the path (`/fr/...`), redirect from `Accept-Language`, hreflang in `meta` and `sitemap` | next-intl, Next i18n recipe (N37) | partly: `i18n` reads query, cookie, header; no path prefix | Middleware | S | 4 | SEO needs one URL per locale; a query or cookie locale is not indexed |
| `redirects` | `[redirects]`, `[rewrites]`, `[headers]` tables in config, checked at start | `next.config` `redirects()`/`rewrites()`/`headers()` (N45) | partly: `CanonicalHost`, `Cors` only | Middleware | S | 3 | Site moves and legacy URLs; cheap |
| `draft-mode` | Signed preview cookie that bypasses `RouteCache` and shows unpublished rows | Next draft mode (N40) | no | Middleware | S | 2 | Needs a CMS-style app first; cache tags make it cheap later |
| `isr` / page cache | Cache rendered pages with revalidation and tag invalidation | Next ISR, `revalidateTag` (N19–N25) | yes: `cache` `RouteCache`, tags, `TagInvalidator` (G8) | Enhancer | — | — | Done |
| `rate-limit` | Per address, session or user | `@upstash/ratelimit`; PB rate limits (P33) | yes: `middleware-rate-limit` | Middleware | — | — | Done |
| `env` / typed settings | Typed, validated environment and secrets | `@t3-oss/env-nextjs`, `astro:env` | yes: `config`, module flags, redaction | Enhancer | — | — | Done |
| forms / server actions | Typed form body, validation, 422 re-render | zod, react-hook-form, next-safe-action, server actions (N16, N17) | yes: `input`, `validation`, dp-post (G2) | Middleware | — | — | Done |
| background work | Durable jobs, retries, cron | inngest, QStash, Vercel Cron | yes: `jobs` | Enhancer | — | — | Done (`crons` above adds the panel) |
| realtime | Live updates | PB realtime (P17, P18) | yes: `realtime-sse`, `realtime-websocket`, model publishes (G4) | Routes | — | — | Done |
| typed API / RPC | Typed calls from outside | tRPC; PB SDKs (P40) | partly: `openapi`, `client` | Routes | — | — | Covered enough for HTML-first |
| `typegen-ts` | TypeScript types / a fetch client from `openapi.json` for a separate frontend | PB TypeScript generator, pocketbase-gogen | partly: `openapi` | External | S | 2 | Not the HTML-first audience |
| `batch` | `POST /api/batch`: many resource writes in one transaction | PB batch API (P16) | no | Routes | M | 2 | API-first feature; forms do not need it |
| `tls` | Automatic Let's Encrypt (ACME) in the binary | PB `serve` with autocert (P1) | no | Middleware | M | 3 | Single-VPS deploys love it; most deploys sit behind a proxy |
| `captcha` | Turnstile / hCaptcha / Altcha check on public forms | Vercel BotID, common in Next auth forms | no | Middleware | S | 3 | Sign-up spam is day-two pain; pairs with rate limiting |
| `fonts` | Download and subset Google fonts at build, emit `@font-face` and preload | `next/font` (N36) | partly: `assets` serves and hashes fonts | External | M | 2 | Privacy and CLS; a hand copy into `assets/` works today |
| `backups` | Scheduled dumps | PB backups (P30) | out of scope: PLAN.md §12 | — | — | — | Declined |

#### Top 15 not yet covered (ranked)

Ranked by value per effort, then by what each unblocks. "Partly" counts as
"not yet covered" when no generated project can reach the feature.

1. **`mail`** (S, 5). The crate exists. A module plus the auth wiring finishes
   the reset and verify flows. It also unblocks `auth-otp`, `auth-alerts`
   and invitations.
2. **`meta`** (M, 5). Per-page title, description, OG and canonical. Today
   every page shares one title. It is also the base for `og-image`,
   `i18n-routing` hreflang and `fonts` preload.
3. **`storage-routes`** (S, 5). Upload, download, thumbnails and protected
   tokens. The crate and the multipart decoder are done. Only the routes are
   missing.
4. **`auth-oauth`** (M, 5). The OAuth2 code is written and tested. The
   scaffold, the provider settings and the callback route are missing.
   better-auth + next-auth + Clerk together have about 32M downloads a week.
5. **`not-found`** (S, 4). `not-found.rsx`/`error.rsx` in the layout chain.
   It is a small change, and every user sees the result.
6. **`webhooks-in`** (S, 4). A verified inbound webhook route. It is the
   base for `billing` and for any SaaS integration.
7. **`superuser`** (S, 4). Operator accounts and `createsuperuser`. With it,
   the `admin` module works without a hand-written policy.
8. **`i18n-routing`** (S, 4). Locale path prefixes and hreflang. next-intl
   has 9.6M downloads a week. The current locale from a query or cookie is
   not indexable.
9. ~~**`search`**~~ (M, 4). Built: Postgres FTS with a migration and a
   search page.
10. **`webhooks-out`** (M, 4). Signed, retried deliveries from `RowChange`
    through jobs and the outbox. svix has 9.7M downloads a week, and
    PocketHost shipped webhooks as a headline feature.
11. **`flags`** (M, 4). Server-side feature flags in Postgres with an admin
    panel. The client needs no SDK.
12. **`error-tracking`** (S, 4). Sentry-protocol reports from `ErrorHook`.
    `@sentry/nextjs` has 16.6M downloads a week.
13. **`auth-otp` + `auth-mfa`** (S+M, 4). Passwordless login and TOTP. The
    code is written. They need `mail` first.
14. **`markdown` + `content`** (M, 4). `.md` pages and typed content
    collections. Contentlayer is abandoned and the need stays. Add `feed` at
    the same time.
15. **`image`** (M, 4). A resize and format endpoint for `assets/`, with
    `srcset`. It reuses the storage decoder and its pixel cap.

Next in line: `audit-log`, `api-tokens`, `orgs` (L), `storage-s3` (needs the
PLAN.md §4 decision to change), `crons` panel, `redirects`, `icons`,
`captcha`, `billing` (L, after `webhooks-in`), `og-image` (L).

#### Sources

- npm downloads API, `https://api.npmjs.org/downloads/point/last-week/<pkg>` (2026-11-09).
- [awesome-pocketbase](https://github.com/benallfree/awesome-pocketbase): pocketbase-otp, pocketpages, presigned URLs, webhooks, TS generator, pb-ext, PocketBase+Stripe and +Lemonsqueezy builds.
- [rodydavis/pocketbase-plugins](https://awesome.ecosyste.ms/projects/github.com%2Frodydavis%2Fpocketbase-plugins): FTS and vector plugins.
- [PocketBase discussion #2199, "Thoughts about Simple Search"](https://github.com/pocketbase/pocketbase/discussions/2199).
- [PocketHost webhooks launch](https://pockethost.io/blog/webhooks-launch).
- Repo: `docs/module-catalog.md`, `docs/audit/nextjs.md`, `docs/audit/pocketbase.md`, `docs/pocketbase-surface.md`, `docs/framework-gaps.md` (G-table), `PLAN.md` §12, `crates/brocanth-mail/src/lib.rs`, `crates/brocanth-input/src/multipart.rs`, `crates/brocanth-cli/src/modules/rsx.rs` (single `TITLE`), `docs/modules/{auth,storage,i18n}.md`.
