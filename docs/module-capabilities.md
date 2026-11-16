# Module capabilities: kinds, providers and contribution points

Status: accepted (2026-11-16)

The owner's answers and corrections are in §4, §5, §6 and §13. Nothing
here is built yet. Base:
`8f546fc`. Read first: PLAN.md §8 and ADR 1, `docs/module-config.md` §9,
`docs/module-flags.md`.

## 1. The problem

A module names other modules by key: `project.has("<key>")`,
`.after("<key>")`, `module_ref("<key>", ..)`. Each name couples it to one
module, not to the thing that module gives. A new module that gives the
same thing must edit every reader.

Measured on `8f546fc` with grep (non-test code, 73 module keys, 79
`Manifest::new` calls in 49 files):

| Relation | Count | Note |
|---|---|---|
| `project.has("<key>")` | 180 calls, 36 keys | 159 name a key the crate does not own |
| top keys asked | `config` 47, `admin` 24, `rsx` 13, `i18n-routing` 10, `db` 9, `model` 7, `middleware-csrf` 6, `commands` 6 | 8 keys hold 122 of 180 |
| `.adapts_to` | 135 (+12 through `.integration`) | `config` 40, `db` 9, `rsx` 7, `jobs` 5, `i18n-routing` 5 |
| `.after` | 55 explicit, +29 implied by `.needs` | `config` 17, `db` 11, `auth` 8 |
| `.absent` / `.needs` / `.recommends` / `.conflicts` | 53 / 29 / 8 / 3 | `needs(db)` 8 |
| `module_ref("<key>", ..)` | 24 | 7 read another module: 5× `admin.mount_at`, 2× `storage-routes` |
| `after_if_present` / `before_if_present` | 18 | runtime hook order by another plugin's hook name |
| process-wide `static INSTALLED: OnceLock` | 7 | flags, oauth-client, cache, webhooks, audit, openapi, analytics |

The 47 `has("config")` calls split so: about 23 choose the registration
form (`plugin_from_config` or a literal `Plugin::new`), about 12 choose
where a database URL comes from, 4 choose a help sentence, and the rest
put lists of records in the file only.

Four symptoms:

1. **A hand list of providers.** `crates/brocanth-auth/src/scaffold.rs:187`
   is `POSTGRES_WITH = ["jobs", "cache", "db"]`, and lines 103-114 declare
   three `adapts_to` for it. The real requirement is "a Postgres pool".
   With `jobs` but no `db`, auth opens a second pool from `[auth] url`.
2. **A hand list of consumers.** `i18n-routing` kept four mount keys, and
   each new mounted module had to edit it. `Opt::mount` and
   `Project::mounts()` fixed it (`crates/brocanth-i18n/src/scaffold.rs:168`).
   During the merge, `resource` was missed: its `mount_at` was an
   `Opt::text`. The fix covers only mounts. This doc generalises it.
3. **Copied facts.** `crates/brocanth-middleware/src/scaffold.rs:653`
   copies the sign-in paths of `auth-screens` into the rate-limit example.
   `crates/brocanth-config/src/check.rs:68` (`MIGRATES`) lists seven
   modules that migrate at startup; line 652 hard-codes `sessions || auth`
   as "has cookies". The rate-limit example and the config checks
   collided between review branches over such lists.
4. **Globals because of start order.** The admin builds tabs in `mount`,
   before plugins start, so each tab reads a process-wide global:
   `AnalyticsPanel::installed()` (golden `everything.txt:2607`) reads a
   `static INSTALLED` (same pattern: `crates/brocanth-cache/src/plugin.rs:107`).
   Two apps in one test process share the first value.

And the docs drift: `docs/modules/auth.md:15-16` omits the manifest's
`after("db")`, `adapts_to("db")` and `adapts_to("auth-oauth")`.

## 2. What other frameworks do

