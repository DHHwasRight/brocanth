# Embedded Postgres in production

**Status:** accepted (2026-11-22). **ADR:** PLAN.md §13, ADR 12.
**Touches:** `brocanth-db` (feature `embedded`), the `db-embedded` and
`db-embedded-prod` modules, `brocanth check --deploy`.

The owner decision (2026-11-22): embedded Postgres must also work in
production, for some cases: first experiments, a single-binary deploy, a
shape close to PocketBase (one binary, one data directory, nothing else to
install). It is an explicit opt-in per project, never a default. The
development-only module stays the default mode.

This page records each decision and its reason. The user manual is
[`docs/modules/db-embedded-prod.md`](modules/db-embedded-prod.md).

## 1. The opt-in

**Decision:** a separate module, `db-embedded-prod`. It is a child of `db`,
off until asked for, and it conflicts with `db-embedded`. Its manifest has
no `development_only`, so `brocanth check --deploy` does not report it.
`db-embedded` keeps `development_only`, and `check --deploy` still reports
it as `deploy/development-only`.

```
brocanth create my-app --with config,db,db-embedded-prod,commands
brocanth remove db-embedded && brocanth add db-embedded-prod   # switch
```

**Reason, against a build-time option (`production = true` in
`src/modules/db_embedded.rs`):** the CLI does not read the values of a
module file. It links no Rust parser, and `check` reads only the presence
of the files (see `module_files`). A module key is in `brocanth.toml`, so
`check --deploy`, `explain` and the wizard see the choice. The two modules
write the same plugin file, the same `[db_embedded]` section and use the
same runtime (`brocanth_db::embedded`), so a switch is one `remove` and one
`add`.

- **The resolver:** no change. Neither module provides `PgPool`; `db`
  stays its one provider. The resolver never adds an embedded module for a
  requirement. A project gets one only when it names it.
- **The mode at run time:** the module is the structural choice, made at
  build time. The environment picks the profile, as for every other
  plugin. With `$<APP>_ENV` set to `development` or `test`, the server
  runs as `db-embedded` runs it (default directory, shared by the tests
  and a second terminal). In any other environment it runs in
  **production mode**: the rules below.
- **The plugin of `db-embedded`** still refuses to start outside
  `development` and `test`. Its `help:` line now names
  `db-embedded-prod`.

## 2. The supply chain

**Decision:** one SHA-256 per target, pinned in the code next to
`POSTGRES_VERSION`, in `crates/brocanth-db/src/embedded_pins.rs`. Bump the
two together. Every archive is checked against the pin before extraction,
in every mode and from every source: the bundled bytes, a file, the cache,
a download. A mismatch is an error with a `help:` line, and nothing is
extracted.

**Reason:** the GitHub release also publishes a `.sha256` file, and
`postgresql_archive` checks a download against it. That hash comes from the
same place as the archive, so it proves only that the download is whole. A
pin in the source proves that the archive is the one a reviewer accepted.
The pins were taken from the release's `.sha256` files and confirmed
against a download of each of the eight archives (2026-11-22).

**Decision, production:** a production start never uses the network and
never reads the per-user cache (`~/.theseus`). It installs from the first
of:

| Source | How |
|---|---|
| bundled | `BROCANTH_POSTGRES_BUNDLE=<archive> cargo build --release`. The build script checks the pin and embeds the archive in the binary (`include_bytes!`). The start checks it again. One binary, nothing else on the box. |
| a file | `--db_embedded.archive=/opt/postgresql.tar.gz` (or `$BROCANTH_POSTGRES_ARCHIVE`). The image build copies the file in (`ADD --checksum=sha256:…`). |
| installed | `<dir>/server/<version>/` with every binary the server needs: a later start reads no archive. |

With none of them, the start fails with a `help:` line that names the
three. **Reason:** a start that downloads depends on GitHub and on its rate
limit at the worst moment (a restart during an incident), and it fetches
code to run on a production box.

**Reason, against a `brocanth db install` step:** the archive file plus the
first start does the same work with one fewer command, and the `brocanth`
CLI links no runtime. The image build verifies the file with
`ADD --checksum`, and the start verifies it again.

**The install check:** a start used to test only that `server/<version>/`
exists, so an emptied copy (a cache that keeps directories and drops files)
passed it and then failed in `initdb`. It now checks that `postgres`,
`pg_ctl`, `initdb`, `pg_dump` and `pg_restore` are non-empty files. An
incomplete copy is removed and extracted again.

## 3. Data safety

- **The directory:** `--db_embedded.dir=…`, required in production. A
  relative path (the default `tmp/postgres`) or a path under the system
  temporary directory is refused at start, with a `help:` line. **Reason:**
  a deploy that forgets the flag must not write its data into the working
  directory of the image, where a restart loses it.
- **The major version:** a start refuses a `data/` whose `PG_VERSION` is
  not the major of `POSTGRES_VERSION`, in every mode. The `help:` line
  gives the dump-and-restore steps. **Reason:** Postgres cannot read the
  files of another major, and a silent `initdb` or upgrade would lose or
  hide data.
