# Django vs brocanth

**Django checked: 6.1**, the current release (docs at `docs.djangoproject.com/en/6.1`;
5.2 is the live LTS). The inventory below extends
`docs/framework-gaps.md` §1.5, which stopped at D28 and at Django 5.2. D1–D28 keep
their numbers and their wording. D29–D88 are new, and they include what 6.0 and
6.1 added: the CSP framework, template partials, the background-task framework and
the modern email API (6.0); fetch modes, database-level foreign-key deletes, the
`MAILERS` setting and the UUID functions (6.1).

**brocanth read at `5017473`**, 2026-10-26. Every verdict comes from the source at
that commit. §2.2 of `framework-gaps.md` is stale and was not used: it says there
is no HTTP test client (`crates/brocanth-test` is one) and that auth is unwired
(`brocanth_auth::scaffold::Auth` is in `crates/brocanth-cli/src/modules/mod.rs`
`ALL`). Nothing was run; no project was generated.

**How a verdict is decided.** A capability reaches a user through `brocanth create`
or `brocanth add`. Code that exists and that no generated project can select is
`partial`, with the reason *written, not reachable*. The menu is the written list in
`crates/brocanth-cli/src/modules/mod.rs`: logging, config, sessions, model, cache,
jobs, auth, policy, storage, entry, client, middleware, openapi, validation, records
(blocked), realtime, realtime-sse (blocked), realtime-websocket (blocked), commands,
rsx, jt, tailwind, htmx. `brocanth-admin`, `brocanth-admin-model` and
`brocanth-test` have no module and no scaffold, and no module turns on the `admin`
feature that the cache, jobs, config, auth, storage, realtime and openapi crates
each keep optional.