| Framework | Mechanism we take | Weakness we avoid | Source |
|---|---|---|---|
| Kubernetes | GVK: a typed name for a resource kind, grouped by owner | per-kind versions with conversion | kubernetes.io/docs/reference/using-api/api-concepts |
| Rails | Railtie `initializer before:/after:`, `ActiveSupport.on_load` lazy hooks, adapters, `middleware.insert_before`, engines, install generators | implicit coupling; load-order bugs | api.rubyonrails.org/classes/Rails/Railtie.html, guides.rubyonrails.org/engines.html |
| Spring Boot | `@ConditionalOnBean` / `@ConditionalOnMissingBean`, `@AutoConfigureAfter`, the conditions report (`--debug`) | conditions run at start, out of sight | docs.spring.io/spring-boot/reference/features/developing-auto-configuration.html |
| NestJS | a module lists `imports`, `providers`, `exports`; only exports are visible outside | runtime DI container | docs.nestjs.com/modules |
| VS Code | contribution points declared in a manifest | JSON, no type check | code.visualstudio.com/api/references/contribution-points |
| Bevy | resources keyed by type; systems ordered by `SystemSet`, not by plugin | ambiguity found only at run time | docs.rs/bevy/latest/bevy/ecs/schedule/trait.SystemSet.html |
| Laravel | contracts (interfaces) bound to one implementation | binding hidden in a service provider | laravel.com/docs/contracts |
| Phoenix | generators write code the app owns (`phx.gen.auth`) | no upgrade path for generated code | hexdocs.pm/phoenix/Mix.Tasks.Phx.Gen.Auth.html |

The blend: Kubernetes names, NestJS exports, VS Code contribution points,
Bevy phases, Laravel contracts with one first implementation, Spring
conditions **resolved by the CLI at generation time**, and Phoenix
ownership of the output. No runtime discovery.

## 3. The model

### 3.1 Kind, Group, and no Version

A **Kind** is a typed capability: one thing a module gives that another
uses. It is a Rust marker type in `brocanth-scaffold`, so a typo does not
compile. A **Group** is the domain that owns it (`db`, `admin`, `http`).
It names the owner in `explain` and in docs; lookup ignores it.

**No version per Kind.** Reason: Kubernetes needs versions because one API
server serves several versions of a resource and converts between them.
brocanth crates release together, and a project pins one release, so
there is no second version to serve. A generated project will record one
**edition** in `brocanth.toml` instead (`edition = "2026-11"`), which
`brocanth upgrade` reads to choose codemods for the files it generated.
The edition arrives with the first codemod that needs it, not before
(§13, decision 4).

**Kinds are generation-time types.** The CLI does not link the core
(`e305bc6`). A Kind names its runtime type (a trait or a handle) as a
path string; the generated code names the real type, and the compiler
checks that code.

| Kind | Group | Runtime type | Card. | Providers | Consumers |
|---|---|---|---|---|---|
| `PgPool` | db | `brocanth_db::Db` | one | `db` (auto-added by a requirement, §4) | auth, jobs, cache, flags, oauth-client, audit, analytics, storage, webhooks, model |
| `JobQueue` | jobs | `brocanth_jobs::Queue` | one | `jobs` (Postgres); dev: `jobs-memory` | mail, webhooks, observe |
| `CacheStore` | cache | `dyn brocanth_cache::Cache` | one | `cache` (Postgres), `cache-redis`; dev: `cache-memory` | route cache, application code |
| `Mount` | http | a path setting + role | many | admin, auth-oauth, oauth-client, resource, storage-routes | i18n-routing, route table test, robots, sitemap, analytics, gtag |
| `AdminPanel` | admin | `impl brocanth_admin::panel::Panel` + RBAC resource | many | cache, jobs, flags, audit, analytics, openapi, oauth-client, webhooks, roles | admin |
| `ConfigSource` | config | a settings layer (one file format) | one | `config` (TOML); later JSON, HCL | the settings loader (§6) |
| `ConfigCheck` | config | a check entry | many | jobs, cache, flags, audit, analytics, oauth-client, webhooks | `brocanth check` |
| `SessionCookie` | http | the session on the request state | one | sessions, auth | oauth-client, auth children, config check (`check.rs:652`) |

Planned when the first module exists, not before: `Scheduler` (pg_cron),
`TimeSeries` (TimescaleDB), `SearchIndex` (Postgres full text). That makes
11 of the ~12 the anti-goals allow (§11). `RequestPhase` (§3.6) is a
closed enum, not a Kind.

### 3.2 provides / requires / uses

- `provides::<K>(what)`: this module gives K.
- `requires::<K>(why)`: this module cannot be written without K. Like
  `needs`, it implies "after the provider".
- `uses::<K>(what_changes)`: this module writes different code when K is
  present. It replaces `adapts_to` on a provider key.