- **Durability:** `fsync`, `synchronous_commit` and `full_page_writes`
  stay at the Postgres defaults (all on). **Reason:** the defaults are
  durable; a faster setting trades data for speed, and that is the
  operator's call.
- **SIGTERM:** `serve` drains on SIGTERM, and the shutdown runs the plugins
  in reverse: `db` closes its pool, then the server stops with
  `pg_ctl stop -m fast` and the plugin waits for it. Each transaction in
  flight rolls back.
- **A crash:** Postgres recovers from its own WAL at the next start. A
  server left running by a killed process is stopped and started again
  (the existing orphan logic). The tests kill the process and the
  postmaster with SIGKILL and read the rows back.
- **One owner:** in production, one process owns the server. It takes
  `users.lock` exclusive and keeps it. A second process (or a second start
  in the same process) is refused with a `help:` line. **Reason:** sharing
  exists for `cargo test` and a second terminal. In production, a second
  process means a second deploy on the same volume, and its stop would
  stop the server under the first one. `db_backup` still reads a running
  server: it connects, and owns nothing.

## 4. Security

- **Listen:** `listen_addresses` is set to `localhost` on the command line,
  in every mode, so a `postgresql.conf` edit cannot open it to the
  network. The Unix socket stays at the Postgres default. Every local
  connection needs a password (below).
- **Authentication:** a new cluster gets `scram-sha-256` in `pg_hba.conf`
  in place of the `password` method that `postgresql_embedded` writes.
  **Reason:** `password` sends the password in clear text.
- **Secrets:** the superuser password and the application role's password
  come from the operating system's random source (`getrandom`), 32 bytes
  in hex. They are written with mode `0600`, in `<dir>/password` and
  `<dir>/app_password`. **Reason:** the volume is the only place the
  server and the application share; a secret manager adds a dependency the
  PocketBase shape does not want.
- **The application role:** `db-embedded-prod` connects the pool as a
  role named after the database, not as the superuser. The role has
  `LOGIN` and `CREATEROLE`, and has no `SUPERUSER`, `CREATEDB` or
  `BYPASSRLS`. It owns the database, so the migrations that run through the
  pool create and own the tables. The superuser is used only to set up the
  role and the database, and for backup and restore.
- **RLS:** the owner of a table bypasses its policies unless the table is
  forced. `brocanth-rls` forces each table by default, and a request
  assumes a role that owns nothing (`SET LOCAL ROLE app_user`), so the
  policies apply. `CREATEROLE` lets the migrations create that role
  (`docs/rls.md`, "The privileges the migration role needs").
- **Not as root:** `initdb` and `postgres` refuse to run as root. The
  image runs as a user that is not root (the `docker` module does already).

## 5. Operations

- **Backup:** `my-app db_backup <file>` (with `commands`): `pg_dump
  --format=custom` of the application's database, from the binaries in
  `<dir>/server`. It connects to the running server (port from
  `postmaster.pid`), or starts the server for the dump when nothing runs.
  The file is written aside and renamed.
- **Restore:** `my-app db_restore <file>`, with the application stopped
  (refused otherwise). It restores into a new database in one transaction,
  then swaps the names. **Reason:** a failed restore leaves the old
  database as it was.
- **The volume:** `<dir>` holds everything: `data/`, `server/`, the two
  password files and the locks. Mount one volume there. Back it up with
  `db_backup`; a copy of `data/` while the server runs is not a backup.
- **Memory:** Postgres defaults (`shared_buffers` 128 MB,
  `max_connections` 100). They run on a 512 MB box; the pool opens 10
  connections. No tuning setting in v1.
- **Logs:** in production mode the plugin copies each new line of
  `data/start.log` to the process's stderr, with the prefix `postgres: `.
  **Reason:** a container platform collects stderr only. `pg_ctl` detaches
  the server and writes its log to a file.
- **Health:** the existing check. `/readyz` calls `Db::ping`, and the pool
  goes through the embedded server: a stopped server fails the check.
- **Minor upgrade** (18.7.0 to 18.8.0, same major): bump `POSTGRES_VERSION`
  and the pins together. The next start extracts the new binaries into
  `server/18.8.0/` and starts them on the same `data/`. Delete the old
  `server/<version>/` by hand.
- **Major upgrade** (18 to 19): the start refuses the old `data/`. With the
  old binary, run `db_backup`; deploy the new binary on an empty `dir`,
  start it once, stop it, and run `db_restore`.

## 6. Deferred

- Replication, a standby, failover.
- Point-in-time recovery (WAL archiving, base backups).
- `pg_upgrade` in place.
- The superuser password from `_file`/`_env`. The generated file serves
  v1.
- A Unix socket only, with no TCP port.
- Server tuning settings (`shared_buffers` and others) and log rotation.
- The `docker` module adapted to `db-embedded-prod` (a `VOLUME`, the
  archive `ADD`). The manual shows the lines.
- Production on Windows: built, not tested.
