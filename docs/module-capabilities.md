# Module capabilities: kinds, providers and contribution points

Status: **proposal** (2026-11-15). Nothing here is built. Base: `8f546fc`.
Read first: PLAN.md §8 and ADR 1, `docs/module-config.md` §9.

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
Bevy phases, Spring conditions **resolved by the CLI at generation time**,
and Phoenix ownership of the output. No runtime discovery.

## 3. The model

### 3.1 Kind, Group, and no Version

A **Kind** is a typed capability: one thing a module gives that another
uses. It is a Rust marker type in `brocanth-scaffold`, so a typo does not
compile. A **Group** is the domain that owns it (`db`, `admin`, `http`).
It names the owner in `explain` and in docs; lookup ignores it.

**No version per Kind.** Reason: Kubernetes needs versions because one API
server serves several versions of a resource and converts between them.
brocanth crates release together, and a project pins one release, so
there is no second version to serve. A generated project records one
**edition** in `brocanth.toml` instead (`edition = "2026-11"`), which
`brocanth upgrade` reads to choose codemods for the files it generated.

**Kinds are generation-time types.** The CLI does not link the core
(`e305bc6`). A Kind names its runtime type as a path string; the generated
code names the real type, and the compiler checks that code.

The initial set (target 8-12, each with at least two providers or two
consumers):

| Kind | Group | Runtime type | Cardinality | Providers | Consumers |
|---|---|---|---|---|---|
| `PgPool` | db | `brocanth_db::Db` | one | `db`; fallback: `jobs`, `cache` (Postgres backends) | auth, flags, oauth-client, audit, analytics, storage, webhooks-in, model |
| `Mount` | http | a `&str` option | many | admin, auth-oauth, oauth-client, resource, storage-routes | i18n-routing, route table test, robots, sitemap, analytics, gtag |
| `AdminPanel` | admin | `impl brocanth_admin::panel::Panel` | many | cache, jobs, flags, audit, analytics, openapi, oauth-client, webhooks | admin |
| `ConfigSection` | config | `[section]` + settings type | many | every module with settings | config |
| `ConfigCheck` | config | a check entry | many | jobs, cache, flags, audit, analytics, oauth-client, webhooks | `brocanth check` |
| `SessionCookie` | http | the session on the request state | one | sessions, auth | oauth-client, auth children, config check (`check.rs:652`) |
| `JobQueue` | jobs | `brocanth_jobs::Queue` | one | jobs, jobs-memory | mail, webhooks, observe |
| `CacheStore` | cache | `brocanth_cache::CacheHandle` | one | cache, cache-memory, cache-redis | none outside `cache` today |

A `Mount` carries a role (`Public`, `Backoffice`, `Machine`), so a reader
asks for "every backoffice mount" instead of naming `admin`.
`CacheStore` fails the two-consumer rule today; it is listed only to be
dropped in stage 3 unless a second consumer appears. `RequestPhase`
(§3.5) is a closed enum, not a Kind.

### 3.2 provides / requires / uses

A module declares what it gives and what it takes:

- `provides::<K>(what)`: this module gives K.
- `provides_fallback::<K>(what)`: this module gives K only when no plain
  provider is chosen (Spring `@ConditionalOnMissingBean`).
- `requires::<K>(why)`: this module cannot be written without K. Like
  `needs`, it implies "after the provider".
- `uses::<K>(what_changes)`: this module writes different code when K is
  present. It replaces `adapts_to` on a provider key.

### 3.3 Contribution points

A Kind with cardinality **many** is a contribution point: many modules
contribute, one collects. Admin tabs, mounts, config sections, config
checks and AGENTS.md sections are contribution points. Tabs already work
so (`Slot::AdminPanels`, `.integration("admin", ..)`); the model gives the
pattern one name and one API.

### 3.4 Exports

A module declares the fields of its module file that other modules may
read: `.exports("mount_at", "where the backoffice is served")`. A
`module_ref(other, field)` on another module's key must name an export.
`check_module` enforces it. Reason: today any field of any module file is
public to every scaffold. An export is the contract; the rest can change
without notice (NestJS `exports`). `auth-screens` exports its sign-in
paths, so rate-limit stops copying them.

### 3.5 Hook phases

A request hook declares a **phase**, and orders itself inside it:

| Phase | Today's hooks |
|---|---|
| `Identify` | proxy-headers, session, i18n negotiate, method-override |
| `Guard` | rate-limit count, csrf check, auth |
| `Cache` | conditional-get, route cache |
| `Encode` (response) | compression, security headers, RateLimit headers |

