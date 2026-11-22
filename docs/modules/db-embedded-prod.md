# `db-embedded-prod`

A Postgres server started from the binary, in production too: one binary, one data directory, an archive pinned by hash.

The wizard shows it under the database, in the Data question:
*Postgres inside the binary, in production too*.

| | |
|---|---|
| Categories | **Enhancer** |
| Wizard question | Data |
| Default | off |
| Presets | none |
| Part of | [`db`](db.md), off until you tick it |
| Conflicts | [`db-embedded`](db-embedded.md): both start the server of `[db_embedded]`; this one adds the production rules, so keep one; [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Development only | no: `brocanth check --deploy` accepts it |
| Crates | `brocanth-db` (feature `embedded`) |
| Without it | the pool opens on `[db] url`, a server you run |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/db_embedded_prod.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | [`commands`](commands.md): `db_backup` and `db_restore` commands dump and restore the database with the server's own pg_dump and pg_restore |
| Contributes | `AgentsSection` `db-embedded-prod`: the database runs in the binary, in production too |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | `[db_embedded]`: `dir`, `port`, `database`, `timeout`, `archive` |
<!-- end generated: capabilities -->

It is the explicit production opt-in of [`db-embedded`](db-embedded.md),
for a first experiment or a single-binary deploy: the binary and one data
directory are the whole install, close to PocketBase. It is never a
default. The decisions and their reasons are in
[Embedded Postgres in production](../embedded-postgres-production.md).

## Opt in and deploy

```
brocanth create my-app --with config,db,db-embedded-prod,commands
# or, from the development module:
brocanth remove db-embedded && brocanth add db-embedded-prod
```

Build one binary that carries the Postgres archive. The build checks the
file against the SHA-256 pinned in `brocanth-db`, and fails on a wrong one:

```
curl -fsSLo postgresql.tar.gz \
  https://github.com/theseus-rs/postgresql-binaries/releases/download/18.7.0/postgresql-18.7.0-x86_64-unknown-linux-gnu.tar.gz
echo "53ca01952cb581145e4fb16b6b3b675da345a82879f350707b9f65863a0642f4  postgresql.tar.gz" | sha256sum -c
BROCANTH_POSTGRES_BUNDLE=$PWD/postgresql.tar.gz cargo build --release
```

Run it as a user that is not root, on a directory that survives a restart:

```
MY_APP_ENV=production ./my-app serve --db_embedded.dir=/var/lib/my-app
```

Without the bundle, copy the archive in and name it. In a Dockerfile (the
hash is the pin for the target; bump it with `POSTGRES_VERSION`):

```dockerfile
ADD --checksum=sha256:53ca01952cb581145e4fb16b6b3b675da345a82879f350707b9f65863a0642f4 \
    https://github.com/theseus-rs/postgresql-binaries/releases/download/18.7.0/postgresql-18.7.0-x86_64-unknown-linux-gnu.tar.gz \
    /opt/postgresql.tar.gz
VOLUME /var/lib/my-app
CMD ["my-app", "serve", "--db_embedded.dir=/var/lib/my-app", "--db_embedded.archive=/opt/postgresql.tar.gz"]
```

## Development and production

The module is the build-time choice. `$<APP>_ENV` picks the profile:

| | `development`, `test` | any other environment |
|---|---|---|
| `dir` | `tmp/postgres` by default | required, an absolute path outside the temporary directory |
| Processes | shared: tests in parallel, a second terminal | one owner; a second process is refused |
| Archive | bundled, `archive`, `$BROCANTH_POSTGRES_ARCHIVE`, the cache, or one download | bundled, `archive` or `$BROCANTH_POSTGRES_ARCHIVE`; never the network |
| Server log | `data/start.log` | `data/start.log`, and each line on stderr as `postgres: …` |

In both, every archive is checked against the pinned SHA-256 before
extraction, a data directory of another major version is refused, the
server listens on localhost only, and the pool connects as a role that is
not the superuser.

Every refusal happens at start, with a `help:` line.

## The role and RLS

The pool connects as a role named after the database (`database`, for
example `my_app`). It has `LOGIN` and `CREATEROLE`, and has no
`SUPERUSER`, `CREATEDB` or `BYPASSRLS`. It owns the database, so the
migrations create and own the tables. Its password is generated at the
first start, in `<dir>/app_password` (mode `0600`). The superuser's is in
`<dir>/password` (mode `0600`); only the setup, the backup and the restore
use it.

The owner of a table bypasses its policies unless the table is forced.
`brocanth-rls` forces each table by default, and a request assumes a role
that owns nothing (`SET LOCAL ROLE app_user`), so the policies apply.
`CREATEROLE` lets a migration create that role.

## Backup and restore

With `commands`, the project gets two commands. Both read `[db_embedded]`
the way the server does:

```
MY_APP_ENV=production ./my-app db_backup /backups/app.dump --db_embedded.dir=/var/lib/my-app
MY_APP_ENV=production ./my-app db_restore /backups/app.dump --db_embedded.dir=/var/lib/my-app
```

- `db_backup` writes a `pg_dump --format=custom` file (mode `0600`). It
  connects to the running server, so the application keeps serving; with
  no server running, it starts one for the dump.
- `db_restore` needs the application stopped. It restores into a new
  database in one transaction, then swaps the names: a failed restore
  changes nothing. The roles the dump names must exist: restore into the
  directory the dump came from, or start the application once on the new
  one first.

Library: `brocanth_db::embedded::backup` and `restore`.

## The volume

`<dir>` holds everything; mount one volume there.

| Path | What |
|---|---|
| `server/<version>/` | the binaries, extracted at the first start (about 100 MB on Linux) |
| `data/` | the cluster. A copy while the server runs is not a backup: use `db_backup` |
| `password`, `app_password` | the two passwords, mode `0600` |
| `setup.lock`, `users.lock` | the locks; `users.lock` is held exclusive by the owner |

## Upgrades

- **Minor** (18.7.0 to 18.8.0): bump `POSTGRES_VERSION` and the pins in
  `crates/brocanth-db/src/embedded_pins.rs` together. The next start
  extracts `server/18.8.0/` and runs it on the same `data/`. Delete the old
  `server/<version>/` by hand.
- **Major** (18 to 19): the start refuses the old `data/`. With the old
  binary run `db_backup`; start the new one once on an empty `dir`, stop
  it, and run `db_restore`.

## Shutdown and crashes

`serve` drains on SIGTERM. Then `db` closes its pool and this plugin stops
the server with `pg_ctl stop -m fast` and waits for it. After a crash, the
next start stops a server the dead process left running, and Postgres
replays its WAL. `fsync` and the other durability settings stay at the
Postgres defaults.

## Not in v1

Replication, point-in-time recovery, `pg_upgrade` in place, the superuser
password from `_file`/`_env`, a socket-only server, tuning settings, log
rotation, a `docker` module adapted to it, Windows in production (built,
not tested).

## Settings

Section `[db_embedded]`. Environment variable: `$<APP>_DB_EMBEDDED__<KEY>`. Flag: `--db_embedded.<key>`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `dir` | text | `"tmp/postgres"` | `--db_embedded.dir` | The directory for the server's binaries, data and passwords. Production needs an absolute path on a volume. |
| `port` | integer | `0` | `--db_embedded.port` | The port on localhost. 0 takes a free one at each start. |
| `database` | text | the project name, `_` for `-` | `--db_embedded.database` | The database the pool opens, created at the first start. The pool's role has the same name. |
| `timeout` | integer | `60` | `--db_embedded.timeout` | Seconds initdb or pg_ctl may take, each. |
| `archive` | text | `""` | `--db_embedded.archive` | A `postgresql-<version>-<target>.tar.gz` file to install from, checked by SHA-256. Empty: none. |

## What it writes

Files:

- `src/plugins/db_embedded/mod.rs`
- with `commands`: `src/commands/db_backup.rs`, `src/commands/db_restore.rs`

Slots: the same as [`db-embedded`](db-embedded.md#what-it-writes), with
`## The embedded database` in AGENTS.md.

## Read more

- [Embedded Postgres in production](../embedded-postgres-production.md): the decisions
- [`db-embedded`](db-embedded.md): the development module, the archive, the lock files
- Source: [`crates/brocanth-db/src/scaffold_embedded.rs`](../../crates/brocanth-db/src/scaffold_embedded.rs), [`crates/brocanth-db/src/embedded.rs`](../../crates/brocanth-db/src/embedded.rs)
