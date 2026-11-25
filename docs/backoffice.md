# The backoffice

What Django's admin and PocketBase's dashboard contain in a default
install, which module here owns each part, and what is missing.

The seam is `brocanth_admin::panel::Panel`: a tab that renders its own body
and is handed the admin's chrome. It lives in the crate that owns the data,
behind an `admin` feature that is off, because `brocanth-admin` keeps its
dependencies few and intends to keep them so. `brocanth-jobs` is the worked
example.

An application adds a page of its own the same way, without a crate:
`AdminConfig::page(key, label, closure)` is a `Panel` from a closure.
That is Django's `AdminSite.get_urls()`: the page gets the layout, a tab
(or none, with `Page::hidden`), the policy as `ViewPanel` and
`ActOnPanel`, and every path under its key. See "Extra pages" in
`docs/admin-customization.md`.

The four model screens go through the entity's row policy. §2a says how.

To change how the admin looks — titles, a theme, one screen for one
model, the whole layout — see `docs/admin-customization.md`. To start
from the shipped screen instead of an empty file,
`brocanth admin eject <screen> [--model <key>]` copies it into the
project and installs it (level 8 there). After an upgrade,
`brocanth admin diff` compares each copy with the shipped screen of the
installed version. The brand and the user links are blocks of their own
(`Views::with_brand`, `Views::with_userlinks`), so a logo or a "Log out"
link needs no copy of the layout.

The backoffice as a service of its own, with the tab data contract and
the second binary, is §7.

Measured by reading each crate's public surface at `54ff827`. Where this
says a tab cannot be built, somebody read the trait.

---

## 1. What a default Django install gives you

`django-admin startproject` plus `django.contrib.admin`, `.auth` and
`.sessions` in `INSTALLED_APPS` — which is what `startproject` writes.

| | Screen |
|---|---|
| J1 | The index: apps grouped, models under them, Add and Change beside each |
| J2 | A model's list: `list_display` columns, `list_filter` sidebar, a search box, sortable headers, pagination |
| J3 | The actions dropdown over selected rows; "delete selected" ships |
| J4 | A change form from the model: a widget per field type, `fieldsets`, `readonly_fields` |
| J5 | Inlines — a related model edited inside its parent's form |
| J6 | Delete, behind a confirmation that lists what cascades |
| J7 | **History**: every change the admin made to an object — who, when, which fields. A `LogEntry` row per save, and a History button on every object |
| J8 | Users: list, add, change, with a password field that is a hash and a separate change-password form |
| J9 | Groups, and a permissions picker |
| J10 | Per-model `add` / `change` / `delete` / `view` permissions, enforced by the admin itself |
| J11 | Login, logout, and "change my password" |
| J12 | `date_hierarchy`: a date drill-down above a list |

## 2. What a default PocketBase install gives you

One binary, `./pocketbase serve`, superuser dashboard at `/_/`.

| | Screen |
|---|---|
| P1 | Collections: create and edit a collection, which is a schema editor — field name, type, required, unique |
| P2 | Records: browse, filter, sort, create, edit, delete, per collection |
| P3 | A filter expression box over the record list |
| P4 | API rules per collection: five expressions, one per operation, saved with the collection |
| P5 | Users and superusers, as collections with auth turned on |
| P6 | Logs: requests, with method, status, duration, and a filter |
| P7 | Settings: application name and URL, SMTP, S3, token durations |
| P8 | Backups: make one, download it, restore it |
| P9 | Files, served from storage, with thumbnails |
| P10 | Realtime: how many clients are connected |
| P11 | Import and export collections as JSON |
| P12 | An API preview generated from the collections |

---

## 2a. Row-level rules on the model screens

PLAN.md §12 item 9. Two policies answer two questions, and both must say
yes:

* `brocanth_admin::Policy` (`with_policy`) answers *may this person open
  this screen*. It is `DenyAll` until the application installs one.
* `brocanth_policy::Policy`, registered per model with
  `AdminConfig::resource(..)`, answers *which rows*. A model with no resource
  is `403` on every screen and is not in the navigation.

