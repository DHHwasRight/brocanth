# PocketBase vs brocanth

**PocketBase version checked: v0.40.6, released 2026-10-23.** The previous
inventory in `docs/framework-gaps.md` §1.4 was written against v0.40.5
(2026-10-08). v0.40.6 adds no capability: it caps thumbnail pixels, adds
fileblob bucket escape checks, patches Go dependencies, and announces that
future releases move off GitHub to `ganigeorgiev.com`. P1–P41 keep their
numbers and their meaning. P42–P51 are capabilities the old table missed.

**brocanth state: `main` at `5017473`, read 2026-10-26.** Every row below was
checked against the code in this checkout. `docs/framework-gaps.md` §2.2 is
stale and was not used as evidence. Three of its claims are now wrong:

* It says OpenAPI is unwired. `brocanth-openapi` is on the `brocanth create`
  menu (`crates/brocanth-cli/src/modules/mod.rs`), and there is a command-line
  client too (`crates/brocanth-client`).
* It says realtime is blocked on a buffered response body. The core streams
  now (`crates/brocanth/src/plugin_body.rs:76`, `Body::Streaming`). The SSE
  transport is simply not written
  (`crates/brocanth-realtime/src/scaffold.rs:103`).
* It says the single-binary story lives on a `brocanth-pgserver` branch.
  **There is no such branch and no such crate.**
  `git log --all --name-only | grep pgserver` finds nothing. The
  `embedded-postgres` branch exists, is merged into `main`, and holds the
  chooser and the realtime broker — not an embedded database.

**What "partial" means on a crate row.** A crate that exists but that no
generated project can select is `partial`, with the reason "written, not
reachable". The menu is `crates/brocanth-cli/src/modules/mod.rs` `ALL`: 24
modules, of which three are listed and blocked (`resource`, `realtime-sse`,
`realtime-websocket`). `brocanth-admin` and `brocanth-admin-model` are not on
the menu at all — neither crate has a `src/scaffold.rs`.