There is no fallback provider. The first draft let `jobs` and `cache`
provide `PgPool` when `db` was absent. §4 makes `db` the only provider of
the one shared pool, so the idea and its cost (publishing a second pool
as `brocanth_db::Db`) go.

### 3.3 Capability Kinds: Postgres first, replaced below the trait

Decision (owner, Q1): a capability module (jobs, cache, scheduler, time
series, search) is a Kind whose runtime type is a **trait**. Its first
provider runs on Postgres and may use Postgres extensions. A caller names
the trait, never the Postgres type. A new backend is a new module that
provides the same Kind. **Reason:** the first users run on Postgres only.
When one outgrows it, the backend changes below the abstraction, and no
caller changes. `CacheHandle` already derefs to `dyn Cache`; `cache-redis`
already plugs in below it. The rule makes that the pattern for every
capability.

### 3.4 Contribution points

A Kind with cardinality **many** is a contribution point: many modules
contribute, one collects. Admin tabs, mounts, config checks and AGENTS.md
sections are contribution points. Tabs already work so
(`Slot::AdminPanels`, `.integration("admin", ..)`); the model gives the
pattern one name and one API.

### 3.5 Exports

A module declares the fields of its module file that other modules may
read: `.exports("sign_in_paths", "the POST paths of the sign-in screens")`.
A `module_ref(other, field)` on another module's key must name an export;
`check_module` enforces it. **Reason:** today any field of any module file
is public to every scaffold. An export is the contract; the rest can
change without notice (NestJS `exports`). Rate-limit then reads
`auth-screens`' paths instead of copying them.

### 3.6 Hook phases

A request hook declares a **phase**, and orders itself inside it:

| Phase | Today's hooks |
|---|---|
| `Identify` | proxy-headers, session, i18n negotiate, method-override |
| `Guard` | rate-limit count, csrf check, auth |
| `Cache` | conditional-get, route cache |
| `Encode` (response) | compression, security headers, RateLimit headers |

These four names are final, and an `after` across phases is a build
error (§13, decision 2).

The core runs phases in this order; `Order::after` / `before` stays for an
edge inside one phase. **Reason:** `after_if_present("proxy-headers.read")`
(`crates/brocanth-middleware/src/rate_limit/mod.rs:643`) names a plugin,
so a second proxy reader must edit every such list. A phase names a stage
of the request (Rails `on_load`, Bevy `SystemSet`). ADR 9 does not change:
a hook that stops the chain still lets every later hook see
`after_dispatch`. `HookSource::REQUIRED` does not change either: required
means "may not be turned off"; a phase means "when it runs". The
manifest's `.hook(..)` gains the phase, so `explain` shows both.

### 3.7 Manifest sketch

```rust
// brocanth-scaffold/src/kind.rs
pub trait Kind: 'static {
    const NAME: &'static str;        // "JobQueue"
    const GROUP: &'static str;       // "jobs"
    const RUNTIME: &'static str;     // "brocanth_jobs::Queue", for generated code
    const CARDINALITY: Cardinality;  // One | Many
}
kind!(JobQueue, group = "jobs", runtime = "brocanth_jobs::Queue", One);

// crates/brocanth-jobs/src/scaffold.rs
Manifest::new("jobs", "a job queue in Postgres ..")
    .requires::<PgPool>("the queue tables live in the project's database")
    .provides::<JobQueue>("the queue, on Postgres (SKIP LOCKED)")
    .contributes::<AdminPanel>(Tab::new("jobs", "the queue: retry, discard"), true)
    .contributes::<ConfigCheck>("deploy/migrate for [jobs]", true)

Manifest::new("jobs-memory", "the queue in memory; gone at restart")
    .provides::<JobQueue>("the queue, in this process")
    .development_only("jobs are lost at restart")

// crates/brocanth-mail/src/scaffold.rs: names the Kind, not `jobs`
Manifest::new("mail", "..").uses::<JobQueue>("sends through the queue; without one, inline")
```

### 3.8 Generated output (auth and mail with `jobs`)

```rust
// src/plugins/mod.rs, the chain the CLI writes
    // PgPool <- db (added: jobs requires PgPool). `brocanth explain PgPool`
    .plugin_from_config::<crate::plugins::db::Database>("db")
    // JobQueue <- jobs
    .plugin_from_config::<crate::plugins::jobs::Jobs>("jobs")
    .plugin_from_config::<crate::plugins::auth::Auth>("auth")   // uses PgPool
    .plugin_from_config::<crate::plugins::mail::Mail>("mail")   // uses JobQueue
```

