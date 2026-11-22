# `db-embedded`

A Postgres server started from the binary, for development with no Postgres installed; never in production.

The wizard shows it under the database, in the Data question:
*Postgres inside the binary, for development*.

| | |
|---|---|
| Categories | **Enhancer** |
| Wizard question | Data |
| Default | off |
| Presets | none |
| Part of | [`db`](db.md), off until you tick it |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Development only | yes: `brocanth check --deploy` reports `deploy/development-only` |
| Crates | `brocanth-db` (feature `embedded`: `postgresql_embedded`, no `bundled`) |
| Without it | the pool opens on `[db] url`, a server you run |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/db_embedded.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | none |
| Contributes | `AgentsSection` `db-embedded`: the database runs in the binary, for development only |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | `[db_embedded]`: `dir`, `port`, `database`, `timeout` |
<!-- end generated: capabilities -->

`brocanth create my-app --with db,db-embedded`, then `cargo run`: the
application starts its own Postgres and serves, with no Postgres and no
container on the machine. `brocanth add db-embedded` adds it to a project
that has `db`.

## How it works

- The plugin `db_embedded` is registered before `db`. In `configure` it
  starts the server and puts `brocanth_db::embedded::EmbeddedUrl` in the
  application's services.
- The `db` plugin reads that URL in its own `configure` and opens its pool
  on it, in place of `[db] url`. `db` stays the one pool: this module opens
  none.