| ID | Capability | PocketBase | brocanth | Evidence |
|---|---|---|---|---|
| P1 | One self-contained binary, `./pocketbase serve`, with automatic Let's Encrypt | yes | partial | `crates/brocanth-cli/src/modules/commands.rs` writes a `serve` subcommand; templates compile in (`rsx`, `jt`) and assets embed (`crates/brocanth-assets/src/lib.rs`), so `cargo build` gives one binary. No TLS and no ACME: `crates/brocanth-server/Cargo.toml` names neither. A rebuild needs a Rust toolchain |
| P2 | Embedded SQLite in `pb_data/`, WAL and `synchronous=NORMAL` | yes | partial | `crates/brocanth-cli/src/modules/model.rs:44` scaffolds `sqlite://db/development.sqlite?mode=rwc`, and `crates/brocanth-model/Cargo.toml:57` carries the `sqlite` feature. Postgres is the documented base (`crates/brocanth-model/src/lib.rs:3`). No pragma management, no `pb_data` layout, no embedded server crate anywhere in the tree |
| P3 | A separate auxiliary database for logs | yes | no | Nothing in the tree. `crates/brocanth-cli/src/modules/logging.rs` writes a line per request and stores nothing |
| P4 | The admin dashboard at `/_/`, one embedded SPA | yes | partial | `crates/brocanth-admin/src/lib.rs` — four screens, replaceable rsx views, `Panel` tabs, ships `DenyAll` by default; `crates/brocanth-admin-model/` adapts the ORM. Written, not reachable: no `src/scaffold.rs` in either crate and neither appears in `crates/brocanth-cli/src/modules/mod.rs` `ALL`. `brocanth-auth`, `brocanth-cache` and `brocanth-realtime` each ship a panel behind an `admin` feature that no module turns on |
| P5 | Three collection types: `base`, `view`, `auth` | yes | n/a (out of scope) | `PLAN.md` §12 declines dynamic collections outright: "runtime schema and compile-time types cannot both be the source of truth" |
| P6 | Fourteen field types, each with its own options | yes | partial | `crates/brocanth-schema/src/` has `string`, `number`, `scalar`, `datetime`, `array`, `object`; the `validation` module puts it on the menu. Column types come from SeaORM. No `file`, `relation`, `editor`, `autodate` or `geoPoint` vocabulary, and no field options as data |
| P7 | Five API rules per collection, default `null` = superusers only | yes | yes | `crates/brocanth-policy/src/policy.rs:92` — `Policy` with `list`/`view`/`create`/`update`/`delete`; every method but `list` defaults to `Pred::Never`, so the default is deny. `policy` is on the menu. Stronger than PocketBase: a column is an enum, not a string (`crates/brocanth-policy/src/lib.rs`, the misspelled-field grant). Caveat: the shipped enforcement point is `brocanth-resource`, which is blocked, so an application wires the condition into its own queries |
| P8 | Request context in a rule: `@request.auth/body/query/headers/method/context` | yes | partial | `Policy::Subject` is the application's own type (`crates/brocanth-policy/src/policy.rs:92`), so the auth half is typed and present. No body, query, header, method or context vocabulary exists |
| P9 | Cross-collection joins in a rule, `@collection.name.field` with `:alias` | yes | partial | `Pred::Exists(Box<Membership<C>>)`, `crates/brocanth-policy/src/pred.rs:126` and `:451`. One membership shape, no aliasing and no arbitrary cross-collection path |
| P10 | Sixteen datetime macros in filters and rules | yes | no | Nothing in `crates/brocanth-policy/src/value.rs` or `pred.rs` resolves `@now` or any relative date |
| P11 | One uniform record REST shape over every collection | yes | partial | `crates/brocanth-resource/src/routes.rs` builds the five routes as a `RouteSource`. Written, listed, not available: `crates/brocanth-resource/src/scaffold.rs:93` is `blocked("a generated app dispatches src/pages; these routes arrive as a RouteSource")` |
| P12 | List query parameters: `page`, `perPage`, `sort`, `filter`, `expand`, `fields`, `skipTotal` | yes | partial | `crates/brocanth-resource/src/routes.rs:26-32` accepts `page`, `perPage`, `skipTotal` and `sort`, and refuses an unmapped sort column with a 400. It states that `filter`, `expand` and `fields` are not accepted |
| P13 | The filter grammar: sixteen operators, `?`-prefixed any-of, `&&`/`\|\|` | yes | n/a (out of scope) | Declined by design. `PLAN.md` §9 and `crates/brocanth-policy/src/lib.rs`: "There is no string-to-filter parser in this crate and there is not one anywhere else in the workspace either" |
| P14 | `expand` for relations, six levels deep, gated by the target's view rule | yes | no | Nothing in the tree. `crates/brocanth-resource/src/routes.rs` rejects `expand` |
| P15 | Back-relations, `comments_via_post`, in `filter`, `sort` and `expand` | yes | no | Nothing in the tree |
| P16 | `POST /api/batch`: creates, updates, upserts and deletes in one transaction | yes | no | No batch route and no upsert verb. `crates/brocanth-resource/src/routes.rs` lists exactly five routes |
| P17 | Realtime over SSE: `GET /api/realtime`, `PB_CONNECT`, a client id | yes | partial | The broker half is done and reachable: `crates/brocanth-realtime/src/{broker,mailbox,frame,topic,gate}.rs`, `realtime` on the menu. The transport is not: `crates/brocanth-realtime/src/scaffold.rs:103` — "the core can stream now; this transport is simply not written yet". The core does stream (`crates/brocanth/src/plugin_body.rs:76`) |
| P18 | Subscriptions set by a second `POST /api/realtime`; authorization happens there | yes | partial | `Subscription`, `Topic` and `Gate` exist (`crates/brocanth-realtime/src/lib.rs`), and a policy inverts into an audience key so a write is a map lookup rather than one query per subscriber. No HTTP endpoint sets them, because there is no transport |
| P19 | Password auth over any unique identity field; `GET .../auth-methods` | yes | yes | `crates/brocanth-auth/src/{accounts,password,identity}.rs`; argon2id with no choice offered. The `auth` module is on the menu and the generated plugin carries registration, login, password reset, email verification, sessions and identity linking (`crates/brocanth-auth/src/scaffold.rs:35`). No `auth-methods` listing endpoint |
| P20 | OAuth2 with a provider catalog and `/api/oauth2-redirect` | yes | partial | `crates/brocanth-auth/src/oauth/{mod,pkce,oidc}.rs`, `providers/catalog.rs`, `apple.rs`; PKCE is mandatory and `state` is compared in constant time. Written, not scaffolded: `crates/brocanth-auth/src/scaffold.rs:42-50` writes no provider on purpose, and the generated `Cargo.toml` turns no provider feature on |
| P21 | OTP: `request-otp` then `auth-with-otp`, with the enumeration defence | yes | partial | `crates/brocanth-auth/src/otp.rs`. Written, not scaffolded: `crates/brocanth-auth/src/scaffold.rs:57` — "No second factor, no one-time code, no login alert", because each needs a store trait the generator cannot pick |
| P22 | MFA by combining two methods; 401 with an `mfaId` | yes | partial | `crates/brocanth-auth/src/{mfa,totp}.rs` — `FactorSet`, `FactorPolicy`, and `Verified`/`Admitted` so a skipped second factor does not compile. Written, not scaffolded, same line as P21 |
| P23 | Self-service email flows: verification, password reset, email change | yes | partial | `crates/brocanth-auth/src/{tokens,recovery}.rs` — selector plus hashed verifier, single use, deleted before it is checked. The flows are scaffolded. **Delivery is not**: no SMTP client and no mailer anywhere in the tree |
| P24 | Impersonation: a static, non-refreshable superuser-minted token | yes | partial | `crates/brocanth-auth/src/impersonation.rs` — the session records both identities and `Restricted` names what an impersonated session may not do. Written, not scaffolded: `crates/brocanth-auth/src/scaffold.rs:63` |
| P25 | Stateless tokens, no server-side store, no logout endpoint | yes | partial | Deliberately the other design. `crates/brocanth-auth/src/{session,storage}.rs` keep server-side session rows that rotate on privilege change, which buys revocation and costs a read. `crates/brocanth-storage/src/token.rs` is the one stateless token in the workspace, and its header argues the trade |
| P26 | File upload through the record API, multipart only, with `field+`/`field-` | yes | partial | `crates/brocanth-storage/` stores bytes under a content-hash address, sniffs the media type from the bytes and writes atomically; `storage` is on the menu. No multipart parser exists anywhere: `crates/brocanth-storage/src/lib.rs:18` and the generated `src/plugins/storage/mod.rs` both say the upload route is the application's |
| P27 | File serving with on-the-fly thumbnails, six size forms | yes | partial | `crates/brocanth-storage/src/spec.rs:120` parses all six forms (`WxH`, `WxHt`, `WxHb`, `WxHf`, `0xH`, `Wx0`) with the codec feature off; `crates/brocanth-storage/src/variants.rs` derives and caches, with a pixel cap before the decode. Two gaps: the `variants` feature is off in a generated `Cargo.toml`, and no serving route is scaffolded |
| P28 | Protected files behind `?token=`, with `viewRule` re-evaluated | yes | partial | `crates/brocanth-storage/src/token.rs` mints and verifies an HMAC-SHA256 file token with a short TTL. The route that decides whether to mint one is the application's |
| P29 | Pluggable S3 storage, swapped from the dashboard | yes | no | `crates/brocanth-storage/src/lib.rs:8` — no `s3` feature, by decision (`PLAN.md` §4). `crates/brocanth-storage/src/postgres.rs` is the second backend instead: objects in `bytea` |
| P30 | Backups: list, create, upload, download, delete, restore, on a cron | yes | n/a (out of scope) | `PLAN.md` §12 declines backups: "`pg_dump` plus an object store is the operator's job and a worse wrapper is negative value" |
| P31 | A request log API: `GET /api/logs`, `/{id}`, `/stats`, `DELETE /api/logs` | yes | no | Nothing in the tree. The logging module writes a line and keeps nothing |
| P32 | Settings as a live API: `GET`/`PATCH /api/settings`, no restart | yes | n/a (out of scope) | `PLAN.md` §12 declines database-stored settings. `crates/brocanth-config/src/lib.rs` is layered TOML plus the environment, with an unclaimed section as a build error |
| P33 | A built-in rate limiter with the audiences `""`, `@guest`, `@auth` | yes | no | Nothing in the tree. `crates/brocanth-middleware/src/lib.rs` ships five hooks and names rate limiting in none of them |
| P34 | An in-process cron, plus `GET /api/crons` and a run-now endpoint | yes | partial | `crates/brocanth-jobs/src/cron.rs` parses five fields, UTC only, and `crates/brocanth-jobs/src/queue.rs:402` is `Queue::recurring(name, cron, job)`. `jobs` is on the menu. No HTTP crons API, no run-now, and the generated jobs plugin registers no recurring job |
| P35 | The JS hook VM: `*.pb.js`, goja, hot reload | yes | n/a (out of scope) | `PLAN.md` §12: "a framework whose thesis is compile-time checking does not ship an interpreter" |
| P36 | A framework with hooks, custom routes and one static binary | yes | yes | `crates/brocanth/src/plugin_api.rs` (`Plugin`, `Lifecycle`), `plugin_hooks.rs` (`RequestHook`, `AsyncRequestHook`, `ResponseHook`, `ErrorHook`), `plugin_routes.rs` (`RouteSource`, `RouteBundle`). Registration is explicit and written in `main`, which `PLAN.md` ADR 1 argues for against distributed slices |
| P37 | Schema as data plus generated migrations, `--automigrate`, `migrate up/down` | yes | partial | `crates/brocanth-model/Cargo.toml:22` depends on `sea-orm-migration`, so hand-written migrations work. Nothing exposes them: no migrations directory in `crates/brocanth-cli/tests/golden/everything.txt`, no `migrate` command in `crates/brocanth-cli/src/commands/`, and `crates/brocanth-model/src/schema_sync.rs` ships `PlanOnly`, which "reports what the entity declares and applies nothing" |
| P38 | Collection management over REST; schema mutable at runtime | yes | n/a (out of scope) | Same decline as P5, `PLAN.md` §12 |
| P39 | A superuser CLI out of band from the API | yes | partial | `crates/brocanth-command/` plus `#[cmd]` lets a project write one, and `commands` is on the menu. No account or superuser command ships: the generated `src/commands/` holds `serve`, `css` and `api` only |
| P40 | Two official SDKs wrapping the REST shape, with an auth store and a realtime client | yes | partial | `crates/brocanth-client/src/lib.rs` builds a command-line client from the route table, in the server's own language and inside the server's own binary. `crates/brocanth-openapi/` emits an OpenAPI 3.1 document at `/openapi.json` and fails the build on drift (`check_file`), which a third-party generator can consume. No JavaScript or Dart SDK, and no auth store or realtime client in any language |
| P41 | Static hosting from `pb_public/` and `GET /api/health` | yes | partial | `crates/brocanth-assets/src/lib.rs` — `assets/` fingerprinted, `public/` verbatim, and a missing name is a compile error rather than a 404. No health endpoint: the only `/health` strings in the tree are test fixtures |
| P42 | `POST /api/sql`: arbitrary statements, superuser only | yes | no | Nothing in the tree, and it opposes the typed-predicate design |
| P43 | Mailer: SMTP settings, per-collection templates, the `{APP_NAME}`/`{TOKEN}`/`{OTP}` placeholders | yes | no | No SMTP client and no template set anywhere. The only matches are a redaction test fixture (`crates/brocanth-config/src/redact.rs`) |
| P44 | Record write modifiers: `field+`/`field-` on numbers and multi-valued fields, `field:autogenerate` | yes | no | Nothing in the tree |
| P45 | Filter field modifiers: `:isset`, `:changed`, `:length`, `:each`, `:lower` | yes | partial | Only `:changed`, and in a different place. `crates/brocanth-policy/src/mask.rs` is `Policy::writable() -> FieldMask`, checked against `Tracked::changed()` from `crates/brocanth-model/src/dirty.rs`. `PLAN.md` §9 calls this "stealing PocketBase's `:changed`". The other four have no equivalent |
| P46 | Filter functions: `geoDistance()` and `strftime()` | yes | no | Nothing in the tree |
| P47 | Auth alerts: an email on login from a new device or location, backed by `_authOrigins` | yes | partial | `crates/brocanth-auth/src/alerts.rs` exists. Not scaffolded (`crates/brocanth-auth/src/scaffold.rs:57`) and it cannot send mail, per P43 |
| P48 | Field-level output hiding: `hidden`, `presentable`, `emailVisibility` | yes | partial | `crates/brocanth-policy/src/mask.rs` is a `FieldMask` over typed columns, and `Policy::writable` is the only method that returns one (`crates/brocanth-policy/src/policy.rs:125`). So the mask governs writes, not what a response serialises |
| P49 | Collection indexes as data, unique included, created from the dashboard | yes | partial | Indexes are declared in a hand-written `sea-orm-migration` migration, which is Rust, not data. `crates/brocanth-model/src/validators.rs` carries a `uniqueness` validator at the model layer |
| P50 | `_superusers` as a separate system auth collection: operators are not application users | yes | partial | The seam exists and is locked: `crates/brocanth-admin/src/lib.rs` takes a `Policy` and defaults to `DenyAll`, so an admin mounted without one answers 403 to its own index. No operator account type ships, and `crates/brocanth-auth` models one kind of account |
| P51 | `auth-refresh`: renew a token and get the record back | yes | partial | `crates/brocanth-auth/src/session.rs` rotates a session on privilege change, which is the same move for a stateful store. No refresh endpoint, because no route ships |