```rust
struct Posts;

impl AdminResource for Posts {
    type Entity = Post;            // a brocanth_policy::Entity
    type Subject = Staff;          // what the principal hook attached
    type Policy = PostRules;       // no default: omit it and this does not compile
    const MODEL: &'static str = "post";
    const DIALECT: Dialect = Dialect::Postgres;
    fn policy(&self) -> &PostRules { &PostRules }
}

AdminConfig::new().with_policy(staff_only).resource(Posts)
```

| Screen | Rule | Where it is applied | Refused |
|---|---|---|---|
| list, and its total | `list` | `AND <condition>` in the page query **and** the count query | `403` when the rule is `Never`; otherwise the rows simply are not there |
| detail | `view` | `WHERE pk = ? AND <condition>` | `404`, the same answer as a missing row |
| edit form, delete confirmation | `update`, `delete` | the same guarded select | `404` |
| update, delete | `update`, `delete` | an admission, then a `Verified` whose condition guards the write | `404` before the write; `409` when the write changes zero rows |
| form fields | `writable_on_create` on a create, `writable` on an update | a field the mask refuses is drawn read-only; a POST that sends one is refused before any row is read | `403` |
| foreign-key labels and pickers | the target model's `view` and `list` | the same scopes | the stored value is shown; the picker leaves the record out |
| create | `create` | only `Always` and `Never` are decided | `403` for anything else |

The data source receives a `Scope` on `count`, `list`, `fetch`, `update` and
`delete`, and must `AND` its condition into the statement. `update` and
`delete` report rows changed, and zero is a failure, never success.
`brocanth-admin-model` does this over SeaORM: reads splice the condition
with `Expr::cust_with_values`, and writes read the row with the condition
and `FOR UPDATE` in a transaction, then write it through the `ActiveModel`
so the hooks still run.

The in-memory `Fixture` cannot run SQL, so it refuses a condition rather
than ignore it. A test or a development build over it calls
`AdminConfig::unscoped_rows_unsafe_for_development()`, which lets a model
with no resource show every row.

### Under Postgres row-level security

On a table under forced row-level security (`brocanth-rls`), a statement
that does not run as a database identity sees no row. So the data source
runs each request as the staff member's identity
(docs/policy-rls-bridge.md, decision 13):

```rust
let data = Models::new(orm.connection().clone()).as_identity(crate::plugins::rls::staff);
```

- `as_identity(who)` takes a function from the request's `Principal` to a
  `DbIdentity`. `AdminData::for_request` then gives each request a copy of
  the source bound to `who(principal)`.
- Each read and each write runs in a transaction that assumed the
  identity first (`SET LOCAL ROLE`, `set_config`). A save with inline rows
  runs in one such transaction.
- The per-tab RBAC and the row policy do not change. The admin still
  `AND`s the `Scope` into every statement. The database adds its own
  policy to the same statement: two checks, one statement.
- `AdminData::check` runs in `Admin::try_new`. It builds the identity of
  an anonymous principal and checks it as `Tx::assume` would: a bad role
  name or key fails when the admin is built. A generated project builds
  the admin over the same source in `AdminSettings::configure`, so it fails
  at start, not on the first request.

A generated project with `admin-model`, `rls` and `policy` writes this
line. `crate::plugins::rls::staff` is the policy's `Visitor` with `admin`
set, as `brocanth_rls::identity(&app_user(), &visitor)`. A table the admin
shows needs a grant to `app_user` and a policy that admits staff, or the
admin sees no row of it. `crates/brocanth-admin-model/tests/rls.rs` tests
it on a forced table, with the identity and without it.

**Open.** A conditional create rule needs the proposed row as a one-row CTE
(`brocanth_resource::sql::create_admission`) and typed values the admin's
string form does not have yet. The edit and delete links on a list row are
decided per subject, not per row, so a row the update rule refuses still
shows the link and its form answers `404`.

## 3. Who owns what here