| ID | Capability | Django | brocanth | Evidence |
|---|---|---|---|---|
| D1 | `models.Model` with field classes, `Meta`, a per-model manager | yes | yes | `crates/brocanth-model/src/lib.rs` (SeaORM 2.0.4, Postgres by default); `model` is on the menu, `crates/brocanth-cli/src/modules/model.rs` |
| D2 | `makemigrations` from a model diff, `migrate`, `sqlmigrate`, `showmigrations` | yes | partial | `crates/brocanth-model/src/lib.rs:149` re-exports `sea_orm_migration`, so `up`/`down`/status exist; `crates/brocanth-model/src/schema_sync.rs` is only a seam — `PlanOnly` reports and applies nothing, and no CLI diffs a model against a database |
| D3 | `RunPython` / `RunSQL` data migrations with `reverse_code` | yes | yes | `sea_orm_migration`'s `MigrationTrait` has `up`/`down` and runs arbitrary statements; `crates/brocanth-model/src/lib.rs:149` |
| D4 | Lazy, chainable, sliceable query set; `select_related` / `prefetch_related` | yes | yes | SeaORM `Select` through `crates/brocanth-model/src/prelude.rs`; `crates/brocanth-model/src/eager.rs` fixes the limit-over-joined-rows bug, `src/probe.rs` detects N+1 |
| D5 | `full_clean()`, per-field validators, `clean_<field>()`, a field→error map | yes | yes | `crates/brocanth-model/src/validation.rs`, `src/validators.rs` (presence, length, format, numericality, inclusion, uniqueness), `src/errors.rs` |
| D6 | `ModelForm`: a form derived from a model, `fields`/`exclude`, widgets, `is_valid()` | yes | partial | `crates/brocanth-admin/src/form.rs` builds a `Form` from a `ModelDescriptor` with `exclude`/`readonly`; written, not reachable — no admin module in `modules/mod.rs` `ALL` |
| D7 | A form rendered by the framework, re-rendered with values and per-field errors | yes | partial | `crates/brocanth-admin/src/templates`, `src/view.rs` `FormCtx`; same unreachability |
| D8 | `ModelAdmin`: `list_display`, `list_filter`, `search_fields`, `ordering`, `readonly_fields` | yes | partial | `crates/brocanth-admin/src/config.rs` has `list_columns`, `searchable`, `sortable`, `readonly`, `per_page`, `widget`, `foreign_key`; **no `list_filter`**, and the crate is not reachable from `create` |
| D9 | Admin auto-discovery (`admin.site.register`, `apps.py`) | yes | partial | `crates/brocanth-admin/src/discovery.rs` reads a registry; `crates/brocanth-admin-model/src/source.rs` adapts `ModelRegistry`; not reachable |
| D10 | `contrib.auth`: user, pluggable hashing, `authenticate`, `login`, `logout`, `@login_required` | yes | partial | `crates/brocanth-auth` is on the menu (`src/scaffold.rs`): registration, login, reset, verification, session rotation. The only `AuthStore` in the workspace is a `HashMap` (`src/memory.rs`), so accounts do not survive a restart |
| D11 | Sessions through a pluggable backend: database, cache, signed cookie, file | yes | partial | `crates/brocanth-cli/src/modules/sessions.rs` writes a signed-cookie session and mounts it as a layer; no database, cache or file backend |
| D12 | CSRF: a per-session token, `{% csrf_token %}`, middleware that rejects a POST without it | yes | no | `csrf` appears only in `crates/brocanth-auth/src/oauth/mod.rs`, for the OAuth `state` parameter. No token, no form helper, no hook |
| D13 | Template language: `{% extends %}`, `{% block %}`, `{% include %}`, filters, auto-escaping | yes | yes | rsx (git dep, `crates/brocanth-admin/Cargo.toml:30`) compiles components into the binary; `crates/brocanth-pages/src/lib.rs` stacks `layout.rsx` by directory, which is inheritance read off the tree instead of named per page |
| D14 | Class-based views: `ListView`, `DetailView`, `CreateView`, `UpdateView`, `DeleteView`, `FormView` | yes | partial | `crates/brocanth-resource` builds five typed CRUD routes but its module is **blocked**: `crates/brocanth-resource/src/scaffold.rs:93`, "a generated app dispatches src/pages; these routes arrive as a RouteSource" |
| D15 | URL routing: `path()`, `include()`, named routes, `reverse()`, `{% url %}` | yes | yes | `crates/brocanth-pages/src/table.rs`, `crates/brocanth/src/routing_codegen.rs` — a route helper is generated and a wrong one is a compile error, which Django cannot do |
| D16 | Middleware as a list of callables, each wrapping the next | yes | yes | `crates/brocanth/src/routing_middleware.rs` (pattern-scoped), `crates/brocanth/src/plugin_hooks.rs`; five hooks ship in `crates/brocanth-middleware`, on the menu |
| D17 | The app registry; `startapp` scaffolds an app | yes | yes | `crates/brocanth-scaffold/src/lib.rs` (`Module`, `Manifest`, `Slot`), `brocanth create` / `brocanth add` in `crates/brocanth-cli/src/commands/` |
| D18 | `manage.py` and custom commands under `management/commands/` | yes | yes | `crates/brocanth-command/src/lib.rs` — `#[cmd]` registers through a `linkme` slice at link time; `crates/brocanth-cli/src/main.rs` delegates any unknown subcommand to the project binary |
| D19 | Cache framework: `cache.get/set` plus memcached, Redis, database and local-memory backends | yes | partial | `crates/brocanth-cache/src/postgres.rs` (an `UNLOGGED` table) and `src/fetch.rs` (read-through, single-flight). One backend; no Redis, no memcached, no process-local tier |
| D20 | Signals: `pre_save`, `post_save`, `pre_delete`, `post_delete`, `m2m_changed`, custom | yes | partial | `crates/brocanth-events/src/lib.rs` carries `RowChange`/`ChangeSink`, and `crates/brocanth-realtime/src/notify.rs` subscribes — but `crates/brocanth-model/Cargo.toml` does not depend on `brocanth-events`, so the model layer still publishes nothing. Only `crates/brocanth-resource/src/flow.rs` emits |
| D21 | `settings.py`, `INSTALLED_APPS`, a settings module per environment | yes | yes | `crates/brocanth-config/src/config.rs` layers four sources and rejects an unclaimed section; `config` is on the menu |
| D22 | `collectstatic`, `ManifestStaticFilesStorage`, `{% static %}` | yes | yes | `crates/brocanth-assets/src/lib.rs` fingerprints `assets/` at compile time, serves `public/` verbatim, and `assets::url()` is a compile error on a missing name |
| D23 | `check`, and `check --deploy` refusing to start on a misconfiguration | yes | partial | `Plugin::configure` returning `PluginError` aborts the boot (`crates/brocanth/src/plugin_api.rs`); there is no check registry, no tags and no `check` command in `crates/brocanth-cli/src/main.rs` |
| D24 | i18n: `gettext`, `.po` files, `{% trans %}`, `LocaleMiddleware`, per-language URLs | yes | no | nothing in the tree; no `fluent`, `gettext` or `icu` in any `crates/*/Cargo.toml` |
| D25 | Test client plus `TestCase` wrapping each test in a transaction | yes | partial | `crates/brocanth-test/src/client.rs` drives a built `App` through hooks, router and handler, and `src/html.rs` asserts on elements; `crates/brocanth-model/src/testing.rs` is the never-commit harness. Not reachable: the scaffold writes no `[dev-dependencies]` at all (`crates/brocanth-cli/src/commands/create.rs`) |
| D26 | `shell` and `dbshell` | yes | no | nothing in the tree |
| D27 | Fixtures: `loaddata` / `dumpdata` | yes | no | `crates/brocanth-admin/src/fixture.rs` is an in-memory `AdminData` test double, not a loader |
| D28 | `contrib.postgres`: array, range and `jsonb` fields, full-text search, trigram | yes | partial | SeaORM's Postgres types give json and array columns; no full-text search, no trigram, no range type helper anywhere in `crates/` |
| D29 | `F()` expressions and database functions (`Coalesce`, `Cast`, `Trunc`, window functions) | yes | yes | `sea_orm` and its `sea_query` `Expr`/`Func` reach a project through `crates/brocanth-model/src/lib.rs:147` and `src/prelude.rs` |
| D30 | `Q()` objects, `__` lookup traversal, custom lookups and transforms | yes | partial | SeaORM `Condition::any/all` composes booleans, and `crates/brocanth-policy/src/pred.rs` + `src/sql.rs` are a typed predicate algebra that compiles to a `WHERE`; there is no string-path traversal across relations, by decision (`crates/brocanth-policy/src/lib.rs` argues it) |
| D31 | Aggregation: `annotate` / `aggregate`, `Count`/`Sum`/`Avg`, `Subquery`, `OuterRef`, `Exists` | yes | yes | SeaORM `QuerySelect::column_as`/`group_by`/`having` and sea-query subqueries, re-exported at `crates/brocanth-model/src/lib.rs:147`; brocanth adds nothing of its own |
| D32 | Transactions: `atomic` blocks, savepoints, `on_commit`, `durable` | yes | partial | `TransactionTrait` is in `crates/brocanth-model/src/prelude.rs` and `src/bulk.rs` writes a set in one transaction; no `on_commit` hook, which is what a transactional outbox would need |
| D33 | `select_for_update` with `nowait` / `skip_locked` / `of` | yes | yes | SeaORM's `lock_exclusive`/`lock_with_behavior` on `Select`, via `crates/brocanth-model/src/lib.rs:147`; `crates/brocanth-jobs/src/store` already leans on row locking for the queue |
| D34 | `Meta.constraints`: `UniqueConstraint`, `CheckConstraint`, conditional and expression constraints | yes | partial | expressible in a hand-written sea-query migration; nothing declares a constraint beside the model, and `uniqueness` in `crates/brocanth-model/src/validators.rs:370` is an application-level `SELECT`, not a database constraint |
| D35 | `Meta.indexes`, functional and partial indexes | yes | partial | same route — sea-query index builders inside a hand-written migration; no declaration on the entity |
| D36 | Custom managers, `QuerySet` subclasses, `Manager.from_queryset`, related managers | yes | partial | the Rust shape is an extension trait, and `crates/brocanth-model/src/eager.rs:90` (`EagerExt`) is the worked example; nothing ships a named-query vocabulary |
| D37 | Raw SQL: `Manager.raw()`, a cursor, `RawSQL` | yes | yes | `ConnectionTrait` is in `crates/brocanth-model/src/prelude.rs`; `sea_orm::Statement` carries bound parameters |
| D38 | Bulk writes: `bulk_create` (with upsert), `bulk_update`, queryset `update()` / `delete()` | yes | yes | `crates/brocanth-model/src/bulk.rs` — and further than Django: hooks fan out by default and a bypass must be named `HooksBypassed` |
| D39 | Model inheritance (abstract, multi-table, proxy) and `contenttypes` generic relations | yes | no | nothing in the tree; SeaORM has no inheritance and there is no content-type table or generic foreign key |
| D40 | Field breadth: JSON, UUID, Duration, File/Image, Decimal, `choices`, `GeneratedField`, `CompositePrimaryKey` | yes | partial | SeaORM covers the scalar and JSON half and `crates/brocanth-admin/src/model.rs` `ColumnKind` enumerates what the admin can render; there is no file/image column type — `crates/brocanth-storage` stores bytes and knows nothing about a column |
| D41 | Fetch modes `FETCH_ONE` / `FETCH_PEERS` / `FETCH_RAISE` (6.1) | yes | partial | `crates/brocanth-model/src/deferred.rs` (`Deferred`, `NotLoaded`) is `FETCH_RAISE` and `src/eager.rs` is `FETCH_PEERS`, but per value rather than as a mode set on a query set |
| D42 | Database-level foreign-key deletes: `DB_CASCADE`, `DB_SET_NULL`, `DB_SET_DEFAULT` (6.1) | yes | partial | `ON DELETE` is writable in a sea-query migration; no option on the relation, and no cascade on the Rust side |
| D43 | Migration operations: `squashmigrations`, `--check`, `--merge`, `--fake`, dependencies, atomic migrations | yes | partial | `sea_orm_migration` gives ordered up/down, status, fresh and refresh; no squash, no merge, no `--check`, no dependency graph across apps |
| D44 | Several databases, `DATABASE_ROUTERS`, `QuerySet.using()` | yes | partial | the plugin system is multi-instance (`Lifecycle::instance`, `instance_names`, `crates/brocanth/src/plugin_api.rs`), so two model plugins can coexist; the generated plugin holds one `Db` and there is no router |
| D45 | Persistent connections and the connection pool (`CONN_MAX_AGE`, pool options) | yes | partial | SeaORM's `DatabaseConnection` is pooled; the generated config carries only `url` (`crates/brocanth-cli/src/modules/model.rs`), so no pool size, timeout or lifetime is tunable |
| D46 | Async ORM: `aget`, `acreate`, `afirst`, async iteration | yes | yes | SeaORM is async-first and `crates/brocanth-model/src/lib.rs:143` re-exports `async_trait` for the hooks |
| D47 | Async views and middleware, ASGI | yes | yes | `crates/brocanth/src/plugin_routes.rs:137` `AsyncHandler` plus `get_async`/`post_async`/…; `docs/async-handlers.md` has the cost measurement |
| D48 | Plain `Form`: field classes, widgets, `cleaned_data`, `clean_<field>`, `add_error`, `initial`, prefixes | yes | partial | `crates/brocanth-input/src/lib.rs` turns a body into a typed struct and `crates/brocanth-schema` validates it (`entry` and `validation` are both on the menu); the HTML half lives only in the unreachable admin, and there is no re-render-with-initial-values path outside it |
| D49 | Formsets, model formsets, inline formsets, the management form | yes | no | nothing in the tree; `crates/brocanth-admin/src/form.rs` is one form over one row |
| D50 | Form and widget rendering templates, pluggable renderers, field groups | yes | partial | `crates/brocanth-admin/src/view.rs` `Views` replaces a whole screen with your own rsx component; a single widget's template cannot be replaced, and `widget_for` is a fixed match (`src/form.rs:85`) |
| D51 | File uploads: `request.FILES`, `FileField`, storage backends, `MEDIA_URL` | yes | partial | `crates/brocanth-storage` is on the menu with a local-disk and a Postgres backend; **no multipart parser exists** (`crates/brocanth-admin/src/plugin.rs:243` says a multipart body arrives as an empty submission) and the crate has no routes by design (`crates/brocanth-storage/src/lib.rs`) |
| D52 | Generic view mixins: `LoginRequiredMixin`, `PermissionRequiredMixin`, `SingleObjectMixin`, `get_context_data` | yes | no | handlers are functions over a `Ctx`; `crates/brocanth-resource` has a typed policy per route but no mixin chain, and nothing wraps a handler in a login check |
| D53 | Admin inlines (`StackedInline`, `TabularInline`) | yes | no | nothing in `crates/brocanth-admin/src/`; the four screens are one row at a time |
| D54 | Admin actions, including an intermediate confirmation page | yes | no | `crates/brocanth-admin/src/config.rs` has no action list; only `no_create`/`no_update`/`no_delete` |
| D55 | `list_editable`, `date_hierarchy`, `raw_id_fields`, `autocomplete_fields`, `list_select_related` | yes | no | `crates/brocanth-admin/src/form.rs` has a foreign-key picker that renders a label instead of an id, and nothing else from this row. An autocomplete widget also needs client JS, which constraint 2 refuses |
| D56 | Admin permissions per action and per object; staff-only login; several admin sites | yes | partial | `crates/brocanth-admin/src/policy.rs` — `Action`, `Principal`, `Request::on(action, model, record)`, and `DenyAll` is the default, which is stronger than Django's default; there is no login screen of the admin's own and no second site |
| D57 | Admin history: `LogEntry` and the object-history page | yes | no | nothing in the tree |
| D58 | Groups, `Permission` rows, default per-model permissions, `user.has_perm` | yes | partial | `crates/brocanth-policy` (on the menu) evaluates typed rules and compiles them to SQL, which covers row rules; `crates/brocanth-auth` has no group, no permission and no role — `grep -r "permission" crates/brocanth-auth/src` finds nothing |
| D59 | Pluggable `AUTHENTICATION_BACKENDS` and a custom user model | yes | partial | `AuthStore` is the seam and `crates/brocanth-auth/src/providers` makes an OAuth/OIDC provider a struct literal rather than a module; the only store implementation is `src/memory.rs`, and the account shape is the crate's |
| D60 | Pluggable password hashers and `AUTH_PASSWORD_VALIDATORS` | yes | partial | `crates/brocanth-auth/src/password.rs` is argon2id with no choice offered, which is a defensible refusal; cost is tunable (`PasswordCost::custom`). There is no password-strength validator — no minimum length, no common-password list |
| D61 | Built-in auth views: login, logout, password change and reset, with templates and email | yes | partial | `crates/brocanth-auth/src/accounts.rs` issues the reset and verification tokens and documents the enumeration defence; no routes, no templates, and nothing can send the link (see D69) |
| D62 | Per-site and per-view caching, template fragment caching, `Vary` headers, conditional GET | yes | partial | `crates/brocanth-cache/src/fetch.rs` is the low-level half and is better than Django's (single-flight); no `@cache_page` equivalent, no fragment tag, and `Vary` appears only in `crates/brocanth-middleware/src/cors.rs`. No ETag or Last-Modified hook |
| D63 | Session expiry, `cycle_key`, and the session backend choice | yes | partial | `crates/brocanth-auth/src/session.rs` rotates the identifier on every privilege change, which is the fixation defence; the cookie module writes name and `secure` only (`crates/brocanth-cli/src/modules/sessions.rs`) |
| D64 | Signal breadth: `request_started/finished`, `m2m_changed`, `pre/post_migrate`, `user_logged_in`, `dispatch_uid` | yes | partial | the lifecycle half is plugin hooks (`crates/brocanth/src/plugin_api.rs`: `configure`, `register`, `after_config`, `before_start`, `start`, `begin_shutdown`, `shutdown`); the data half is `RowChange` only — create, update, delete on one row (`crates/brocanth-events/src/lib.rs`) |
| D65 | Custom tags and filters, inclusion tags, context processors, a pluggable template engine | yes | yes | a component is a Rust function, so a tag or filter is a function; a plugin publishes into the context and a handler reads it (commit `c35ef4c`, `crates/brocanth/src/plugin_context.rs`); the engine is pluggable in fact — `crates/brocanth-cli/src/modules/jt.rs` is a second renderer attached through the same seam |
| D66 | Template partials: `{% partialdef %}` / `{% partial %}` (6.0) | yes | partial | rsx components in one file are the same shape, and dp-swap re-renders by layout depth (`crates/brocanth-cli/vendor/dpswap.min.js`); there is no named-fragment address a handler can render on its own |
| D67 | `makemessages` / `compilemessages`, `{% translate %}`, `{% blocktranslate %}`, `i18n_patterns`, lazy strings | yes | no | nothing in the tree |
| D68 | `USE_TZ`, `activate()`, `localtime`, per-locale number and date formats | yes | no | `chrono` is used for job scheduling and cache clocks only (`crates/brocanth-jobs/src/clock.rs`, `crates/brocanth-cache/src/clock.rs`); there is no request-scoped time zone and no locale-aware format |
| D69 | Email: `send_mail`, `EmailMessage`, SMTP / console / file / locmem backends, `MAILERS` (6.1) | yes | no | no `lettre` and no SMTP client in any `crates/*/Cargo.toml`. `crates/brocanth-auth/src/accounts.rs` returns *what to mail* and leaves the sending to an application that has no mailer |
| D70 | Background tasks: `@task`, `.enqueue()`, the `TASKS` backends (6.0) | yes | yes | `crates/brocanth-jobs` is on the menu and goes further than 6.0's framework: retries with backoff (`src/backoff.rs`), cron (`src/cron.rs`), a dead letter, a panic hook, and an admin panel |
| D71 | `SecurityMiddleware`: SSL redirect, HSTS, `nosniff`, referrer policy, cross-origin-opener-policy | yes | no | no match for `hsts`, `strict-transport`, `nosniff` or `referrer-policy` anywhere in `crates/`. `crates/brocanth-middleware` ships proxy headers, method override, canonical host, content types and CORS |
| D72 | Clickjacking protection: `X-Frame-Options` middleware and the exempt decorator | yes | no | no match for `x-frame-options` in `crates/` |
| D73 | CSP framework: `ContentSecurityPolicyMiddleware`, `SECURE_CSP`, nonces, `csp_nonce_attr` (6.0/6.1) | yes | no | no match for `content-security-policy` in `crates/` |
| D74 | `ALLOWED_HOSTS` and `django.core.signing` | yes | partial | `crates/brocanth-middleware/src/canonical_host.rs` redirects to one host, which is not the same as refusing an unknown `Host`; signing exists for sessions (`crates/brocanth-cli/src/modules/sessions.rs`) and for auth keys (`SigningKey::parse`, 32 bytes, `crates/brocanth-auth/src/lib.rs`), but there is no general signer |
| D75 | Pagination: `Paginator`, `Page`, `AsyncPaginator` (6.0) | yes | partial | two paginators exist and neither is a general one: `crates/brocanth-admin/src/paging.rs` for admin screens and `crates/brocanth-resource/src/page.rs`, whose header is the best argument in the tree for why a page must carry the policy's condition into the `WHERE` and count the same set |
| D76 | Serialization: `django.core.serializers` (json/xml/yaml), `model_to_dict` | yes | partial | `crates/brocanth-resource/src/json.rs` writes the envelope and the application writes the row (`Resource::encode_row`); `crates/brocanth-openapi/src/document.rs` describes the shapes. No model→document serializer, in either direction |
| D77 | GeoDjango: `contrib.gis`, geometry fields, spatial lookups, a spatial admin | yes | n/a (out of scope) | nothing in the tree, and nothing plans it; Postgres is the base, so PostGIS stays reachable through D37 |
| D78 | `contrib.sites`, `sitemaps`, syndication feeds, `redirects`, `flatpages`, `humanize` | yes | no | nothing in the tree. A sitemap and a feed are two handlers over the route table (`crates/brocanth-pages/src/table.rs`), which is why this row is small rather than absent in principle |
| D79 | The messages framework: flash messages across a redirect, with levels and tags | yes | no | nothing in the tree; the session plugin stores no flash slot (`crates/brocanth-cli/src/modules/sessions.rs`) |
| D80 | `staticfiles`: finders, `STORAGES`, dev serving, hashed names | yes | yes | `crates/brocanth-assets/src/lib.rs` — embedded at compile time, so there is no finder to configure and no file to go missing after `cargo build`; `serve_asset` is wired into the generated dispatch (`crates/brocanth-cli/src/commands/create.rs:1783`) |
| D81 | `runserver` with autoreload | yes | partial | the scaffold writes `src/commands/serve.rs` and `app::serve` (`crates/brocanth-cli/src/commands/create.rs:856`, `:1748`); there is no file watcher and no reload — that is `cargo watch`'s job today |
| D82 | URL extras: custom path converters, namespaces, `reverse_lazy`, `APPEND_SLASH` | yes | yes | `crates/brocanth/src/routing_params.rs` and `routing_path.rs` type and parse parameters, `routing_codegen.rs` generates the helpers, and the module path is the namespace. No trailing-slash redirect, which is the one piece missing |
| D83 | `LOGGING`: handlers, formatters, loggers, `AdminEmailHandler` | yes | partial | `crates/brocanth-cli/src/modules/logging.rs` writes the logger into the project in full, so it is readable and changeable; there is no handler/formatter vocabulary and no error mail (see D69) |
| D84 | `HttpResponse` family: `JsonResponse`, `FileResponse`, `StreamingHttpResponse`, `get_object_or_404`, `redirect` | yes | partial | `crates/brocanth/src/plugin_body.rs` has `Body::Buffered` and `Body::Streaming` with the hook contract split around it (`src/plugin_hooks.rs:1117`); the named shortcuts do not exist — a handler returns `impl IntoResponse` |
| D85 | Test tooling beyond the client: `assertNumQueries`, `override_settings`, `RequestFactory`, a parallel runner, `LiveServerTestCase` | yes | partial | `crates/brocanth-model/src/probe.rs` is `assertNumQueries` and more (it names the suspect), `crates/brocanth-test/src/request.rs` is the request factory, and `crates/brocanth-model/src/testing.rs:155` takes a transaction-scoped advisory lock so a suite can run in parallel; no settings override and no live-server case |
| D86 | `inspectdb`: generate models from an existing database | yes | no | nothing in the tree; `crates/brocanth-model/src/schema_sync.rs` reads entities, never the database |
| D87 | `contrib.admindocs` | yes | n/a (out of scope) | rustdoc over a workspace whose every crate header is an argument is the equivalent, and it is already built |
| D88 | WSGI / ASGI deployment entry points behind gunicorn or uvicorn | yes | n/a (out of scope) | `crates/brocanth-server/src/lib.rs` binds the socket itself; a single binary has no application-server protocol to speak |