**Counts.** yes 3 · partial 30 · no 12 · n/a (out of scope) 6 · total 51.

* **yes (3):** P7, P19, P36.
* **no (12):** P3, P10, P14, P15, P16, P29, P31, P33, P42, P43, P44, P46.
* **n/a (6):** P5, P13, P30, P32, P35, P38 — each declined in `PLAN.md` §9 or
  §12, with a reason written at the time.
* **partial (30):** everything else. Eighteen of those thirty are "the crate is
  written and tested, and a route or a scaffold is missing". That is the shape
  of this repository right now.

## Worth building, ranked

Ranked by what each one unblocks per hour, not by size.

1. **Mount route bundles in a generated `src/app.rs`** (P11, P12, P4) —
   `build()` mounts what the application chose and `dispatch` falls back to
   `App::route_plan()`. `crates/brocanth-resource/src/scaffold.rs:60-67` names
   the two lines. It unblocks the records API and the admin at once, and both
   are already written and tested. **1–2 days.**
2. **The SSE transport** (P17, P18) — broker, mailboxes, frames, topics and
   gates are done, `Body::Streaming` landed, and the scaffold says the
   transport "is simply not written yet". One handler plus a subscribe route
   turns the whole realtime crate from a library into a feature. **2–3 days.**
3. **Scaffold the rest of `brocanth-auth`** (P20, P21, P22, P24, P47) — 8,875
   lines of OAuth2, OTP, MFA, impersonation and alerts that no generated
   project can reach, because each wants a store trait. Ship a default store
   per trait and tick five boxes on the menu. **~1 week.**