The core runs phases in this order; `Order::after` / `before` stays for an
edge inside one phase. Reason: `after_if_present("proxy-headers.read")`
(`crates/brocanth-middleware/src/rate_limit/mod.rs:643`) names a plugin,
so a second proxy reader must edit every such list. A phase names a stage
of the request (Rails `on_load`, Bevy `SystemSet`). ADR 9 does not change:
a hook that stops the chain still lets every later hook see
`after_dispatch`, whatever its phase.

`HookSource::REQUIRED` does not change either. Required means "may not be
turned off"; a phase means "when it runs". The manifest's `.hook(..)`
gains the phase, so `explain` and the docs show both without a build.

### 3.6 Manifest sketch

```rust
// brocanth-scaffold/src/kind.rs
pub trait Kind: 'static {
    const NAME: &'static str;        // "PgPool"
    const GROUP: &'static str;       // "db"
    const RUNTIME: &'static str;     // "brocanth_db::Db", for generated code
    const CARDINALITY: Cardinality;  // One | Many
}
kind!(PgPool, group = "db", runtime = "brocanth_db::Db", One);

// crates/brocanth-db/src/scaffold.rs
Manifest::new("db", "one Postgres pool ..")
    .provides::<PgPool>("the pool [db] opens, published before_start")

// crates/brocanth-jobs/src/scaffold.rs
Manifest::new("jobs", "..")
    .provides_fallback::<PgPool>("its own pool from [jobs] url, when no db")
    .contributes::<AdminPanel>("a jobs tab", true)
    .contributes::<ConfigCheck>("deploy/migrate for [jobs]", true)

// crates/brocanth-auth/src/scaffold.rs: replaces POSTGRES_WITH and 3 adapts_to
Manifest::new("auth", "..")
    .uses::<PgPool>("accounts live in Postgres, on the shared pool; without it, in memory")
    .provides::<SessionCookie>("the signed session cookie")

// in contribute(): ask the Kind, not the keys
match project.provider::<PgPool>() {
    Some(_) => s.file("src/plugins/auth/mod.rs", postgres_plugin_rs(project)),
    None => s.file("src/plugins/auth/mod.rs", plugin_rs(project)),
}
```

### 3.7 Generated output (auth with `jobs`, no `db`)

`src/plugins/mod.rs`, the chain the CLI writes:

```rust
    // PgPool <- jobs (fallback: no `db`). `brocanth explain PgPool`
    .plugin_from_config::<crate::plugins::jobs::Jobs>("jobs")
    // uses PgPool
    .plugin_from_config::<crate::plugins::auth::Auth>("auth")
```

`src/plugins/auth/mod.rs`, in `before_start`:

```rust
let db = lc.services().get::<brocanth_db::Db>().cloned().ok_or_else(|| {
    PluginError::new("auth needs the Postgres pool")
        .help("register the plugin that provides PgPool (here: jobs) before auth")
})?;
```

With `db`, the comment says `PgPool <- db` and `db` registers first. The
auth file is the same in both cases, and `[auth] url` goes: one pool per
project. Cost: `jobs` and `cache` must publish their pool as
`brocanth_db::Db`; today `jobs` publishes only its `Queue`.

## 4. How it keeps ADR 1

ADR 1 refuses registration the reader cannot see: a distributed slice
filled by whatever links. This model adds none.

- The CLI resolves kinds **once, at generation time**. The output is the
  same explicit chain as today, each plugin named in `src/plugins/mod.rs`.
- The binding is written as a comment over the chain and in `brocanth.toml`
  (`[bindings] PgPool = "db"`). `add` and `remove` re-resolve it.
- Runtime lookups stay typed: `Services::get::<brocanth_db::Db>()`, as
  flags, cache and oauth-client read the pool today.
- Spring evaluates conditions at start. The CLI evaluates them and
  `explain` prints them: the conditions report is a command, not a log.

## 5. The resolver

`brocanth-cli` gains one pass between "chosen keys" and "contribute":

1. Collect `provides`, `provides_fallback`, `requires`, `uses` and
   `contributes` of each chosen module.
2. For each Kind with cardinality **one**:
   - No provider and a `requires`: refuse, like `needs` today:
     `error: auth-screens requires SessionCookie` /
     `help: add one of: sessions, auth`. The wizard ticks the default
     provider (the first plain provider in the registry).
   - One plain provider: bind it. Fallback providers become consumers, so
     `jobs` with `db` reads the `db` pool. No plain, one fallback: bind it.
   - Two plain providers: a conflict that names both. Reason: two pools or
     two session cookies are a bug, not a choice. A Kind that needs a real
     choice reads a config option. No Kind needs one today.
   - Two fallbacks, no plain: bind the first in registry order, and say so
     in `explain` (open question 1).