| Feature | Module | Tab | State |
|---|---|---|---|
| J1, J2, J4, J6 | `admin` + `model` | the four screens | **done** |
| J3 actions over selected rows | `admin` | the list screen | **missing** |
| J5 inlines | `admin` | the form screen | **missing** |
| J7 history / audit | `events` | **Changes** | **blocked** — see below |
| J8 users | `auth` | **Accounts** | **blocked** — see below |
| J9, J10, P4 rules | `policy` | **Rules** | buildable, read-only. The rules already *apply* on every model screen: §2a |
| J11 login | `auth` | not a tab; a route | **missing** |
| J12 date hierarchy | `admin` | the list screen | missing, small |
| P1 schema editor | — | — | **out of scope.** A collection here is a Rust type checked at compile time. §4.8: if scaffolding produces code you would not write by hand, the framework below it is wrong — and a schema editor writing Rust at run time is that, twice over |
| P2, P3 records and filters | `admin` + `model` | the list screen | **done**, except the filter expression |
| P5 superusers | `auth` + `policy` | **Accounts** | with J8 |
| P6 request logs | `logging` | **Requests** | **blocked** — see below |
| P7 settings | `config` | **Settings** | buildable; it needed redaction, not a merged document |
| P8 backups | — | — | missing, and nothing owns it |
| P9 files | `storage` | **Files** | **blocked** — see below |
| P10 realtime clients | `realtime` | **Realtime** | **buildable today** |
| P11 import/export | — | — | out of scope with P1 |
| P12 API preview | `openapi` | **API** | **buildable today** |
| — | `jobs` | **Background jobs** | **done** |
| — | `cache` | **Cache** | partly buildable |

## 4. The tabs that cannot be built, and why

Four, not five: `config` turned out not to be one of them.

Each of these is a missing *read*, not a missing screen. The panel is an
afternoon; the read is the work.

**Accounts (`auth`).** `AuthStore` can fetch an account by id and by email
and cannot list them. There is no `accounts(limit, offset)` and no count.
The single most used screen in Django's admin needs one method this trait
does not have. It also needs a second: `MemoryStore` is the only
implementation in the workspace, so a tab over it shows accounts that
vanish on restart.

**Files (`storage`).** The `Storage` trait has `put`, `get`, `meta`,
`exists`, `delete` and the variant pair. It cannot enumerate. An address is
a content hash, so there is nothing to list *from* — a file browser needs
an index the crate deliberately does not keep.

**Requests (`logging`).** The logger writes a line per request to stderr.
Nothing retains anything, so there is no table to show. PocketBase's logs
tab is backed by a table; this would need one.

**Settings (`config`).** ~~The loader keeps no merged document.~~ Wrong,
and wrong the same way the test counts in `framework-gaps.md` were: I
grepped `lib.rs`, saw no `pub fn`, and called the surface empty. `lib.rs`
re-exports; the API is in `config.rs` and `value.rs`. `Loader::load`
returns a `Config` whose `root: Value` **is** the deep-merged tree, every
node carrying a `Provenance` — file, line and column, or `$VAR`, or a
plugin default — plus `Value::shadowed()`, the chain of layers it beat.
`Dump` already renders it.

What was actually missing is **redaction**, which is a sharper gap than
the one it replaces: without it the tab prints the password out of a
database URL, and so does `Config::dump()` into a terminal, a ticket or a
log.

**Changes (`events`).** `ChangeSink` is implemented by `brocanth-realtime`
and the seam works. But `brocanth-model` holds no sink, so no row written
through the ORM publishes anything, and a history tab would show an empty
table for ever. This is §G4 and it blocks the one Django screen with no
equivalent anywhere else.

## 5. The three that can be built now

**Realtime.** `Broker::stats()` returns clients, subscriptions, topics, and
the indexed/scanned split that says whether the subscription index is
working. That is a dashboard.

**API (`openapi`).** The document is rendered at startup and
`document_of(services)` hands it over. A tab that lists operations, with
each path, verb and summary, needs no new read.

**Cache.** `Cache` has no stats, but it has `clear(namespace)` and
`purge_expired(limit)`. A tab with two buttons and no numbers is honest and
useful; the numbers need a `stats()` the trait does not have.