Auth reads `lc.services().get::<brocanth_db::Db>()` in `before_start`.
Mail reads `lc.services().get::<brocanth_jobs::Queue>()`, the trait
handle, so `jobs-memory` in development and `jobs` in production give the
same mail file. Today mail declares `adapts_to("jobs")` and
`adapts_to("jobs-memory")` separately
(`crates/brocanth-mail/src/scaffold.rs:62-73`).

Auth keeps two files, not three: on the shared pool when `PgPool` is
bound, on `MemoryStore` when it is not. `POSTGRES_WITH`, the "own pool"
variant and `[auth] url` go.

## 4. Decision 1: one shared pool, added on demand

Owner, Q1, with a correction: `db` is **not** in every project. A project
may serve static JSON only, or relay API calls, with no database. `db`
comes in when it is chosen, or when a chosen module requires `PgPool`.
PLAN.md §8 ("nothing ships unless asked") does not change, and there is
no ADR for it.

What does change:

- **One pool per project.** `db` is the only provider of `PgPool`. `jobs`,
  `cache`, flags, audit and the other Postgres modules require it and
  never open their own. **Reason:** one set of connections, sized by
  `[db] max_connections`; one URL; one place for TLS and migrations.
- **Postgres first, replaced below the trait** (§3.3). Modules may rely
  on Postgres extensions (pg_cron, TimescaleDB). A module that needs one
  says so in its `requires` line and in `brocanth check`.
- **Optional users stay optional.** A module that only *adapts* to a
  database declares `uses::<PgPool>`, not `requires`, and keeps its code
  path without one. Auth without `db` writes the `MemoryStore` plugin as
  today. mail without `db` keeps the test that starts the application.
- **Development-only providers stay.** `jobs-memory` and `cache-memory`
  provide `JobQueue` and `CacheStore` with no pool, marked
  `development_only` (`brocanth check --deploy` refuses them). They do not
  require `PgPool`, so they do not pull `db` in.
- `has("db")` (9) becomes `provider::<PgPool>()`; `needs("db")` (8)
  becomes `requires::<PgPool>`; `adapts_to("db")` (9) becomes
  `uses::<PgPool>`; `after("db")` (11) is implied by both.

### 4.1 How the resolver adds `db`

A chosen module `requires::<K>` and no chosen module provides K:

- **One provider in the registry** (non-development): the CLI adds it.
  `brocanth create` prints `added db: jobs requires PgPool (a Postgres
  pool)`, `brocanth.toml` records the reason, and `brocanth explain`
  shows it. **Reason:** there is nothing to choose, and an error that
  names the one fix only adds a round trip. The wizard ticks it on
  screen, so the flags and the chooser agree.
- **Several providers** (`JobQueue`: `jobs`, `jobs-memory`): an error that
  names each one, with a `help:` line. Reason: a backend is a real choice.
- `remove db` while a chosen module requires `PgPool` is refused, and the
  error names the modules that require it.

Today `needs` refuses on the command line and ticks in the chooser. For a
requirement on a Kind, both routes now add the single provider.

### 4.2 `db-embedded`: Postgres in the binary, for development

Owner, former open question 1: **offer** to bundle Postgres for local work. A
module `db-embedded`, part of `db`, starts a Postgres server from the
binary before `db` opens its pool, and hands `db` the URL. `PgPool` is
still `db`'s; nothing else changes.

- Offered at `brocanth create` and `brocanth add db`, off by default.
- `development_only`: `brocanth check --deploy` refuses it, as it refuses
  `jobs-memory`.
- Crate: `postgresql_embedded` (theseus-rs). Its `bundled` feature embeds
  the server archive at build time; without it, the archive downloads at
  first start. Precedent: zonky `embedded-postgres` (io.zonky.test, Maven)
  for Java tests.
- Trade-offs: the archive adds tens of MB to a development binary, per
  target platform. PostgreSQL's licence is permissive (BSD-style); check
  the crate's own licence and the archive's source before shipping it.
  Because `check --deploy` refuses the module, a deployed build does not
  carry it.

## 5. Decision 2: the backoffice is a separable service

Owner, Q2: the backoffice is a second service. It is embedded in the
first iterations, and it splits to its own process or machine when
traffic or complexity grows. Access control is RBAC from the start.

### 5.1 What an `AdminPanel` may touch