3. For each Kind with cardinality **many**: collect all, in registry
   order. Zero is not an error.
4. Add an order edge from each provider to each consumer, as `needs` does.
5. Give `Project` the bindings: `provider::<K>() -> Option<&str>` and
   `contributions::<K>()`, which replaces `mounts()`.

`check_module` keeps its two-way rule: a module that declares `uses::<K>`
must ask `project.provider::<K>()`, and a module that asks must declare.

## 6. What stays

- **`needs(key)`** for a true one-to-one dependency: `rsx-tailwind` needs
  `commands` because it writes a subcommand into a `Slot` only `commands`
  opens. A Kind with one provider and one consumer is a renamed `needs`.
- **Slots.** A Slot is where code lands; a Kind is what a module gives.
- **`absent`, `conflicts`, `asked_in`, `part_of`, `recommends`, `option`,
  `krate`.** They describe the module, the chooser, or the build.
- **Renderer branches.** `has("rsx")` (13) and `has("jt")` (3) stay. Each
  branch writes different template text; a `Templates` Kind removes none.
- **A module's own children.** `cache` asking `has("cache-memory")` stays
  inside one crate. It is not a boundary.

## 7. Mapping: every relation to its new form

| Today | New form | Example |
|---|---|---|
| `needs(key, why)` | unchanged for a one-to-one Slot need; `requires::<K>` when the key is one provider of a Kind | `needs(db)` ×8 -> `requires::<PgPool>` |
| `after(key)` | implied by `requires`/`uses`; a hook order becomes a phase; else unchanged | `after("db")` ×11 -> gone; `after("config")` ×17 -> implied by `ConfigSection` |
| `absent(what, why)` | unchanged | |
| `adapts_to(provider key)` | `uses::<K>` | auth's `jobs`/`cache`/`db` -> `uses::<PgPool>` |
| `adapts_to("config")` | `contributes::<ConfigSection>` | ×40 |
| `adapts_to("admin")`, `integration("admin", ..)` | `contributes::<AdminPanel>(what, on)` | cache, jobs, flags |
| `adapts_to(renderer / own child)` | unchanged | `rsx`, `cache-memory` |
| `recommends(key, why)` | unchanged | |
| `conflicts(key, why)` | unchanged; two plain providers of a "one" Kind conflict without a line | |
| `asked_in`, `part_of`, `because` | unchanged | |
| `option(Opt::text/bool/..)` | unchanged | |
| `option(Opt::mount)` | also `provides::<Mount>` with a role (§11 Q2) | admin `mount_at` |
| `hook(name, what, Hook)` | gains a phase; `Hook::required()` unchanged | |
| `Project::has(other key)` | `project.provider::<K>()` / `contributions::<K>()`; unchanged for needs, renderer, own children | 159 -> target ≤ 40 |
| `Project::mounts()` | `project.contributions::<Mount>()` | i18n-routing |
| `module_ref(other, field)` | must name an `exports` field; mount reads go through `Mount` | robots -> admin `mount_at` |
| hand lists (`POSTGRES_WITH`, `SCREEN_PATHS`, `MIGRATES`) | `uses::<PgPool>`, an export, `contributes::<ConfigCheck>` | |
| `static INSTALLED: OnceLock` ×7 | a typed handle per app (§8) | `CacheHandle::installed()` |
| `after_if_present("<plugin>.<hook>")` | a phase; `after` only inside a phase | rate-limit after proxy-headers |

## 8. Runtime side

The generated `plugins::admin_panels` lists each tab behind its module's
`integrate.admin` flag. That explicit list stays. Today each tab calls
`XPanel::installed()`, a process-wide `OnceLock` that the plugin fills in
`before_start`, because the admin builds tabs in `mount`, which runs
first. Proposal: `Services` gains a late cell per type. The build creates
it empty, the provider fills it in `before_start`, the tab holds it.

```rust
// generated admin_panels, with a Late handle instead of a global
config = config.panel(brocanth_cache::admin::CachePanel::new(
    services.late::<brocanth_cache::CacheHandle>(),
));
```