## 6. Order

1. The three that need nothing: **Realtime**, **API**, **Cache**.
2. The two reads that are one method each: `AuthStore::accounts` and a
   merged document on the config loader. Then **Accounts** and **Settings**.
3. `brocanth-model` holding a sink, which is §G4. Then **Changes**, and
   Django's history screen exists here.
4. `storage` keeping an index, or deciding it should not. Then **Files**,
   or a written reason there is none.
5. `logging` keeping rows, which is a table and a retention policy before
   it is a tab.

Actions over selected rows (J3) and inlines (J5) are `brocanth-admin`'s own
work and belong with the four screens rather than with a panel.

## 7. The backoffice as a service of its own

Designed 2026-11-21. The owner fixed the frame
(`docs/module-capabilities.md` §5): the backoffice is a second service,
embedded first and split later; RBAC per tab (`read`, `edit`, `owner`)
from the start; settings CLI first; Postgres only, with the staff
member's database identity on every read and write
(`docs/policy-rls-bridge.md`, decision 13). This section says how a tab
reaches its data so that it can move, and how a project builds the
second binary. v1 keeps the data in-process: the second binary links the
application's crates.

### 7.1 The tab data contract

Crate `brocanth-backoffice`. The admin's four model screens reach their
rows through it.

**D1. A contract per kind of tab, the model screens first.**
`TabData` has `registry`, `check`, `call` and `begin`. Reason: the model
screens are the only tabs whose data is generic. A panel's data belongs
to its crate (jobs, cache, roles), so each panel gets its own trait in
that crate, later.

**D2. One envelope of owned serde types.** `Call { caller, op }`, where
`op` is a read (`Count`, `List`, `Fetch`, `FetchMany`) or a write
(`Insert`, `Update`, `Delete`). The answer is `Count`, `Rows`, `Row`,
`Key` or `Changed`. Nothing borrows. Reason: a wire adds a transport and
a codec, not a type. A JSON loopback in the crate encodes every call and
every answer, and the screen tests run the admin through it.

**D3. A call names its model by key.** The data side finds the model in
its own registry. Reason: the caller never names a table or a column set
for the data side to run.

**D4. The row scope travels compiled.** `Scope::Where` carries the
`brocanth_policy::Condition`: SQL text, typed binds, dialect. The admin
compiles it from the row policy, as it does today. Reason: the
`AdminResource` is part of the admin's configuration, and a copy on the
data side would drift. Consequence: the data side runs the condition as
its own code. The binds are parameters, never text, but a wire server
must accept a call only from an authenticated backoffice service.

**D5. The identity travels as a `Claim`, on each call.** `Caller` holds
the tab (the RBAC resource), the level the call needs (`read` for a
read, `edit` for a write) and the `Claim`: subject, superuser, grants.
The admin side builds the claim from the request's `Principal` with a
function. A project with `superuser` reads `brocanth_auth::Staff`. The
data side gives `Principal::new(&claim)` to `AdminData::for_request`, so
a source with `as_identity` runs the call as the staff member's database
identity. Reason: a `Principal` is an opaque `&dyn Any` and cannot cross
a process. The claim is the part that RBAC and RLS read. Consequence: an
identity function on the data side reads `is_anonymous()` or
`downcast::<Claim>()`, not the session type. The generated
`rls::staff` reads `is_anonymous()` only.

**D6. The data side checks the grant when it is told to.**
`InProcess::enforcing_grants()` refuses a call whose claim does not hold
`<tab>.<level>` (a higher level holds the lower ones; a superuser holds
every level). The admin's `Policy` still gates every screen first. In
one process the second check repeats the first, so it is off by
default. A wire server must turn it on. Reason: a server must not trust
the caller's RBAC decision.

**D7. Errors are data, with a `help:` line.** `TabError` has a kind
(`UnknownModel`, `Invalid` with the field errors, `Refused`, `Backend`,
`Protocol`), a message and a help line. It converts to `DataError` and
back, so a form still shows each message beside its field.