**Row counts.** 88 rows: **22 yes**, **43 partial**, **20 no**, **3 n/a (out of scope)**.

Three themes under the 43 `partial`s, and they are different problems. *Written, not
reachable*: the admin and everything hanging off it (D6–D9, D50, D56), the test
client (D25, D85), the records API (D14). *Present through SeaORM, not through
brocanth*: D32, D34, D35, D36, D45. *Half the loop*: a reset token with no mailer
(D61, D69), a storage backend with no multipart parser (D51), a cache with no view
layer (D62).

---

## Worth building, ranked

Ranked by value to brocanth, not by size. Sizes assume somebody who knows the tree.

1. **CSRF (D12) — 1–2 days.** A scaffolded project today accepts any cross-origin
   POST, and every other item on this list that adds a form makes it worse; the
   session plugin already has the per-session secret the token needs.
2. **An `admin` module on the menu (D6–D9, D50, D56) — 2–4 days.** 5,274 lines of
   admin plus seven crates' panels and `brocanth-admin-model`'s adapter are written,
   tested and unreachable, so this is the largest ratio of shipped capability to
   remaining work anywhere in the repository.
3. **`makemigrations` that diffs (D2, D43) — 1–2 weeks.** `ModelRegistry` enumerates
   the entities and `schema_sync::Plan` already splits additive from destructive;
   what is missing is the planner that compares the two and writes the migration
   file, which is the single biggest day-to-day gap against Django.