- The first start installs Postgres `POSTGRES_VERSION` (18.7.0) into
  `tmp/postgres/server` (see [The archive](#the-archive)), runs `initdb` into
  `tmp/postgres/data` and creates the database. It takes a few seconds. A
  later start only starts the server.
- `shutdown` runs in reverse order: `db` closes its pool, then this plugin
  releases the server. The last user stops it with `pg_ctl stop -m fast`
  and waits for it. A drop of the handle (a panic, an early error) releases
  it too.
- The server is a child process that `pg_ctl` detaches. It listens on
  localhost only (`127.0.0.1` and `::1`). So Ctrl-C reaches the application,
  and the application stops the server; a server that stopped already is
  not an error.
- A server left running by a killed process (`kill -9`) is stopped at the
  next start, so the start does not fail on a stale `postmaster.pid`.

## One server per directory

Every application started on one `dir` uses one server: the tests of a
project, which build an application each and run in parallel, and a second
terminal. Two servers on one `data/` cannot run: the second `pg_ctl start`
fails, and a second `initdb` refuses a directory that is not empty.

- **In one process**, a registry keyed by the canonical directory holds the
  server and counts its users. The first start sets it up and starts it;
  the others join it. Each start creates its own database if it is missing.
  The last release stops it.
- **Across processes**, two lock files in `dir` order the work:
  - `setup.lock`, exclusive, while a process installs, runs `initdb`,
    starts, joins or stops. A second process waits for the first, so
    `initdb` runs once. `initdb` writes `data.partial/`, renamed to
    `data/` when it is done.
  - `users.lock`, shared, held by each process that uses the server. A
    process that joins reads the port from `data/postmaster.pid` and the
    password from `password`. A process that releases stops the server
    only when no other process holds `users.lock`.
- The operating system drops the locks of a dead process. A server that
  runs while nobody holds `users.lock` is an orphan of a `kill -9`: the next
  start stops it and starts it again.
- The first start chooses the port. A process that joins uses that port,
  whatever its own `port` says.

**Reason:** one cluster has one postmaster. Sharing the running server is
what a developer expects from `cargo test` and from a second terminal; a
server per test would need a data directory per test and an `initdb` each.

## Never in production

Three guards, each with a `help:` line:

1. `brocanth check --deploy` reports `deploy/development-only` as an error,
   from the manifest's `development_only`.
2. The plugin refuses to start when `$<APP>_ENV` is not `development` or
   `test`.
3. The feature `embedded` is off unless an embedded module is chosen, so a
   project without one links none of it.

Before a deploy, either remove the module (`brocanth remove db-embedded`,
and set `[db] url`), or switch to the production module:
`brocanth remove db-embedded && brocanth add db-embedded-prod`
([`db-embedded-prod`](db-embedded-prod.md)).

**Reason:** this module keeps its server in one checkout, under a relative
directory, shared by every process that starts on it. The production
module adds what a deploy needs: a required data directory, one owning
process, an archive pinned by hash with no download, a role that is not the
superuser, and backup and restore. The decisions are in
[Embedded Postgres in production](../embedded-postgres-production.md).

## Facts checked (2026-11-16)

| | |
|---|---|
| Crate | `postgresql_embedded` 0.22.0, theseus-rs |
| Crate licence | `(Apache-2.0 OR MIT) AND PostgreSQL` |
| Archive source | GitHub releases of `theseus-rs/postgresql-binaries`, licence PostgreSQL |
| PostgreSQL licence | The PostgreSQL Licence: permissive, BSD/MIT style. No copyleft term reaches the application. |
| Server version | 18.7.0, pinned in `brocanth_db::embedded::POSTGRES_VERSION` |
| `sqlx` | 0.9, the copy the workspace already has: no second driver |
| Network | none at build time. At the first start only, and only when no archive file and no cache is on the machine. |

Archive size per target, release 18.7.0 (one download, one cached file):

| Target | Archive |
|---|---|
| `aarch64-apple-darwin` | 84 MiB |
| `x86_64-apple-darwin` | 87 MiB |
| `x86_64-unknown-linux-gnu` | 38 MiB |
| `aarch64-unknown-linux-gnu` | 38 MiB |
| `x86_64-unknown-linux-musl` | 98 MiB |
| `aarch64-unknown-linux-musl` | 96 MiB |
| `x86_64-pc-windows-msvc` | 47 MiB |
| `aarch64-pc-windows-msvc` | 46 MiB |

Supported platforms: the eight targets above, macOS, Linux (glibc and
musl) and Windows, on x86_64 and aarch64. On another target the first start
fails: no release has an archive for it.

## The archive

The build needs no network. The first start installs the binaries into
`tmp/postgres/server/18.7.0/` from the first of:

| Source | When |
|---|---|
| `$BROCANTH_POSTGRES_ARCHIVE` | the variable names a `postgresql-18.7.0-<target>.tar.gz` file. For a machine with no network: download the file from the [releases](https://github.com/theseus-rs/postgresql-binaries/releases/tag/18.7.0) once and point the variable at it. |
| `~/.theseus/postgresql/postgresql-18.7.0-<target>.tar.gz` | the cache. One file per machine, shared by every checkout. `postgresql_embedded`'s own build script caches to the same name. |
| GitHub | one API request (the release by tag, not the release list) and one download, checked against the release's SHA-256 file, then written to the cache. Set `GITHUB_TOKEN` behind the rate limit. |

The extraction goes to a directory beside `18.7.0/` and is renamed into
place, so a process killed halfway leaves nothing that looks installed.

**Reason, against `bundled`:** the `bundled` feature of `postgresql_embedded`
downloads the archive in its build script. Its version defaults to the
latest release, so the script lists the releases through the GitHub API on
each fresh target directory. Every `--all-features` build of this workspace
then needed the network, failed on a GitHub 504, and shared the
unauthenticated limit of 60 requests an hour, which CI reaches. A pinned
`POSTGRESQL_VERSION` at build time still makes one API request per fresh
machine, and an environment variable that every build must set is easy to
lose. Installing at the first start keeps every build offline and
deterministic, and the binary is 38-98 MiB smaller. The cost: the first
`cargo run` on a machine with no cache needs the network once, or the
archive file.

The tests that start a real server (`tests/embedded.rs`) need the archive.
They skip with a message when no archive file and no cache is on the
machine, unless `BROCANTH_EMBEDDED_TEST=1` lets them download it.
`mise run embedded` sets it and runs them.

## Directories

| Path | What | Lifetime |
|---|---|---|
| `tmp/postgres/server/<version>/` | the extracted binaries: 270 MB on `aarch64-apple-darwin` | extracted once |
| `tmp/postgres/data/` | the cluster; `start.log` is the server's log. 47 MB empty | survives a restart |
| `tmp/postgres/password` | the superuser password, made at the first start | goes with `data/` |
| `tmp/postgres/setup.lock`, `users.lock` | the locks of [One server per directory](#one-server-per-directory) | empty files |

`[db_embedded] dir` moves all three. The path is relative to the working
directory: `cargo run` starts in the project root. `.gitignore` gets
`/tmp/postgres`. Delete the directory for an empty database.

## `migrate` and `dbshell`

Both read `[db] url` and do not start the server. To use them, set
`[db_embedded] port`, run the application, and point `[db] url` at that
port: the start line prints the URL with the password redacted, and the
password is in `tmp/postgres/password`.

## What it writes

Files:

- `src/plugins/db_embedded/mod.rs`

Slots:

- src/plugins/mod.rs (the builder chain), before `db`
- src/plugins/mod.rs: `pub mod db_embedded;`
- src/plugins/mod.rs (the provider list): `db_embedded crate::plugins::db_embedded::DbEmbedded`
- config/<env>.toml: `[db_embedded]`
- .gitignore: `/tmp/postgres`
- AGENTS.md: `## The development database`

In `src/plugins/db/mod.rs`, which `db` writes: three lines at the top of
`configure` that read `EmbeddedUrl`.

## Settings

Section `[db_embedded]`. Environment variable: `$<APP>_DB_EMBEDDED__<KEY>`. Flag: `--db_embedded.<key>`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `dir` | text | `"tmp/postgres"` | `--db_embedded.dir` | The directory for the server's binaries, data and password, from the project root. |
| `port` | integer | `0` | `--db_embedded.port` | The port on localhost. 0 takes a free one at each start. |
| `database` | text | the project name, `_` for `-` | `--db_embedded.database` | The database the pool opens, created at the first start. |
| `timeout` | integer | `60` | `--db_embedded.timeout` | Seconds initdb or pg_ctl may take, each. |

## Feature, not crate

The runtime piece is `brocanth_db::embedded`, behind the feature
`embedded` of `brocanth-db`, and not a crate of its own. **Reason:** the
module is part of `db` (module capabilities, section 4.2) and its code is
one file that hands a URL to the pool. A feature keeps one dependency line
in the project; the optional dependencies keep a project without the
module free of the crate.

## When module capabilities stage 1 lands

Today the link to `db` is a manifest `after` and a service type. With
stage 1 (`PgPool` as a Kind):

- `db-embedded` becomes a development-only provider of the URL `db`
  opens, not a second provider of `PgPool`: `db` stays the only provider
  of the pool. Its manifest declares `development_only` as now and
  `requires::<PgPool>` in place of `part_of_but_off("db")` as its reason
  to need `db`.
- The resolver never adds it on its own. When a module requires `PgPool`,
  the resolver adds `db`, the one non-development provider. `db-embedded`
  comes in only when it is chosen.
- The `after("db-embedded")` line in `db`'s manifest becomes an order
  inside a phase (stage 4), and the `EmbeddedUrl` lookup stays in `db`'s
  `configure`.

## Read more

- [Module capabilities](../module-capabilities.md), section 4.2
- [`db`](db.md)
- Source: [`crates/brocanth-db/src/scaffold_embedded.rs`](../../crates/brocanth-db/src/scaffold_embedded.rs), [`crates/brocanth-db/src/embedded.rs`](../../crates/brocanth-db/src/embedded.rs)
