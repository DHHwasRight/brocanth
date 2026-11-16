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
| Needs | none |
| After | none; [`db`](db.md) waits for it |
| Adapts to | none |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Development only | yes: `brocanth check --deploy` reports `deploy/development-only` |
| Crates | `brocanth-db` (feature `embedded`: `postgresql_embedded` with `bundled`) |
| Without it | the pool opens on `[db] url`, a server you run |

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
- The first start extracts the bundled archive into `tmp/postgres/server`,
  runs `initdb` into `tmp/postgres/data` and creates the database. It takes
  a few seconds. A later start only starts the server.
- `shutdown` runs in reverse order: `db` closes its pool, then this plugin
  stops the server with `pg_ctl stop -m fast` and waits for it. A drop of
  the handle (a panic, an early error) stops it too.
- The server is a child process that `pg_ctl` detaches. It listens on
  localhost only (`127.0.0.1` and `::1`). So Ctrl-C reaches the application,
  and the application stops the server; a server that stopped already is
  not an error.
- A server left running by a killed process (`kill -9`) is stopped at the
  next start, so the start does not fail on a stale `postmaster.pid`.

## Never in production

Three guards, each with a `help:` line:

1. `brocanth check --deploy` reports `deploy/development-only` as an error,
   from the manifest's `development_only`.
2. The plugin refuses to start when `$<APP>_ENV` is not `development` or
   `test`.
3. The feature `embedded` is off unless this module is chosen, so a project
   without it links none of it. Remove the module before a deploy:
   `brocanth remove db-embedded`, and set `[db] url`.

**Reason:** the server lives in one checkout on one disk, with a generated
password in a file beside its data. It has no backup, no replica, and no
upgrade path. The release image would also carry the 38-98 MiB archive.

## Facts checked (2026-11-16)

| | |
|---|---|
| Crate | `postgresql_embedded` 0.22.0, theseus-rs |
| Crate licence | `(Apache-2.0 OR MIT) AND PostgreSQL` |
| Archive source | GitHub releases of `theseus-rs/postgresql-binaries`, licence PostgreSQL |
| PostgreSQL licence | The PostgreSQL Licence: permissive, BSD/MIT style. No copyleft term reaches the application. |
| Server version | the latest release at build time; 18.7.0 when this was checked. `POSTGRESQL_VERSION='=17.6.0'` at build time pins one, and the build script then caches the archive in `~/.theseus/postgresql`. |
| `sqlx` | 0.9, the copy the workspace already has: no second driver |
| Network | at build time only: the build script downloads the archive once per target directory (`OUT_DIR`). The first start needs none. |

Archive size per target, release 18.7.0 (the binary grows by this much):

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
musl) and Windows, on x86_64 and aarch64. Another target fails the build
with the build script's error.

The build script uses the GitHub API without a token. Behind a rate limit,
set `GITHUB_TOKEN` for the build.

## Directories

| Path | What | Lifetime |
|---|---|---|
| `tmp/postgres/server/<version>/` | the extracted binaries: 270 MB on `aarch64-apple-darwin` | extracted once |
| `tmp/postgres/data/` | the cluster; `start.log` is the server's log. 47 MB empty | survives a restart |
| `tmp/postgres/password` | the superuser password, made at the first start | goes with `data/` |

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
module free of the archive, the crate and its build script. The cost:
`cargo build --all-features` in this workspace downloads the archive.

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