A tab reaches application data **only through an interface that could
become remote**:

- an async trait (as `AdminData` and `AccountDirectory` are today);
- owned, serialisable arguments and results (`serde`), no borrowed app
  state, no closures over app internals;
- a handle given at build, filled at start: a late cell in `Services`
  (`RolesPanel::late`, `crates/brocanth-auth/src/admin/staff.rs:119`,
  is the prior art). No `OnceLock` global, no `X::installed()`.

**Reason:** in the split, each such trait gets a second implementation, a
client over HTTP to the app (or over the shared database). A tab that
reads a global or an app type cannot move. The 7 `INSTALLED` globals
(§1) are the work list.

### 5.2 Minimal RBAC

Each tab declares its **RBAC resource**: its `Panel::key` (`jobs`,
`cache`, `accounts`). A role grants a level per resource:

| Level | Allows |
|---|---|
| `read` | see the tab (`Action::ViewList`, `ViewDetail`, `ViewPanel`) |
| `edit` | `read`, plus every button and write (`Create`, `Update`, `Delete`, `ActOnPanel`) |
| `owner` | `edit`, plus give or take `read` and `edit` on this tab to other staff |

The model is `role -> resource -> level`, stored as permission strings
`<resource>.<level>` in the `roles` tables that `brocanth-auth::staff`
already owns. `StaffPolicy` maps an admin `Action` to a level and asks
`Staff::allows`. A superuser passes every check, and only a superuser
grants `owner`. **Reason:** this keeps the existing rule "only a superuser
changes roles" (Django's `auth.change_group`, `admin/staff.rs`) and
relaxes it only inside one tab, so an owner cannot reach past its tab.
Today's per-model verbs (`<model>.view|add|change|delete`) map onto
`read` and `edit` for model screens.

Relation to the other crates:

- `superuser` and `roles` (`brocanth-auth::scaffold::staff`) store the
  grants and give the principal (`Staff`). `roles` contributes the roles
  tab, whose resource is `roles` and which only a superuser edits.
- `brocanth_admin::policy::Policy` is the screen gate: may this person
  open this tab, press this button.
- `brocanth-policy` stays the row gate: which rows a list shows and which
  write is `Verified`. `Staff::gate("<resource>.edit")` turns a level into
  a row predicate, so both travel in one `condition()`.

### 5.3 Path to decoupling

What must stay true now, so the split costs a new binary and not a
rewrite:

1. Tabs follow §5.1. No tab names an app type or a global.
2. The admin is mounted under one `Mount` with role `Backoffice`. No app
   page links into the admin except through that mount's setting.