4. **Reach the admin from `brocanth create`** (P4, P50) — a `scaffold.rs` for
   `brocanth-admin` and `brocanth-admin-model`, after item 1. It is the one
   PocketBase capability people name first, and it is already built. **2–3
   days.**
5. **Files end to end** (P26, P27, P28) — a multipart decoder in
   `brocanth-input`, plus an upload route and a download route with
   `?thumb=` in the storage module. Every hard part is done; the HTTP edge is
   missing. **3–4 days.**
6. **`brocanth migrate`, and a migrations crate in the scaffold** (P37, P49) —
   `sea-orm-migration` is already a dependency and nothing exposes it, so a
   generated project has an ORM and no way to create a table. **2–3 days.**
7. **The model layer publishes `RowChange`** (P17 chain) —
   `brocanth-resource` already publishes to a `ChangeSink`;
   `crates/brocanth-model/Cargo.toml` does not even name `brocanth-events`.
   Without this, realtime only sees writes that go through a resource. **1–2
   days.**
8. **A mailer seam** (P23, P43, P47) — a `Mailer` trait, an SMTP
   implementation behind a feature, and the token templates. Password reset
   and verification are scaffolded today and cannot deliver anything. **2–3
   days.**
9. **A rate limiter hook** (P33) — a sixth `brocanth-middleware` hook with
   guest and authenticated audiences. Small, and every public deployment needs
   it before it needs most of this list. **1–2 days.**
10. **`/health`, and a request-log sink** (P41, P31) — a health route in the
    scaffold, and a `LogSink` the logging module writes to so a log is
    queryable instead of only printable. **1–2 days.**

Three more sit just outside the ten and are worth naming: `expand` for
relations (P14), the batch endpoint (P16), and datetime values in predicates
(P10). Each one needs a design decision first, which is why none of them is a
day's work.