**D8. A transaction is owned.** `TabData::begin` returns a
`Box<dyn TabTransaction>` that borrows nothing. The in-process side
keeps the source and the transaction that borrows it inside one pinned
async block. Reason: a wire server keeps a transaction open between two
requests, so it cannot borrow from the stack of one call. The pinned
block gives that to every `AdminData`, with no change to the trait.

**D9. One configuration line puts the admin on the contract.**
`AdminConfig::data_through(f)` maps the data source before
`Admin::try_new` uses it, for the start-time check and for every
request. The `backoffice` module writes the line into
`plugins::admin_panels`, a generated file:
`config.data_through(crate::plugins::backoffice::contract)`. Reason: an
existing admin file needs no edit, and the embedded and the separate
modes serve through the same contract. `brocanth-admin` gains no serde
dependency: an off-by-default `serde` feature derives the types.

### 7.2 The second binary

**B1. `src/bin/backoffice.rs`, written by the `backoffice` module.** It
includes the application's own modules with `#[path]` (`app`, `pages`,
`plugins`, `modules`, and `commands` when there is one) and serves.
Reason: it links the models, the policies and every plugin (the pool,
auth, sessions) with no library split. The cost is a second compile of
the application crate. Not a subcommand: a project without `commands`
has none, and a binary of its own is its own container. Its tests do
not run a second time (`test = false`).

**B2. One code path, two roles.** Both binaries call `app::build` and
`app::serve`: one plugin chain, one route table, one admin. A server
wrap, `plugins::backoffice::Door`, asks a `brocanth_backoffice::Gate`
and answers 404 outside `mount_at` when
the process runs as the backoffice, and 404 inside `mount_at` when the
application runs with `embedded = false`. The second binary sets its
role before it serves. Reason: a second router would drift, and the
mount role `Backoffice` (§5.4) is already the boundary. The role is a
process-wide value, set once by `main`: it says which binary this is,
and it carries no data, so §5.1 does not apply to it.

**B3. It shares by linking.** Models, policies, the `db` pool, auth and
sessions come from the same `register!` chain. The two processes read
one database, so staff and grants are one set (§5.3, points 3 and 5).
`backoffice` does not require `db`: it adds no database of its own.

**B4. Two run-time settings in `[backoffice]`.** `addr`: where the
second binary listens (`127.0.0.1:3001`). `embedded`: whether the
application binary serves `mount_at` (`true`). Each has a flag
(`--backoffice.addr`, `--backoffice.embedded`), a variable and a file
key. Both binaries take every flag of the project, so one configuration
serves both. `mount_at` stays a build-time choice in
`src/modules/admin.rs`.

**B5. A bad configuration fails at start.** The plugin refuses an
`addr` that is not a socket address in `register`, in both binaries.
The contract's `check` runs in `Admin::try_new`, which
`AdminSettings::configure` calls: it runs the source's own check (the
identity of decision 13) through the contract.

**B6. To split:** run the application with
`--backoffice.embedded=false`, run `backoffice` beside it, and route
`mount_at` to it at the proxy. One host keeps one session cookie for
both.

### 7.3 Built (v1, 2026-11-21)

- `crates/brocanth-backoffice`: `contract` (the types and `TabData`),
  `client` (`Contracted`), `server` (`InProcess`, with the owned
  transaction of D8), `json` (the loopback), `gate` (`Gate`, `Role`,
  `check_addr`), `scaffold` (the `backoffice` module).
- `brocanth-admin` and `brocanth-policy` gained an off-by-default `serde`
  feature, and `AdminConfig::data_through`.
- `brocanth-scaffold` gained `Scaffold::app_binary` and `APP_MODS`. The
  generator writes `src/main.rs`'s `mod` lines into the binary, lists it
  in `Cargo.toml` with `test = false`, and keeps it as an owned file.
  `testing::build` builds such a binary with `cargo build --bins`,
  because `cargo test` does not.
