# Rails vs brocanth

**Rails version checked: 8.1.4**, released 24 September 2026 (rubygems.org).
8.1.3.1 is the previous patch; there is no 8.2. The capability list in
`docs/framework-gaps.md` §1.3 is still current for 8.1, so R1–R77 keep their
numbers and their wording. R78–R95 are new rows for Rails capabilities that
list missed.

**brocanth version read: `5017473`**, 26 October 2026. Every verdict below
comes from reading the code in this checkout. `docs/framework-gaps.md` §2.2 is
stale and was not used: it says "no HTTP test client" (there is one) and "auth
unwired" (it is on the menu). Where §2.2 and the code disagree, the code wins.

## How a verdict is decided

A capability reaches a user through `brocanth create`. The menu is the `ALL`
array in `crates/brocanth-cli/src/modules/mod.rs`. Three modules are on that
menu and `.blocked(...)`, so nobody can pick them: `resource`, `realtime-sse`,
`realtime-websocket`.

* `yes` — present, and a generated project can have it.
* `partial` — some of it is present, or all of it is written and **no
  generated project can select it** ("written, not reachable").
* `no` — nothing in the tree, or the part that matters is absent.
* `n/a (out of scope)` — brocanth refuses it on purpose. Only one Rails row
  hits a declared non-goal.

The declared non-goals are: no client-side component runtime (`framework-gaps`
preamble, constraint 2), no LiveView-style stateful process per connection
(`PLAN.md` §5), and policy rules compile to SQL instead of being an expression
language (`PLAN.md` §9).

Crates that exist and no generated project can reach: `brocanth-admin`,
`brocanth-admin-model`, `brocanth-test`, `brocanth-events`. `brocanth-assets`
is reachable, but only as a side effect of picking `rsx-tailwind` or
`rsx-htmx`.

---