3. The principal comes from a trait (today's `StaffStore`) over the shared
   Postgres session and staff tables, not from the app's request state.
   Both services read the same tables in step one of a split.
4. The admin ships its own assets (it does: `admin.css` is embedded).
5. RBAC lives in the database, not in app code, so both services see one
   set of grants.

The split itself: the CLI writes a second binary, `backoffice`, that
registers `db`, the admin and each module's data implementation. Points
3 and 5 need a database, so an admin with `roles` or `superuser`
requires `PgPool`, and the resolver adds `db` (§4.1).

### 5.4 Mount roles

`Mount` keeps three roles: `Public`, `Backoffice`, `Machine`. `admin` is
`Backoffice`. `resource` (the records API), `storage-routes` and the OAuth
callbacks are `Machine`. analytics, gtag, robots and sitemap read "every
`Backoffice` mount" and stop naming `admin`; i18n-routing keeps every
non-`Public` mount unprefixed.

## 6. Decision 3: settings are declared once, read from layers

Owner, Q3: every module option is settable on the command line as
`--<module>.<option>=value`, and code can set each option. A config
module (TOML, JSON, HCL) keeps the flags and adds a section to its file.
Merging several files comes later.

Most of this exists: `docs/module-flags.md` already gives each declared
setting a flag, an environment variable and, with `config`, a file key.
What is left is the 47 `has("config")` branches (§1).

**Decision 3a: one registration form.** A module declares its settings
once (`Setting`). The scaffold always writes one registration, which
reads the layered settings. The literal `Plugin::new(..)` form and its
`PLUGIN_TRAIT_USE` import go. **Reason:** the two forms are the 23
registration branches, and they exist only because the file was the
layer loader.

**Decision 3b: the layers, lowest to highest:**

1. the crate default (`Settings::default()`);
2. code: the module file `src/modules/<key>.rs`, which the project owns;
3. files, from the `ConfigSource` provider, when one is chosen;
4. the environment, `$<APP>_<MODULE>__<KEY>`;
5. the command line, `--<module>.<option>=value`.

**Reason:** the narrower decision wins (`docs/module-flags.md`): a file is
shared by every deployment, a variable by one container, a flag by one
run. **The environment stays.** Containers and platforms set variables,
not flags.

**Decision 3e: a secret has flags, and no flag carries its value.**
Owner, former open question 3. A flag is visible in `ps` to every user on the
host, so a secret setting gets two pointer flags instead of a value flag:

- `--auth.signing_key_file=/run/secrets/key` reads the value from a file
  (Docker and Kubernetes secrets mount so);
- `--auth.signing_key_env=MY_VAR` renames the variable read, instead of
  the default `$<APP>_AUTH__SIGNING_KEY`.

**Reason:** every setting stays reachable from the command line, and the
value never enters the process list. The settings report shows the
pointer, never the value (`<redacted>`, as today).

**Decision 3c: `ConfigSource` is the Kind.** `config` (TOML) provides it;
a JSON or HCL module is another provider of the same Kind. Cardinality
**one** for now; it becomes **many** when the loader merges several
files. The other layers are core, not a Kind. `ConfigSection` from the
first draft goes: a module's section is derived from its declaration.

**Decision 3d: structural options are build-time.** Owner, former
open question 2. An `Opt` that shapes the build (`mount_at`, `integrate.*`,
hook toggles) stays a constant in the module file. No flag, variable or
file overrides it at run time. The brocanth command line sets it when it
writes the file: `brocanth add admin --admin.mount_at=/backoffice`.
Readers keep `module_ref(..)` to the constant, as i18n-routing does.
**Reason:** the route table test, the i18n exceptions and the robots
rules are generated from the constant and checked by the compiler. A
mount path that differs per deployment would break them silently. So
the two declarations stay: `Opt` (build time, `brocanth` flags) and
`Setting` (run time, binary flags, env, file).

Lists of records (rate-limit rules, robots groups) have no flag form.
They stay in code or in the file (§13, decision 1).

## 7. How it keeps ADR 1

ADR 1 refuses registration the reader cannot see: a distributed slice
filled by whatever links. This model adds none.

- The CLI resolves kinds **once, at generation time**. The output is the
  same explicit chain as today, each plugin named in `src/plugins/mod.rs`.
- The binding is written as a comment over the chain and in `brocanth.toml`
  (`[bindings]`: the provider chosen per Kind and why, for example
  `JobQueue = { provider = "jobs", why = "chosen" }`). `add` and
  `remove` re-resolve it (§13, decision 3).
- Runtime lookups stay typed: `Services::get::<T>()`, by trait handle.
- Spring evaluates conditions at start. The CLI evaluates them and
  `explain` prints them: the conditions report is a command, not a log.

## 8. The resolver

`brocanth-cli` gains one pass between "chosen keys" and "contribute":

1. Collect `provides`, `requires`, `uses` and `contributes` of each chosen
   module.
2. For each Kind with cardinality **one**:
   - No chosen provider and a `requires`: if the registry has one
     non-development provider, add it and print why (§4.1); repeat from
     step 1 for the added module. With several, refuse:
     `error: <key> requires JobQueue` / `help: add one of: jobs,
     jobs-memory`.
   - No chosen provider and only `uses`: bind nothing. The module writes
     its code path without K.
   - One provider: bind it.
   - Two providers: a conflict that names both. Reason: two queues or two
     session cookies are a bug, not a choice.
3. For each Kind with cardinality **many**: collect all, in registry
   order. Zero is not an error.
4. Add an order edge from each provider to each consumer, as `needs` does.
5. Give `Project` the bindings: `provider::<K>() -> Option<&str>` and
   `contributions::<K>()`, which replaces `mounts()`.

`check_module` keeps its two-way rule: a module that declares `uses::<K>`
must ask `project.provider::<K>()`, and a module that asks must declare.

## 9. What stays

- **`needs(key)`** for a true one-to-one dependency: `rsx-tailwind` needs
  `commands` because it writes into a `Slot` only `commands` opens.
- **Slots.** A Slot is where code lands; a Kind is what a module gives.
- **`absent`, `conflicts`, `asked_in`, `part_of`, `recommends`,
  `development_only`, `krate`.** They describe the module, the chooser or
  the build.
- **Renderer branches.** `has("rsx")` (13) and `has("jt")` (3) stay. Each
  branch writes different template text; a `Templates` Kind removes none.
- **A module's own children.** `cache` asking `has("cache-memory")` stays
  inside one crate.

## 10. Mapping: every relation to its new form

| Today | New form |
|---|---|
| `needs("db")` ×8 | `requires::<PgPool>`; the resolver adds `db` (§4.1) |
| `adapts_to("db")` ×9, `has("db")` ×9 | `uses::<PgPool>`, `provider::<PgPool>()` |
| `after("db")` ×11 | implied by `requires`/`uses` |
| `needs(key)` (other) | unchanged for a Slot need; `requires::<K>` when the key provides a Kind |
| `after(key)` (other) | implied by `requires`/`uses`; a hook order becomes a phase; else unchanged |
| `adapts_to(provider key)` | `uses::<K>`: mail on `jobs` -> `uses::<JobQueue>` |
| `adapts_to("config")` ×40, `after("config")` ×17, `has("config")` ×47 | gone: one registration form, layers, `ConfigSource` (§6) |
| `adapts_to("admin")`, `integration("admin", ..)` | `contributes::<AdminPanel>(Tab { resource, .. }, on)` |
| `adapts_to(renderer / own child)`, `recommends`, `conflicts`, `asked_in`, `part_of`, `absent` | unchanged |
| `option(Opt::*)` | unchanged: build time, set by `brocanth` flags (§6, 3d) |
| `option(Opt::mount)` | also `provides::<Mount>` with a role (§5.4) |
| `Setting::secret()` | gains `--<m>.<key>_file` and `--<m>.<key>_env` flags (§6, 3e) |
| `hook(name, what, Hook)` | gains a phase; `Hook::required()` unchanged |
| `development_only` on `jobs-memory`, `cache-memory` | unchanged; they also `provides::<JobQueue/CacheStore>` |
| `Project::has(other key)` | `provider::<K>()` / `contributions::<K>()`; unchanged for needs, renderer, own children |
| `Project::mounts()` | `contributions::<Mount>()`, filtered by role |
| `module_ref(other, field)` | must name an `exports` field; mount reads go through `Mount` |
| `POSTGRES_WITH`, `SCREEN_PATHS`, `MIGRATES` | `uses::<PgPool>` (one pool), an export, `contributes::<ConfigCheck>` |
| `static INSTALLED: OnceLock` ×7 | a late cell in `Services` (§5.1) |
| `after_if_present("<plugin>.<hook>")` | a phase; `after` only inside a phase |
| `StaffPolicy` per-model verbs | levels `read` / `edit` / `owner` per RBAC resource (§5.2) |

## 11. Anti-goals and risks

Anti-goals:

- **No runtime container.** Nothing is injected or discovered at start.
- **No Kind for one provider and one consumer.** That is a renamed
  `needs`. A new Kind names its two providers or two consumers; a
  capability trait counts its planned second backend only when a module
  for it exists.
- **No more than ~12 Kinds.** Each Kind is a word every author learns.
- **No Kinds from outside modules (for now).** A `with_extra` module may
  provide and use core Kinds, not define one.
- **No per-Kind versions** (§3.1).

Risks:

- Two vocabularies during migration. Mitigation: one Kind at a time; the
  PR that adds a Kind removes the old keys.
- Auto-add hides a dependency. Mitigation: the create output,
  `brocanth.toml` and `explain` name each added module and its reason.
- Phases too coarse. Mitigation: `Order::after` inside a phase; a
  cross-phase `after` is a build error.
- Two declarations (`Opt` at build time, `Setting` at run time) can
  confuse an author. Mitigation: `explain` and the docs table mark each
  as "build" or "run".
- RBAC levels too coarse for a big tab. Mitigation: a tab may declare
  more than one resource later; not now.

## 12. Migration plan

Sizes assume one engineer who knows the scaffold crate. Each stage
re-blesses the goldens and reads the diff.

**Stage 1 — one pool, the resolver (2-3 days).** Kind trait, manifest
methods, resolver with auto-add (§4.1), `check_module` rules. Exit:
`has("db")` 9 -> 0 (now `provider::<PgPool>()`), `needs("db")` 8 -> 0,
`after("db")` 11 -> 0; `POSTGRES_WITH`, the auth "own pool" variant and
`[auth] url` are gone; no module but `db` opens a pool; `brocanth create
--jobs` prints `added db`.

**Stage 1b — `db-embedded` (1-2 days, independent).** The module over
`postgresql_embedded`, offered in the wizard, refused by `check --deploy`.
Exit: `brocanth create --db --db-embedded` runs `cargo run` with no
Postgres installed; `check --deploy` reports it as an error.

**Stage 2 — backoffice: `Mount`, `AdminPanel`, RBAC (4-5 days).** Mount
roles; tabs on late cells and async traits; `read`/`edit`/`owner` in
`StaffPolicy` and the roles tab. Exit: `static INSTALLED` 7 -> 0;
`module_ref("admin", ..)` outside `brocanth-admin` 5 -> 0; `has("admin")`
24 -> ≤ 4; each tab declares a resource, checked by `check_module`.

**Stage 3 — settings layers and `ConfigSource` (3-4 days).** One
registration form; secret pointer flags (`_file`, `_env`). Exit:
`has("config")` 47 -> ≤ 3 (inside `config` and the scaffold helper);
`adapts_to("config")` 40 -> 0; `PLUGIN_TRAIT_USE` gone; every run-time
setting has a flag (a secret through `_file` / `_env`), except record
lists.

**Stage 4 — hook phases (2 days).** `Phase` in `brocanth::plugin_order`;
the registrar and `.hook` take it. Exit: cross-plugin
`after_if_present`/`before_if_present` 18 -> ≤ 3; `.after(<middleware
key>)` 6 -> 0; ADR 11 records the phases.

**Stage 5 — capability Kinds and exports (2-3 days).** `JobQueue`,
`CacheStore`, `SessionCookie`, `ConfigCheck`, and AGENTS.md sections as a
contribution point; exports for `auth-screens`. Exit: no caller names a
provider type; each module's AGENTS.md section comes from its
contribution; `MIGRATES`, the `sessions || auth`
line and `SCREEN_PATHS` are gone; `has` on another crate's key 159 ->
≤ 30.

**Stage 6 — `brocanth explain` and docs tables (1-2 days).** `explain`
prints why each module is in, each Kind's provider, each contribution,
each hook with phase and "required", and each setting with its layer. The
same data writes a table on each `docs/modules/*.md` page, and
`docs_check` compares it. Exit: 77 of 77 pages carry a generated table; a
manifest edit without a re-bless fails `docs_check`.

Total: 15-21 days (first draft: 10-13). Stage 1b is new; stages 2 and 3
grew with RBAC and the settings layers.

## 13. Decisions on the former open questions (owner, 2026-11-16)

No question is open.

1. **Lists of records have no flag form.** Rate-limit rules and robots
   groups live in code or in the config file. **Reason:** a record list
   on a command line is a JSON string nobody reads or quotes correctly;
   code and file already hold it with types and line numbers.
2. **The phases are `Identify`, `Guard`, `Cache`, `Encode`, and an
   `after` across phases is a build error.** **Reason:** a cross-phase
   edge is the plugin-name coupling the phases remove. A warning would
   let it come back one hook at a time.
3. **`brocanth check` reads config-check entries from generated code;
   `brocanth.toml` records the bindings** (the provider chosen per Kind,
   and why: chosen, or added for a requirement). **Reason:** the code is
   what runs, so a check on it cannot drift from the binary. The bindings
   are a decision of the generator, and `add`, `remove` and `explain`
   need it without a second resolution that could disagree.
4. **The project `edition` arrives with the first codemod that needs
   it.** **Reason:** a field that nothing reads is a promise with no test.
   The first codemod defines what an edition boundary means.
5. **Modules outside the workspace use the core Kinds and define none,
   for now.** **Reason:** each Kind is a word in the shared vocabulary
   and a seam the resolver and `explain` must know. Opening it waits for
   a real outside module that needs one.
6. **AGENTS.md sections become a contribution point in stage 5.**
   **Reason:** it is a many-to-one seam like the admin tabs, and stage 5
   already adds the other contribution Kinds.
7. **Only a superuser grants `owner`; an `owner` grants `read` and `edit`
   on its own tab only** (§5.2). **Reason:** this keeps the rule "only a
   superuser changes roles" and limits delegation to one tab, so no
   owner can raise itself or reach past its tab.