- Tests: `tests/contract.rs` (each type survives JSON; the contract
  answers as the source; transactions; the claim reaches the source;
  grants; the source's check at build), `tests/screens.rs` (`TestClient`:
  RBAC per tab through the contract and the codec; the data side refuses
  a write a lax policy let through), `tests/identity.rs` (Postgres, a
  forced table: each call, each transaction and each list runs as the
  identity of the claim), `tests/scaffold.rs` (three shapes, built with
  `BROCANTH_BUILD=1`).

`brocanth-pages`'s `html!` normalises the calling file's path: through
`#[path]` a page arrives as `src/bin/../pages/about.rs`, which named a
module under `crate::bin`. The `everything` shape found it.

Found while building: the transaction borrow (D8). `AdminData::begin`
borrows its source, and the source bound to a claim is a new `Arc`. So
no `TabData` could return a transaction without a change to the trait,
or the pinned block that D8 describes.

### 7.5 The wire (designed 2026-11-24)

The contract of §7.1 over HTTP. The application serves it; the
backoffice binary calls it. Module `brocanth_backoffice::wire`, behind
the `wire` feature (on by default).

**W1. One POST path, one envelope, the loopback's codec.** The
application answers `POST /_backoffice/wire`. The body is an `Ask` in
JSON: `Hello`, `Call(Call)`, `Begin(Claim)`, `InTx { tx, caller, write }`,
`Commit { tx }`, `Rollback { tx }`, or `Panel { tab, claim, call }`
(§7.6). The answer is `Result<Reply, TabError>` in JSON, status 200. A
transport problem has its own status: 401 for a request that is not
signed right, 400 for a body that does not decode, 405 for another verb.
Reason: the types are the contract's (D2), and an error is data (D7). One
handler and one codec cannot drift, and `Json` already proves each type
survives it.

**W2. A server wrap, mounted only when `[backoffice] wire = true`.** The
generated `Door` answers the wire's path before the request hooks run.
With `wire = false` (the default) the path is not served, and the page
table answers it. Reason: a machine call needs no session, no CSRF token
and no locale. A route in the page table would run every request hook
first. Nothing listens unless asked (PLAN.md §8). The backoffice binary
never serves the wire: it answers only the admin's mount (B2).

**W3. One service secret, on both sides, checked at start.**
`[backoffice] service_secret` is a secret setting: no value flag, only
`--backoffice.service_secret_file=PATH` and
`--backoffice.service_secret_env=VAR`, and `<redacted>` in `Debug`. It is
required when `wire` is true or `data_url` is set, and it has at least 32
bytes after the trailing whitespace of a file is cut. The `Backoffice`
plugin refuses a missing or short one in `register`, in both binaries.
Reason: the owner's rule that a bad configuration fails at start, and
secrets that never reach `ps`.

**W4. Each request is signed with HMAC-SHA256; replays are refused.**
The client signs `brocanth-backoffice/1`, the time in Unix seconds, a
random 128-bit nonce, the path and the body, and sends the time, the nonce
and the signature as headers. The server recomputes the signature and
compares it in constant time (`Mac::verify_slice`). It refuses a time more
than 60 seconds away from its own clock, and a nonce it saw inside that
window. It keeps at most 65 536 nonces; when the set is full it answers
503 and does not forget one early. The reply is signed too, over the
request's nonce, so the backoffice knows its application answered.
Reason: a bearer token puts the secret itself on every request, and a
proxy log keeps it. An HMAC keeps the secret off the wire, binds the body
and the path to it, and costs one hash. The nonce set makes a replay
inside the window cost nothing to refuse. The signature gives integrity
and authenticity, not secrecy: rows travel in clear over `http://`. Use
`https://` or a private network.

**W5. D4 and D6 hold on the wire.** The server decodes a `Call`, with its
`Scope::Where` and its `Claim`, only after the signature verifies. Its
data side is always `InProcess::enforcing_grants()`; there is no setting
to turn the check off. Reason: the condition runs as the server's own
SQL (D4), and the server must not trust the caller's RBAC decision (D6).
The secret is what makes the caller "an authenticated backoffice".