An empty cell at request time is a 503 with a `help:` line. Reason: the
value is per `App`, not per process, so two apps in one test do not share
it, and the type is the key (Bevy resources). There is no "every `dyn
Panel` in Services" lookup: the generated list is the ADR 1 manifest.

## 9. Anti-goals and risks

Anti-goals:

- **No runtime container.** Nothing is injected or discovered at start.
- **No Kind for one provider and one consumer.** That is a renamed
  `needs`. A new Kind names its two providers or two consumers.
- **No more than ~12 Kinds.** Each Kind is a word every author learns.
  Past twelve, the words cost more than the keys they hide.
- **No Kinds from outside modules (for now).** A `with_extra` module may
  provide and use core Kinds, not define one.
- **No per-Kind versions** (§3.1).

Risks:

- Two vocabularies during migration. Mitigation: one Kind at a time; the
  PR that adds a Kind removes the old keys.
- A fallback provider hides a missing `db`. Mitigation: the chain comment
  and `explain` say "fallback".
- Phases are too coarse, and cross-phase edges come back. Mitigation:
  `Order::after` inside a phase; a cross-phase `after` is a build error.
- `ConfigSection` rendering grows into a template engine. Mitigation:
  stage 3 only; stop at the first module that needs a special case.

## 10. Migration plan

Sizes assume one engineer who knows the scaffold crate. Each stage
re-blesses the goldens and reads the diff.

**Stage 1 — `Mount`, `PgPool`, `AdminPanel`, the resolver (3-4 days).**
Kind trait, manifest methods, resolver pass, `check_module` rules,
`Late<T>` in `Services`. Exit:
- `POSTGRES_WITH` is gone; `[auth] url` is gone.
- `module_ref("admin", ..)` outside `brocanth-admin`: 5 -> 0.
- `has("db")`: 9 -> ≤ 2; `.after("db")`: 11 -> 0; `has("admin")`: 24 -> ≤ 4.
- `static INSTALLED: OnceLock`: 7 -> 0.

**Stage 2 — hook phases (2 days).**
`Phase` in `brocanth::plugin_order`; the registrar and the manifest `hook`
take it; the core sorts by phase, then by `Order`. Exit:
- `after_if_present`/`before_if_present` on another plugin: 18 -> ≤ 3.
- `.after(<middleware key>)`: 6 -> 0; `.after("auth")`: 8 -> 0 or justified.
- ADR 10 records the phases.

**Stage 3 — the remaining `adapts_to` (4-5 days).**
`ConfigSection`, `ConfigCheck`, `SessionCookie`, `JobQueue`, `CacheStore`
(drop the last if it keeps one consumer). Exports for `auth-screens`. Exit:
- `adapts_to("config")`: 40 -> 0; `project.has("config")`: 47 -> ≤ 10.
- `MIGRATES`, the `sessions || auth` line and `SCREEN_PATHS` are gone.
- `project.has` on another crate's key: 159 -> ≤ 40.

**Stage 4 — `brocanth explain` and generated docs tables (1-2 days).**
`brocanth explain [<key> | <Kind>]` prints why each module is in the
project (asked, needed by, default provider of), each Kind's provider,
each contribution, and each hook with its phase and "required". The same
data writes a Provides / Requires / Uses / Contributes / Exports / Hooks
table on each `docs/modules/*.md` page, and `docs_check` compares it.
Exit: 77 of 77 pages carry a generated table; a manifest edit without a
re-bless fails `docs_check`, so the `auth.md` drift cannot recur.

Total: 10-13 days.

## 11. Open questions for the owner

1. `jobs` and `cache` as fallback providers of `PgPool`, or `db` as the
   only provider that every Postgres module `requires`? The second is
   simpler; it costs one more module in small projects.
2. `Mount` roles: is `Public` / `Backoffice` / `Machine` the right set?
   With roles, analytics, gtag, robots and sitemap read "every backoffice
   mount" and stop naming `admin`. Is `resource` a backoffice mount?
3. `ConfigSection`: should the Scaffold render a setting as a file entry
   or a literal (one place for the 47 `has("config")` branches), or does
   each module keep its own branch?
4. Phases: are the five names right, and is a cross-phase `after` a build
   error or a warning?
5. `brocanth check` reads `ConfigCheck` entries from where:
   `brocanth.toml`, or generated code? And does `brocanth.toml` record
   `[bindings]`, or does `explain` recompute them each time?
6. Edition: add `edition` now, or with the first codemod that needs it?
7. Outside modules: core Kinds only, as proposed, or may an outside crate
   define a Kind?
8. AGENTS.md sections as a contribution point: in stage 3 or later?