| ID | Capability | Rails | brocanth | Evidence |
|---|---|---|---|---|
| R1 | `rails new` writes the tree, lockfile, `bin/setup`, `bin/dev`, Dockerfile, deploy config, test harness; `--skip-*` | yes | partial | `crates/brocanth-cli/src/commands/create.rs` `scaffold_files`; the full output is `crates/brocanth-cli/tests/golden/everything.txt` — no Dockerfile, no setup script, no `tests/`, no `db/`. Opt-outs are inverted: `--with` opts in, there is no `--skip-*` |
| R2 | `generate scaffold post title:string` emits model, migration, seven actions, five views, routes and a test | yes | no | No `generate` subcommand: `crates/brocanth-cli/src/main.rs` `enum Command` is `Create`, `Add`, `Delegated`. `crates/brocanth-cli/src/modules/model.rs:61` tells the user to run `brocanth generate model`, which does not exist |
| R3 | `generate model` / `controller` / `resource`, and `destroy` reversing a generator | yes | no | `crates/brocanth-cli/src/commands/add.rs` adds a *module*, not a model. `brocanth.toml` records per-module files "for a `remove` that does not exist yet" (`create.rs` `brocanth_toml`) |
| R4 | `generate migration AddPartNumberToProducts part_number:string` parses the name and writes the body | yes | no | nothing in the tree |
| R5 | `generate authentication`: user with `has_secure_password`, DB-backed `Session`, `Current`, an `Authentication` concern, reset mailer | yes | partial | `crates/brocanth-auth/src/scaffold.rs` writes one file, `src/plugins/auth/mod.rs`. argon2id is real (`crates/brocanth-auth/src/password.rs`); the store is a `HashMap` (`crates/brocanth-auth/src/memory.rs`), there is no user model, no migration, no sign-in page, and the generated header says a sync handler cannot call the async API |
| R6 | Custom generators, `config.generators`, `rails new -m template.rb` | yes | partial | `crates/brocanth-scaffold/src/lib.rs` `trait Module` is a real third-party generator seam, but joining the menu is two lines in `crates/brocanth-cli/src/modules/mod.rs` plus a CLI rebuild. No `-m` / application template |
| R7 | Associations with `dependent:`, `polymorphic:`, `counter_cache`, `touch:`, `inverse_of`, `has_many :through` | yes | partial | SeaORM's own relations only (`crates/brocanth-model/fixtures/blog.rs`). brocanth adds the relation *slot* (`crates/brocanth-model/src/deferred.rs`) and nothing else — no through, polymorphic, dependent, counter cache or touch |
| R8 | Single Table Inheritance; `delegated_type` | yes | no | nothing in the tree |
| R9 | Declarative validations, custom validators, `if:` / `on: :create`, an `errors` object with `full_messages` | yes | partial | `crates/brocanth-model/derive/src/lib.rs` `validates`, `crates/brocanth-model/src/validators.rs` (presence, length, format, numericality, inclusion, uniqueness), `crates/brocanth-model/src/errors.rs` `Errors`/`Entry::full_message`. No conditional or context-scoped validation: the derive's `Common` carries only `message` |
| R10 | Callbacks `before_validation` … `after_commit` / `after_rollback` / `after_touch` | yes | partial | Only SeaORM's four. The derive emits one, `before_save` → `enforce` (`crates/brocanth-model/derive/src/lib.rs`). No `after_commit`, which is what a broadcast would hang off |
| R11 | `scope :published, -> { … }`, `default_scope`, `unscope`, `merge` | yes | no | nothing in the tree; a user composes `Select<E>` by hand |
| R12 | `includes` / `preload` / `eager_load`, and `strict_loading` | yes | yes | `crates/brocanth-model/src/eager.rs` `EagerStrategy`, `with_related` / `without_related`; `crates/brocanth-model/src/deferred.rs` `NotLoaded` panics rather than querying — strict loading by construction; `crates/brocanth-model/src/probe.rs` detects N+1. Strategy is inferred from LIMIT/OFFSET and reported, where Rails asks |
| R13 | `enum :status, […]` generating predicates and a scope | yes | no | nothing in the tree; the fixture models status as a `String` with `inclusion` (`crates/brocanth-model/fixtures/blog.rs`) |
| R14 | `find_each` / `find_in_batches` / `in_batches` with a cursor | yes | no | nothing in the tree |
| R15 | `insert_all`, `upsert_all`, `pluck`, `pick`, `explain`, `annotate`, `load_async` | yes | partial | `crates/brocanth-model/src/bulk.rs` ships the opposite default — hook-running fan-out (`insert_each`, `update_where`) with a witnessed bypass (`HooksBypassed::because`, `unchecked::insert_many`). No upsert/ON CONFLICT, no `pluck`, no `explain`, no async loading |
| R16 | `encrypts :email, deterministic: true` | yes | no | nothing in the tree for the model layer |
| R17 | `connects_to`, `connected_to(role: :reading)`, sharding, GET routed to a replica | yes | no | The generated plugin holds one `url: String` and one `Db` item (`crates/brocanth-cli/src/modules/model.rs`) |
| R18 | Reversible migrations, one `change` with the inverse inferred, versioned migration classes | yes | partial | `sea_orm_migration` is re-exported (`crates/brocanth-model/src/lib.rs`) and migrations are hand-written `up`/`down` (`crates/brocanth-model/fixtures/blog.rs`). No inferred inverse, no version pinning, and a generated project has no `migrations/` directory |
| R19 | A schema dump in two formats plus `schema_migrations` | yes | partial | `seaql_migrations` exists and is read (`crates/brocanth-admin/src/discovery.rs` `MIGRATION_TABLES`). No schema dump of any kind |
| R20 | `db:create`, `db:migrate`, `db:rollback STEP=`, `db:migrate:status`, `db:schema:load`, `db:seed`, `db:prepare` | yes | no | A generated project's command line is `serve` (plus `css` and `api` from other modules); `crates/brocanth-cli/src/modules/model.rs` `contribute` adds no command. `crates/brocanth-model/src/testing.rs` has `migrate`/`rollback` and they are test-only |
| R21 | Strong parameters: `params.expect(post: [:title, :body])`, raising on an unpermitted hash | yes | yes | The declared struct *is* the allow-list: `crates/brocanth-input/src/lib.rs` `Entrance::input`. A field not on the struct can never be assigned; loud rejection is opt-in through `#[entry(strict)]` → `refuse_spare` (`crates/brocanth-schema/src/object.rs` `UnknownKeys::Reject` is the default on the hand-built side) |
| R22 | `before_action` / `after_action` / `around_action` / `skip_before_action`; a filter that renders halts the chain | yes | partial | `crates/brocanth/src/plugin_hooks.rs` `RequestHook` (before/after) and `Flow::respond` (halt by rendering); `crates/brocanth/src/routing_middleware.rs` `Layer`/`Next` (around) and `Scope::except`. Skipping is by path, never per action; plugin hooks are global |
| R23 | `respond_to` dispatching one action to `format.html`, `.json`, `.turbo_stream`, `.md` | yes | no | Nothing reads `Accept`. `crates/brocanth/src/routing_verb.rs:111` reserves `handle_json` as "a content-negotiation hook" and returns `None`. `crates/brocanth-middleware/src/content_types.rs` is the request side only (415). Format is chosen by file: `status.rs` + `status.jt` |
| R24 | `flash` across a redirect, `cookies.signed` / `.encrypted` / `.permanent`, `reset_session` | yes | partial | Signed session cookie and rotation: `crates/brocanth-auth/src/session.rs` `SigningKey`, `Sessions::seal`/`rotate`/`revoke`. **No flash** — `SessionRecord` (`crates/brocanth-auth/src/storage.rs`) has no data bag, and the `sessions` module's `layer` is a pass-through (`crates/brocanth-cli/src/modules/sessions.rs`). No general cookie jar, no encrypted cookies |
| R25 | CSRF: an `authenticity_token` on every non-GET form, rejected without a valid one | yes | no | nothing in the tree. The `csrf` hits are the OAuth `state` check (`crates/brocanth-auth/src/oauth/mod.rs`). `docs/middleware.md:73` says CSRF is blocked on dp-swap's missing mutating verb |
| R26 | Conditional GET: `fresh_when` / `stale?`, `ETag`, `Last-Modified`, 304 | yes | partial | Static assets do it, in generated code: `crates/brocanth-cli/src/commands/create.rs` (the `if-none-match` → 304 branch) over `crates/brocanth-assets/src/lib.rs` `Asset::etag`. For a dynamic response there is no ETag hook — the only `impl ResponseHook for ETag` in the tree is a doc example at `crates/brocanth/src/plugin_hooks.rs:596` and a fixture in `crates/brocanth/tests/response_body.rs`. No `Last-Modified`, no `fresh_when` |
| R27 | `resources :photos` → seven named routes plus `photo_path(id)`; `only:`, singular, `shallow:` | yes | partial | Named helpers are **better than Rails**: `crates/brocanth/src/routing_codegen.rs` `generate_helpers` emits one `path`/`url` per route with that route's exact arity through `build.rs`, so a bad link is `error[E0061]`. Seven-from-one does not exist; the five CRUD routes in `crates/brocanth-resource/src/scaffold.rs:93` are `.blocked(...)` |
| R28 | `member` / `collection` blocks, route `concern`s, `namespace`, `scope` | yes | partial | A directory is a namespace and a nested path (`crates/brocanth/src/routing_path.rs` `module_path_for_file`); member and collection routes exist as files but nothing names them as such. `crates/brocanth/src/routing_pattern.rs` `ScopePattern` scopes *middleware*, not declarations. No route concerns |
| R29 | Segment regexes, request constraints (`subdomain:`), globs, `defaults:`, `mount RackApp, at:` | yes | partial | Globs yes: `crates/brocanth/src/routing_path.rs` `ParamKind::CatchAll`, enforced last-only. No segment regex, no request constraint, no defaults. `crates/brocanth/src/plugin_app.rs:853` `mount_routes` exists and a generated `src/app.rs` never reads `route_plan` |
| R30 | `bin/rails routes` with `-g`, `-c`, `--unused`; `assert_routing` | yes | partial | `brocanth routes` is referenced in six doc comments (e.g. `crates/brocanth/src/plugin_routes.rs:243`) and is **not implemented** — the CLI has `create` and `add`. The table is enumerable and every generated project ships `fn the_table_is_the_shape_of_the_directory` (`crates/brocanth-cli/src/commands/create.rs` `PAGES_MOD_RS_TESTS`) |
| R31 | `form_with model: @book` inferring URL and verb, scoping names, injecting `_method` and the token | yes | partial | The server half of the `_method` tunnel is written: `crates/brocanth-middleware/src/method_override.rs`, POST-in/unsafe-verb-out. The form builder is not: the only one is the backoffice's (`crates/brocanth-admin/src/form.rs` `widget_for`, `Form::accept`), it infers no URL or verb, and the admin is not on the menu |
| R32 | Partials with `collection:`, `as:`, `locals:`, `spacer_template:`, the `_counter` local, `render @collection` | yes | partial | An rsx component is a partial with typed locals, and `render` is the no-layout fragment escape hatch (`crates/brocanth-cli/src/modules/rsx.rs:258`). A collection is a plain Rust `for`; no counter local, no spacer, no shorthand, no per-partial layout |
| R33 | `layout "admin"`, `layout :method`, `yield :sidebar` with `content_for`, nested layouts, template inheritance | yes | partial | Nested layouts are the strongest part of the view layer: `layout.rsx` stacks root-down off the directory tree at compile time (`crates/brocanth-pages/src/lib.rs` `html!`). No named slots — `html!` fills `children` and nothing else — and no runtime layout choice |
| R34 | `accepts_nested_attributes_for` with `f.fields_for` | yes | no | nothing in the tree |
| R35 | Turbo Drive: intercept every link and form, swap `<body>`, `data-turbo-permanent` | yes | partial | `crates/brocanth-cli/vendor/dpswap.min.js` boosts same-origin `<a>` clicks, swaps into `dp-target`, does `pushState`/`popstate`, prefetches on hover with an LRU, and has eight swap styles. **Forms are not intercepted** — the matcher is `e.target.closest("a")` — and every request is `fetch(url, {signal, headers})` with no method and no body |
| R36 | Turbo Frames: `turbo_frame_tag`, scoped navigation, lazy frames | yes | partial | `dp-target` / `dp-select` scope a swap, and lazy loading exists twice: `dp-trigger="revealed"` and `dp-island` + `dp-client="visible"` (`crates/brocanth-cli/vendor/dpswap.min.js`). No server-side frame identity (only `DP-Partial` depth), and a frame does not scope its descendant links |
| R37 | Turbo Streams over the response channel: `.replace` / `.append` / `.remove` / `.update` / `.before` / `.after` | yes | partial | `applyOutOfBand` reads `dp-swap-oob="<style>[:<selector>]"` with the eight styles, covering every Rails operation (`crates/brocanth-cli/vendor/dpswap.min.js`). But it rides an ordinary GET body, there is no builder API on the server, and with no POST there is no non-GET channel |
| R38 | Turbo Streams over WebSocket: `turbo_stream_from` plus `broadcast_append_to` | yes | no | The broker is written (`crates/brocanth-realtime/src/broker.rs`); the transport is `.blocked("an upgrade out of the request/response shape, plus a frame codec")` at `crates/brocanth-realtime/src/scaffold.rs:134` |
| R39 | Page refreshes with morphing; `broadcasts_refreshes` | yes | no | dp-swap has eight swap styles and no DOM morph, no scroll preservation, and no broadcast path |
| R40 | Stimulus: `data-controller`, `data-action="click->x#y"`, targets and values | yes | partial | dp-swap's islands are the nearest thing: `dp-island="<module url>"` dynamic-`import()`s a module and calls `mount(el, props)` with `dp-props` JSON, gated by `dp-client`. That is lazy JS mounting, not a controller/target/value/action vocabulary. htmx itself is not vendored despite the module's name (`crates/brocanth-cli/src/modules/rsx.rs:114`) |
| R41 | Action Cable channels: `subscribed` / `unsubscribed`, `stream_from`, browser-callable `perform()` | yes | n/a (out of scope) | `PLAN.md` §5 refuses "a LiveView-style stateful process per connection: process memory times concurrent users, sticky sessions, and a round trip per keystroke". A Channel object per subscription with browser-callable methods is that shape |
| R42 | `ActionCable.server.broadcast`, `Channel.broadcast_to`; async/redis/postgresql adapters | yes | partial | `crates/brocanth-realtime/src/broker.rs` indexes subscriptions by topic and inverts a policy predicate into an audience key, so one write is a map lookup; `crates/brocanth-realtime/src/notify.rs` carries it across processes on `LISTEN/NOTIFY`. Nothing reaches a browser |
| R43 | `perform_later`, `set(wait:, queue:, priority:)`, `queue_as`, `retry_on` / `discard_on` | yes | yes | `crates/brocanth-jobs/src/queue.rs` `Enqueue::queue/priority/at/in_/unique`, `Queue::discard`/`discard_dead`; `crates/brocanth-jobs/src/backoff.rs` exponential with jitter; retries are a `const` on the `Job` trait with a working default |
| R44 | `ActiveJob::Continuable`: `step … step.advance! from:` resuming at a cursor | yes | no | nothing in the tree; a job's `perform` either completes or retries from the start |
| R45 | Solid Queue: database-backed, `FOR UPDATE SKIP LOCKED`, `bin/jobs start`, no Redis | yes | yes | `crates/brocanth-jobs/src/store/sql.rs`, `crates/brocanth-jobs/src/worker.rs` (concurrency, heartbeat, orphan reclaim, graceful drain), dead-letter table with payload, error and backtrace. Goes further than Rails: `crates/brocanth-jobs/src/cron.rs` ships recurring work, and the backend is a Cargo feature so dev and production cannot differ |
| R46 | Solid Cache: database-backed, FIFO eviction | yes | yes | `crates/brocanth-cache/src/postgres.rs` (an `UNLOGGED` table, on purpose), TTL enforced on read, `crates/brocanth-cache/src/sweep.rs` for eviction, `crates/brocanth-cache/src/fetch.rs` read-through with a lease so one caller computes on a stampede |
| R47 | Solid Cable: pub/sub through the database, retained a day | yes | partial | `crates/brocanth-realtime/src/notify.rs` is the database half and it works across processes. There is no socket for it to feed |
| R48 | Fragment caching `<% cache product do %>`, keyed on `updated_at` and the template digest | yes | no | The template language has no cache construct — `grep -i cache` over the rsx checkout returns nothing. `crates/brocanth-cache` is a byte-blob KV store keyed on a string you build by hand |
| R49 | Russian-doll caching: nested `cache` blocks plus `touch: true` | yes | no | needs R48 and R7's `touch:`, and has neither |
| R50 | Collection caching `cached: true` with one multi-read, `Rails.cache.fetch`, a per-request SQL query cache | yes | partial | `Rails.cache.fetch` has an equal and a better one: `CacheExt::fetch` (`crates/brocanth-cache/src/fetch.rs`), and `mget` is a real multi-read (`crates/brocanth-cache/src/cache.rs`). No collection caching, no SQL query cache |
| R51 | `has_one_attached` / `has_many_attached`, `attach`, `download`, `purge`, `purge_later` | yes | no | `crates/brocanth-storage` has put/get/meta/delete on a `Storage` trait and no model association anywhere — `attach` and `purge` do not exist (the only `purge` is `Cache::purge_expired`) |
| R52 | `config/storage.yml` services Disk/S3/GCS/Mirror; redirect mode or proxy mode | yes | partial | Two backends: `crates/brocanth-storage/src/local.rs` and `crates/brocanth-storage/src/postgres.rs`. No S3, by decision (`crates/brocanth-storage/src/lib.rs`: "An S3 backend is a later, separate crate… no `s3` feature here"). `crates/brocanth-storage/src/token.rs` MACs a capability token, which is the signed-ID half; the generated plugin writes no upload or download route |
| R53 | Variants: `resize_to_limit`, named variants, `representation`, libvips/ffmpeg/poppler | yes | partial | `crates/brocanth-storage/src/variants.rs` `VariantSpec` and `Storage::get_variant`, behind a `variants` feature so the decoders are absent unless asked for, with the decompression-bomb cap before the decode. No named model variants, no video or PDF previews, and generating is not a job |
| R54 | Direct upload: `direct_upload: true`, browser to S3, bucket CORS | yes | no | needs R51 and R52's remote service, and has neither |
| R55 | Action Mailer: `mail(to:, subject:)`, automatic multipart, attachments, `deliver_now` vs `deliver_later` | yes | no | nothing in the tree. `crates/brocanth-auth/src/alerts.rs:3`: "The crate sends no mail" — auth returns `LoginAlert`, `RegistrationNotice` and `ResetOutcome` values for a mailer that does not exist |
| R56 | Mailer previews at `/rails/mailers` | yes | no | needs R55 |
| R57 | Action Text: `has_rich_text`, `rich_textarea` over Trix, sanitised `to_s` | yes | no | nothing in the tree, and no HTML sanitiser anywhere (the `sanitize` functions are route-identifier and OpenAPI-name helpers) |
| R58 | Action Mailbox: `ApplicationMailbox.route`, ingress adapters, incineration, the conductor | yes | no | nothing in the tree |
| R59 | Active Model: `Attributes`, `Validations`, `Errors`, `Dirty`, `Naming`, `SecurePassword` on a plain class | yes | partial | `crates/brocanth-schema` validates a plain struct with no database near it — `#[derive(Validate)]`, a full constraint vocabulary, failures as `Issue` (`crates/brocanth-schema/src/issue.rs`). The form-shaped error object and dirty tracking are bound to SeaORM: `crates/brocanth-model/src/validation.rs` imports `ActiveModelTrait`, so a table-less object cannot use them |
| R60 | `ActiveSupport::Concern` with `included do` and `class_methods do` | yes | yes | The language is the mechanism: a trait with default methods and a blanket impl, as `crates/brocanth/src/plugin_api.rs` `trait Plugin` uses it. No framework code is needed and none should exist |
| R61 | Core extensions: `blank?`, `try`, `delegate`, `1.day`, `3.megabytes`, `to_fs(:currency)`, `String#pluralize` | yes | no | No core-extension crate. The one near-miss is `fn humanize` inside `crates/brocanth-model/src/errors.rs` |
| R62 | `ActiveSupport::CurrentAttributes`; `ActiveSupport::Notifications` instrumenting every framework event | yes | partial | The `Current` half is present and better than Rails': `crates/brocanth/src/plugin_context.rs` `Context<E>` with `cx.extract::<&T>()`, compile-checked against the installed providers, per request and not a thread-local. The instrumentation half is absent — see R63 |
| R63 | `Rails.event.notify("user.signup", …)`, `tagged`, `set_context`, subscribers | yes | no | `crates/brocanth-events/src/lib.rs` carries exactly one event, `RowChange`, over a `ChangeSink`. Not named events with arbitrary payloads, and the crate is **not on the menu** — it arrives only as a transitive dependency of realtime and resource. `crates/brocanth-jobs/src/events.rs` `JobEvent` is crate-private |
| R64 | `credentials:edit` with `master.key`, per-environment files, `credentials:fetch` | yes | no | `crates/brocanth-config/src/lib.rs:135` declines it by name: "Database-stored settings and secret management are declined" |
| R65 | `config.load_defaults 8.1` pinning every behavioural default to one version | yes | no | `brocanth.toml` (`crates/brocanth-cli/src/commands/create.rs` `brocanth_toml`) records a module list, files and config sections, and no framework version |
| R66 | Propshaft: content-digest fingerprints, `.manifest.json`, `asset_path`, `assets:precompile` | yes | yes | `crates/brocanth-assets/src/lib.rs` — SHA-256 fingerprints for `assets/`, verbatim `public/`, a compile-time manifest, strong ETag and `max-age=31536000, immutable`. `assets::url()` beats `asset_path`: an unknown name is a build failure, not a 404. Reachable only through `rsx-tailwind` or `rsx-htmx` (`crates/brocanth-cli/src/commands/create.rs:1466`) |
| R67 | importmap-rails: `bin/importmap pin`, bare specifiers, no Node, `bin/importmap audit` | yes | no | nothing in the tree |
| R68 | jsbundling / cssbundling / tailwindcss-rails, with `bin/dev` and `Procfile.dev` | yes | partial | Tailwind only: `crates/brocanth-cli/src/modules/rsx.rs` writes a `package.json` with one devDependency and a `brocanth css` command (`-W` to watch) that shells to the Tailwind CLI. No JS bundler, no `Procfile`, no `bin/dev` |
| R69 | `bin/rails console` with a sandbox, `runner`, `about`, `stats` | yes | partial | `crates/brocanth-command/src/lib.rs` registers `#[cmd]` structs at link time through a `linkme` slice, and `crates/brocanth-cli/src/commands/delegate.rs` forwards an unknown subcommand to the project binary — that is the `runner` role for compiled commands. No REPL, and `PLAN.md` §6 says so: "There is no REPL and there is no fix." No `about`, no `stats` |
| R70 | Fixtures: YAML loaded once, `posts(:welcome)`, each test in a rolled-back transaction | yes | no | The transaction half exists (R72). File fixtures do not — `crates/brocanth-admin/src/fixture.rs` is an in-memory `AdminData` double written in Rust |
| R71 | Four test layers — model, controller, integration, system — with `assert_response`, `assert_difference`, `assert_emails` | yes | partial | **Written, not reachable.** `crates/brocanth-test/src/client.rs` drives a built `App` through hooks → router → handler → hooks with no socket; `crates/brocanth-test/src/response.rs` has `assert_ok`/`assert_status`/`assert_redirects` and redirect following; `crates/brocanth-test/src/html.rs` asks real questions (`assert_link_to`, `assert_form_posts_to`, `within`). `crates/brocanth-cli/src/commands/create.rs` emits no `[dev-dependencies]` at all, so no generated project has it |
| R72 | `parallelize(workers: :number_of_processors)` with a numbered database per worker; `test -n`, `test:system` | yes | partial | `crates/brocanth-model/src/testing.rs` `Harness` gives SQLite a private `sqlite::memory:` per test and isolates Postgres with `pg_advisory_xact_lock` plus a `TestTransaction` that has no `commit` method. No runner integration, no per-worker Postgres database, and nothing is scaffolded |
| R73 | Local CI: `config/ci.rb` with a `CI.run` DSL run by `bin/ci` | yes | no | The nearest thing a project gets is `mise.toml` `[tasks.test] run = "cargo test"` (`crates/brocanth-cli/src/commands/create.rs` `mise_toml`) |
| R74 | `rails plugin new --mountable`, `mount Blorgh::Engine, at: "/blog"`, engine migrations | yes | partial | Both halves exist and the runtime half is unreachable. Build time: `crates/brocanth-scaffold/src/lib.rs` `Module` plus `crates/brocanth-cli/src/commands/add.rs` replaying it with digest guards. Run time: `crates/brocanth/src/plugin_routes.rs` `RouteSource` and `crates/brocanth/src/plugin_app.rs:853` `mount_routes` — which a generated `src/app.rs` never calls. No module contributes a migration; `crates/brocanth-jobs/src/scaffold.rs` hands you `schema_sql()` to paste |
| R75 | `config.middleware.use` / `insert_before` / `delete`, `bin/rails middleware`, `mount` | yes | partial | Inspectable: `crates/brocanth/src/routing_middleware.rs` `patterns_for`, `crates/brocanth/src/plugin_hooks.rs` `Pipeline::request_hooks()`. Reorderable declaratively and validated at build: `crates/brocanth/src/plugin_order.rs` `Order::before`/`after`. No `delete` or `swap` on an assembled stack, and no command to print it |
| R76 | Kamal 2 in `config/deploy.yml`; `kamal setup` with zero-downtime cutover | yes | no | nothing in the tree, and no Dockerfile either |
| R77 | Thruster in front of Puma: X-Sendfile, asset caching, gzip and brotli, one Go binary | yes | no | Asset caching exists (R66). Compression does not: `crates/brocanth-middleware/src/lib.rs:34` lists it as absent "because gzip is a dependency and this crate has none yet". No X-Sendfile |
| R78 | Zeitwerk: constants autoloaded from paths, reloading in development, eager loading in production, `zeitwerk:check` | yes | partial | The registration half is structural and checked by the compiler: `crates/brocanth-pages/src/lib.rs` `pages!` and `load_dir!` read the directory while the crate compiles, so a file in `src/pages` is a route with nothing to register. There is no reloading — a change is a rebuild (`PLAN.md` §6: "Hot reload does not exist outside Dioxus") |
| R79 | `rate_limit to: 10, within: 3.minutes` on a controller, over the cache store | yes | no | nothing in the tree. `PLAN.md` §4 item 11 lists rate limiting under "the cheap finish", unbuilt |
| R80 | `allow_browser versions: :modern` answering 406 below a floor | yes | no | nothing in the tree |
| R81 | PWA: `manifest.json.erb` and `service-worker.js` generated and routed | yes | no | a generated `public/` holds `dpswap.min.js` and nothing else (`crates/brocanth-cli/tests/golden/everything.txt`) |
| R82 | Hotwire Native: path configuration, bridge components, iOS and Android shells | yes | no | nothing in the tree |
| R83 | `bin/rails notes` extracting TODO / FIXME / OPTIMIZE annotations | yes | no | nothing in the tree |
| R84 | `/up` from `Rails::HealthController`, which Kamal's deploy probe reads | yes | no | nothing in the tree. `PLAN.md` §4 item 11 names `/up` as unbuilt. Graceful shutdown, its neighbour on that list, *is* built: `crates/brocanth-server/src/lib.rs` `GracefulShutdown` |
| R85 | `bin/rails dbconsole` opening the database client with the app's credentials | yes | no | nothing in the tree |
| R86 | I18n: `config/locales/*.yml`, `t` and `l`, pluralization rules, fallbacks, a locale per request | yes | no | nothing in the tree. `PLAN.md` §4 cuts i18n from v1 by name. `crates/brocanth-schema/src/issue.rs:154` says messages are "free to… be translated", which is an invitation, not a mechanism |
| R87 | `ActiveSupport::ErrorReporter`: `Rails.error.report` / `handle` / `record`, severity, context, subscribers | yes | partial | The seam exists: `crates/brocanth/src/plugin_hooks.rs` `ErrorHook` and `Pipeline::error_hooks()`, and `crates/brocanth-jobs/src/store.rs` keeps the error and the backtrace on the dead letter. No application-facing reporter, no severity, no context, no subscriber list |
| R88 | Content Security Policy DSL with a per-request nonce; `config.force_ssl` and HSTS | yes | no | nothing in the tree — the `nonce` hits are OIDC. `crates/brocanth-middleware` has proxy headers, method override, canonical host, content types and CORS, and no security headers |
| R89 | API-only mode: `rails new --api`, `ActionController::API`, a trimmed middleware stack, no view layer | yes | yes | The whole design is opt-in, so API-only is the default and HTML is the choice: `rsx` is a menu row, not a given (`crates/brocanth-cli/src/modules/mod.rs`). `crates/brocanth-openapi/src/scaffold.rs` derives the document from the route table and `crates/brocanth-client/src/scaffold.rs` derives a typed CLI client from the same table |
| R90 | `Rails.logger`, `ActiveSupport::TaggedLogging`, per-environment levels, `config.filter_parameters` | yes | partial | `crates/brocanth-cli/src/modules/logging.rs` writes a request logger into the project with a `[logging] level` section — the whole of logging, readable and deletable. No tagging and no request-parameter filtering. `crates/brocanth-config/src/redact.rs` redacts by key name, but for config dumps, not for the log |
| R91 | `MessageVerifier` / `MessageEncryptor`, `record.signed_id(expires_in:)` and `find_signed`, GlobalID | yes | partial | Two concrete uses and no general facility: `crates/brocanth-auth/src/session.rs` `SigningKey::sign_message` (HMAC-SHA256) and `crates/brocanth-storage/src/token.rs` (a MAC'd capability token for a blob). No `signed_id` on a record, no `find_signed`, no GlobalID |
| R92 | `travel_to`, `freeze_time`, `travel_back` in tests | yes | partial | Injectable clocks per crate: `crates/brocanth-auth/src/clock.rs`, `crates/brocanth-jobs/src/clock.rs`, `crates/brocanth-cache/src/clock.rs`. No framework-wide test helper, so a test moves one crate's clock at a time |
| R93 | `rescue_from`, `config.exceptions_app`, `public/404.html`, the development error page | yes | partial | `crates/brocanth/src/plugin_hooks.rs` `ErrorHook` turns an error into a response, and `crates/brocanth-input/src/lib.rs` `Answer` renders a fault as RFC 9457 `application/problem+json` 422. No HTML error pages, no development error page with a source extract |
| R94 | `ActiveSupport::Deprecation` with `:raise` / `:log` / `:notify` per environment | yes | no | nothing in the tree |
| R95 | Optimistic locking through `lock_version`; `lock!` / `with_lock` | yes | no | nothing in the model layer. `FOR UPDATE SKIP LOCKED` exists in `crates/brocanth-jobs/src/store/sql.rs` and is the queue's, not a model's |

## Row counts

| Verdict | Rows |
|---|---:|
| `yes` | 8 |
| `partial` | 42 |
| `no` | 44 |
| `n/a (out of scope)` | 1 |
| **Total** | **95** |

The eight `yes` rows are R12 (eager loading and strict loading), R21 (strong
parameters), R43 and R45 (jobs and the queue), R46 (the cache), R60
(concerns, which the language supplies), R66 (the asset pipeline) and R89
(API-only). Four of those — jobs, queue, cache, assets — go further than
Rails: the queue ships cron, the asset helper fails at build time, and the
job backend cannot differ between development and production.

Seven `partial` rows are "written, not reachable" rather than half-built:
R31 and R71 (the admin's form builder and `brocanth-test`), R27, R29 and R74
(`resource`, `mount_routes` and engines, all blocked on the one unread
`route_plan` seam), R42 and R47 (the broker, with no transport).

---

## Worth building, ranked

Ranked by what each one unblocks, not by size.

1. **A mutating verb on the client, plus CSRF and a form helper** (R25, R31,
   R37, R24's flash). dp-swap sends one `fetch(url, {signal, headers})` with
   no method and never intercepts a form, so there is no POST path at all.
   That single hole blocks CSRF, the real Turbo Stream channel, any
   `form_with` story and the flash that a redirect-after-POST needs. Nothing
   else on this list unblocks four rows. **Size: 1–2 weeks** — the client
   verb, a token middleware, a form helper and the 422-re-render path.

2. **`brocanth generate`: model, migration, scaffold** (R2, R3, R4). The
   generated project tells the reader to run `brocanth generate model`, a
   command `main.rs` does not define. `PLAN.md` §4 calls the scaffold
   generator "the capstone and also the specification". **Size: 1 week** for
   model and migration; the full seven-action scaffold waits on item 1.

3. **Database tasks, a `migrations/` directory, and an actual pool** (R18,
   R19, R20). The `model` plugin today publishes `Db { url: String }` and
   opens no connection, the project has no `migrations/` directory, and the
   command line is `serve`. A project that picks the ORM cannot run a query
   or create a table. **Size: 3–5 days.**

4. **Mount `RouteSource` in the generated `src/app.rs`** (R27, R29, R74, and
   the admin). One seam nobody reads: `App::mount_routes` and `route_plan`
   are written and tested, and a generated app dispatches `pages::router()`
   alone. Reading it unblocks the `resource` module, mountable engines, the
   5,274-line admin, and lets OpenAPI stop pretending to be a page.
   **Size: 1–2 days.**

5. **Wire `brocanth-test` into generated projects** (R71, R70, R72). The
   crate is finished and good — an in-process client, redirect following, an
   HTML reader that knows what a link and a form are — and `create.rs` emits
   no `[dev-dependencies]`, so nobody outside this repository can use it. A
   dependency line and a generated `tests/smoke.rs`. **Size: 4–8 hours**,
   the best return on this list.

6. **The SSE transport** (R38, R41 partly, R42, R47). The body contract
   landed (`Body::Streaming`, `on_headers`/`on_body`, `StreamGate`), and the
   blocked row now says only "this transport is simply not written yet".
   Writing it turns `brocanth-realtime` from a library into a feature and
   gives `LISTEN/NOTIFY` somewhere to arrive. **Size: 3–5 days.**

7. **A dynamic ETag / `fresh_when` response hook** (R26). The hook contract
   was designed around this exact example and the only `impl ResponseHook for
   ETag` in the tree is a doc comment. For a framework whose client is
   GET-only, conditional GET is the highest return per line in the HTTP
   layer. **Size: 1 day.**

8. **A `cache` block in rsx, keyed on a record and the template digest**
   (R48, R49, R50). The cache is a byte store and the template engine has no
   cache construct, so nothing caches a view today. The digest half is free:
   templates already compile, so the digest is a build-time constant — which
   makes Russian-doll nesting cheaper here than in Rails. **Size: 3–5 days**,
   and it needs R7's `touch:` to invalidate properly.

9. **A mailer** (R55, R56). `brocanth-auth` already produces `ResetOutcome`,
   `LoginAlert` and `RegistrationNotice` values and states that it sends no
   mail, so a password reset cannot complete in a generated application. A
   preview surface is the GET-only developer screen this framework is best
   shaped for. **Size: 1 week** for SMTP, templates through rsx, multipart
   and a `deliver_later` that hands off to `brocanth-jobs`.

10. **A `routes` command** (R30, R75). Six doc comments promise `brocanth
    routes` and nothing implements it. The table is already enumerable and
    every generated project already asserts on its shape, so this is a
    printer with a filter — and the same command can print the middleware
    stack. **Size: 4–8 hours.**

Close behind, and left off on purpose: `after_commit` plus a publishing model
layer (R10, R63 — it is what would make `brocanth-events` and the broker
meet), named scopes and enum columns (R11, R13), a Dockerfile and `/up` (R76,
R84), and i18n (R86, cut from v1 by `PLAN.md` §4).