**W6. The server holds a transaction between requests, with a timeout
and a cap.** `Begin` opens a transaction for the claim and answers a
random 128-bit id. Each `InTx` takes the transaction out of the table,
runs one write, and puts it back. A second request for the same id while
one runs gets a `Protocol` error. A transaction idle for
`[backoffice] tx_timeout_secs` (30) rolls back: every request sweeps the
table, and a task the first request starts sweeps it every half timeout.
At `[backoffice] tx_max` (32) open transactions, `Begin` is refused with a
`help:` line. A write must carry the subject of the claim that opened the
transaction. A request cut off in the middle drops its transaction, which
keeps nothing. The client's transaction, dropped without `commit`, sends
a rollback. Reason: D8 made the transaction owned so that a server can
keep it. A held transaction holds a database connection, so it needs an
end that does not depend on the client.

**W7. The registry and the check are read once, at connect.** The
client's first act is `Hello`. The answer holds the protocol version, the
registry and the result of the data side's `check`. The client keeps
them: `TabData::registry` and `TabData::check` stay synchronous and read
the copy. A connect that fails is the client's `check` error, so the
admin's start-time check fails and the backoffice does not start. Start
the application first. Reason: an entity registry does not change while a
process runs (D1), and a sync `registry` keeps the admin's build as it is.

**W8. The backoffice binary takes its rows over the wire when
`[backoffice] data_url` is set.** `plugins::backoffice::contract` picks
the wire client when the binary is `backoffice` and `data_url` is set;
otherwise it keeps `InProcess`. The application binary ignores
`data_url`, and the backoffice binary never serves the wire, so one
configuration file serves both. The binary's name comes from
`CARGO_BIN_NAME`, which cargo compiles in. Reason: `contract` runs when
the admin is built, in the admin plugin's `configure` stage, before any
plugin's `register` and with no services to read a role from. The name of
the binary is what the binary is; it is fixed when it is compiled, and it
is not state that one application in a process can set for another. The
client connects on a thread of its own with a small runtime, because the
admin builds synchronously, both at start and on its first request.

**W9. The application serves the wire over the admin's own models.** The
data side of the wire is `InProcess::enforcing_grants()` over the
admin's source: the models of `src/plugins/admin/models.rs`, over the
ORM, as `plugins::rls::staff` with `rls` and `policy`. The generated
backoffice file compiles `models.rs` a second time with `#[path]`, as the
second binary compiles the application (B1). Reason: the admin's plugin
file builds its source inside a private `Later`, and its legacy rewrite
tables compare that file text for text. A seam there is a separate change
(§7.4).

### 7.6 Panel data contracts

**P1. A panel's data is a trait in the panel's crate, with owned serde
types.** The flags panel reads `brocanth_flags::admin::FlagsData`: `read`
(the stored rules and the last error), `set` and `remove`. `Flags`
implements it. `FlagsPanel::over(data, declared)` takes any
`Arc<dyn FlagsData>`; `FlagsPanel::new` and `FlagsPanel::late` keep their
signatures, so a generated admin needs no edit. Reason: the rule of
`docs/module-capabilities.md` §5.1, and D1, which said each panel gets its
own trait in its own crate.

**P2. One JSON seam in `brocanth-admin`, no serde in it.**
`brocanth_admin::panel::remote` has two traits that carry JSON text:
`PanelWire` (the backoffice's end: send one call of a tab, for a
principal) and `PanelServe` (the data side: decode a call, say the level
it needs, run it). A panel crate implements `PanelServe` for its data and
a remote `FlagsData` over any `PanelWire`. `brocanth-backoffice`
implements `PanelWire` over the wire client and dispatches
`Ask::Panel` to the `PanelServe` registered for the tab. Reason: a panel
crate must not depend on `brocanth-backoffice`, and the backoffice must
not depend on every panel crate. Text keeps `brocanth-admin` without
serde.

**P3. RBAC on the server, per panel call.** The panel's `PanelServe`
names the level each call needs (`read` for a read, `edit` for a write),
and the wire server refuses a claim that does not hold `<tab>.<level>`,
as D6 does for a model screen.

