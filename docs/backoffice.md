# The backoffice

What Django's admin and PocketBase's dashboard contain in a default
install, which module here owns each part, and what is missing.

The seam is `brocanth_admin::panel::Panel`: a tab that renders its own body
and is handed the admin's chrome. It lives in the crate that owns the data,
behind an `admin` feature that is off, because `brocanth-admin` has four
dependencies and intends to keep them. `brocanth-jobs` is the worked
example.

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

## 3. Who owns what here

| Feature | Module | Tab | State |
|---|---|---|---|
| J1, J2, J4, J6 | `admin` + `model` | the four screens | **done** |
| J3 actions over selected rows | `admin` | the list screen | **missing** |
| J5 inlines | `admin` | the form screen | **missing** |
| J7 history / audit | `events` | **Changes** | **blocked** — see below |
| J8 users | `auth` | **Accounts** | **blocked** — see below |
| J9, J10, P4 rules | `policy` | **Rules** | buildable, read-only |
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