4. **A mailer (D69), and the reset flow it unblocks (D61) — 3–5 days.** `Accounts`
   returns what to mail and nothing can send it, so the auth module that just
   reached the menu cannot complete its most common journey.
5. **Security headers and CSP (D71, D72, D73) — 1 day.** Three `ResponseHook`s next
   to the five that already ship in `brocanth-middleware`; the cheapest real
   improvement here, and Django 6.0 made CSP table stakes.
6. **A durable `AuthStore` (D10, D59) — 2–4 days.** The only store in the workspace
   is a `HashMap`, so every scaffolded account disappears on restart; a SeaORM-backed
   store makes the auth module true in production rather than in a demo.
7. **Per-view and fragment caching with `Vary` (D62) — 3–5 days.** The hard half —
   a read-through cache with single-flight — exists; the missing half is a response
   hook keyed on path and `Vary`, plus a fragment helper in rsx, and it is the
   feature that makes the Postgres cache pay for itself.
8. **File uploads end to end (D51) — 3–5 days.** A multipart reader in
   `brocanth-input` plus an upload and a download route turns `brocanth-storage` from
   a library into a feature, and the admin's file widget stops being a hole.
9. **Groups and permissions over auth (D58, D56) — about a week.** `brocanth-policy`
   answers "which rows" and nothing answers "which people", which is the question an
   admin, a staff flag and `@login_required` all actually ask.
10. **The test client as a generated dev-dependency (D25) — hours.** `brocanth-test`
    is finished and no generated project gets it; writing `[dev-dependencies]` and
    one example test costs an afternoon and changes how every later feature on this
    list is verified.

**The largest thing still absent and not on this list is i18n (D24, D67, D68)**,
which is 1–2 weeks for catalogs compiled at build time — a good fit for a framework
that compiles its templates and its routes, but nothing else is blocked on it.

**Out of scope, and it bounds three rows above.** There is no client-side component
runtime and there will not be one, so the admin's `autocomplete_fields` (D55) and
`list_editable` inline editing can only ever be a plain form plus a round trip, and
a spatial admin (D77) is out for the same reason plus its own. dp-swap still has no
mutating verb, so anything above that needs a POST without a full navigation is
waiting on the server half of PLAN.md §5.