### 7.7 A binary without the application: waits

A backoffice binary that links only the admin and a contract client (no
application plugins) is not built. Three things the admin takes from the
application have no contract yet:

1. **The principal.** The admin reads the staff member from the session,
   through `auth` and `superuser`, over the database. Without those
   plugins the binary needs a session call on the wire, or a client of
   the session and staff tables.
2. **The admin's configuration.** The row policies (`AdminResource`), the
   model screens and the panels are code in `src/plugins/admin/`. The row
   scope is compiled from them on the admin's side (D4). A binary without
   that code has no scope to send, unless the scope moves to the data
   side, which reverses D4.
3. **Most panels.** Only flags has a data contract (§7.6); the rest read
   their crate's handle through a late cell.

With the wire, the second binary already reads and writes every model row
through the application. It still opens the database for the sessions
and the staff of point 1, and for the panels of point 3.

### 7.8 Built (v2, 2026-11-25)

- `brocanth_backoffice::wire` (feature `wire`, default): `Ask`, `Reply`,
  `ServiceSecret`, `sign`, `check_settings`, `WireServer` (held
  transactions, nonces, the reaper, `Ask::Panel` dispatch), `WireClient`
  (`connect`, `lazy`, `failed`, `panels`).
- `brocanth_admin::panel::remote`: `PanelWire`, `PanelServe`.
- `brocanth_flags::admin`: `FlagsData`, `FlagsView`, `FlagsCall`,
  `FlagsAnswer`, `FlagsServe`, `RemoteFlags`, `FlagsPanel::over`. `Rule`
  derives serde under `admin`.
- The `backoffice` module: five settings (`wire`, `data_url`,
  `service_secret`, `tx_timeout_secs`, `tx_max`), checked in `register`;
  `Door` serves the wire; `contract` picks the client (W8);
  `wire_source` (W9).
- Tests: `tests/wire.rs` (a server on a real socket in its own runtime:
  list, fetch, insert, update, delete; transactions committed, rolled
  back, dropped, timed out, capped; grants refused on the data side; a
  transaction bound to its subject; a wrong, missing or short secret at
  connect; an unsigned, stale, replayed or altered request; the admin's
  screens and the flags tab through `TestClient` over the wire; a panel
  call with its grant checked), `tests/identity.rs` (Postgres, forced
  RLS: each call and a held transaction run as the claim's identity
  through HTTP), the generated project's tests (no secret stops the
  start; the wire is not served while off). The three shapes build with
  `BROCANTH_BUILD=1`, the second binary included.

To split over the wire, with one secret for both:

```text
openssl rand -hex 32 > /run/secrets/backoffice
cargo run -- --backoffice.wire=true --backoffice.embedded=false \
  --backoffice.service_secret_file=/run/secrets/backoffice
cargo run --bin backoffice -- --backoffice.data_url=http://127.0.0.1:3000 \
  --backoffice.service_secret_file=/run/secrets/backoffice
```

Start the application first: the backoffice reads the registry at start
and stops with a `help:` line when it cannot.

### 7.4 Deferred

- Data traits for the panels jobs, cache, roles and analytics (and
  audit, config, openapi, realtime, webhooks, oauth-client). Each still
  reads its crate's handle through a late cell. Flags has one (§7.6).
- The flags panel over the wire in a generated project: the `flags`
  module's admin line still builds `FlagsPanel::late`, and `Door` does
  not register `FlagsServe`. The crate pieces exist and are tested.
- A backoffice binary that links only the admin and a contract client
  (§7.7).
- A seam in the admin's plugin file for its data source, so the wire's
  `wire_source` stops repeating it (W9).
- A test in the generated project with the wire on: its settings come
  from the process's arguments, so a test cannot turn it on for one
  application. The crate's tests cover the path.
- Sign-in pages in the second binary. It serves only `mount_at`; staff
  sign in on the application, on the same host.
- An image and a deployment for the second binary (`docker`,
  `kubernetes`).
- A conditional create rule (§2a, "Open") is unchanged by the contract.
