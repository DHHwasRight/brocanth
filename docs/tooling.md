# Tooling: fixtures, the database shell, deploy checks, prefetch

Four small tools from gap G9 (`docs/framework-gaps.md`). Each one says what it
does, and what it does not do.

## 1. Fixtures (Django D27)

A fixture is a JSON file of records under `fixtures/`. A record names a model
by its table name and sets columns by their database names:

```json
[
  { "model": "user", "fields": { "id": 1, "name": "Ada", "email": "ada@example.com" } },
  { "model": "post", "fields": { "id": 1, "user_id": 1, "title": "Hello",
                                 "slug": "hello", "status": "draft", "views": 0 } }
]
```

`brocanth_model::Fixtures` loads it. You register the entities a fixture may
write:

```rust
use brocanth_model::Fixtures;

let fixtures = Fixtures::new()
    .dir("fixtures")
    .model::<user::Entity>()
    .model::<post::Entity>();
fixtures.load("blog", &db).await?;
```

What it does:

- **It checks the whole file before it writes a row.** An unknown model, an
  unknown column, or a value that does not fit the field's Rust type is a
  problem with a file, a line, a column and a `help:` line. It reports every
  problem in the file, not only the first:

  ```text
  fixtures/blog.json:3:58: record 2 (`user`) has no column `emial`
    = help: did you mean `email`? The columns of `user` are: id, name, email
  ```

- **It writes through the model layer.** Each record becomes the entity's
  `ActiveModel` and goes through `insert`, so `before_save` and every
  `#[brocanth_model::validates]` rule run. Django and Rails skip the hooks for
  a fixture. brocanth does not: a seed that fails a validation is a seed that
  no form can edit.
- **One transaction per file.** If record 7 fails, records 1 to 6 do not stay.
  Records go in file order, so put a row after the rows it refers to.
- **On Postgres, it moves the sequence** of each table where a record set an
  integer key, so the next `INSERT` without a key does not collide.

`Fixtures::check(file, text)` runs the first half with no database. A CI step
can check every fixture file with it.

### In a test

`Harness::begin` gives a transaction that cannot commit. Load into it. The
fixture's transaction becomes a `SAVEPOINT`, and the rows go away at the end
of the test:

```rust
let harness = Harness::migrated::<Migrator>().await?;
let txn = harness.begin().await?;
fixtures.load("blog", &*txn).await?;
```

### From the command line

`brocanth fixtures load blog` gives `fixtures load blog` to the project's own
binary, as `brocanth` does for every command it does not define. The global
`brocanth` cannot know the entities of a project, so the command is in the
project. Put this in `src/commands/fixtures.rs` and add `pub mod fixtures;`
to `src/commands/mod.rs`:

```rust
use brocanth_command::{anyhow, clap, cmd, Command, Context};

/// Load a fixture from fixtures/<NAME>.json into the database
#[cmd]
#[derive(clap::Args)]
pub struct Fixtures {
    #[command(subcommand)]
    action: Action,
}

#[derive(clap::Subcommand)]
enum Action {
    /// Write every record in fixtures/<NAME>.json, in one transaction
    Load { name: String },
    /// Check fixtures/<NAME>.json against the models, with no database
    Check { name: String },
}

impl Command for Fixtures {
    fn run(self, ctx: &mut Context) -> anyhow::Result<()> {
        let set = brocanth_model::Fixtures::new()
            .dir(ctx.root().join("fixtures"))
            .model::<crate::models::user::Entity>();
        let config = brocanth_config::Loader::new(crate::app::APP).load()?;
        let url = config.section::<crate::plugins::model::Settings>("model")?.url;
        tokio::runtime::Runtime::new()?.block_on(async {
            match self.action {
                Action::Load { name } => {
                    let db = brocanth_model::sea_orm::Database::connect(url).await?;
                    let loaded = set.load(&name, &db).await?;
                    println!("{} rows from {name}", loaded.rows());
                }
                Action::Check { name } => {
                    let path = set.path_of(&name);
                    let text = std::fs::read_to_string(&path)?;
                    let records = set.check(&path.display().to_string(), &text)?;
                    println!("{records} records, all of them fit the models");
                }
            }
            Ok(())
        })
    }
}
```

The scaffold does not write this file yet. The model list is the project's,
and the scaffold has no entities to put in it.

Not done: `dumpdata`, and a JSON, array, `Decimal` or `Uuid` column. Those
types are behind SeaORM features that `brocanth-model` does not turn on, and
the error says so.

## 2. `brocanth dbshell` (Django D26)

```text
brocanth dbshell                      # the development database
brocanth dbshell --env production     # [model] url of config/production.toml
brocanth dbshell -- -c 'select 1'     # arguments after -- go to the client
brocanth dbshell --print              # print the command line, run nothing
```

It reads `[model] url` through the same `brocanth_config::Loader` the
application uses, so the files, `$<APP>_ENV` and `$<APP>_MODEL__URL` agree
with what the application sees. Then it replaces itself with the database's
own client:

| URL | Client | Notes |
|---|---|---|
| `postgres://` or `postgresql://` | `psql <url>` | The password goes in `PGPASSWORD`, not on the command line, where `ps` shows it |
| `sqlite://<path>` | `sqlite3 <path>` | A relative path is read from the project root. A file that does not exist is refused, because `sqlite3` would create an empty one without a word |
| `sqlite::memory:` | none | Refused: no other process can open an in-memory database |

The alias is `dbconsole`, the Rails name.

### Why there is no `brocanth shell`

`rails console` and `manage.py shell` evaluate the application's own language
against its loaded classes. Rust has no interpreter, and the types of a
project exist only inside its compiled binary. A REPL such as `evcxr`
compiles each line as a new crate. It cannot see the private modules of a
project, it cannot hold the state of a running application, and it builds
the dependency tree again on the first line. A command named `shell` would
promise a thing it cannot do.

A console has two uses, and each one has an honest tool:

- **Look at the data**: `brocanth dbshell`.
- **Run application code one time**: a project command in
  `src/commands/<name>.rs`, run as `brocanth <name>`. The compiler checks it
  against the project, and it is a file, so you can review it and run it
  again.

## 3. `brocanth check --deploy` (Django D23)

```text
brocanth check                         # the checks for every environment
brocanth check --deploy                # plus the production checks, on `production`
brocanth check --deploy --env staging  # another environment
brocanth check --deploy --allow deploy/hsts --strict
```

It reads `brocanth.toml`, loads the configuration of one environment through
the application's own loader, and reads `src/` as text. It builds nothing and
connects to nothing, so it runs in CI. Each finding has an id, a message, a
location when there is one, and a `help:` line:

```text
error[deploy/cookie-insecure]: `[auth] secure` is false, so the cookie also travels over plain HTTP
 --> config/production.toml:4
  = help: set `secure = true` under `[auth]` in config/production.toml, or $SHOP_AUTH__SECURE=true
```

| Id | Severity | When |
|---|---|---|
| `check/dialect` | error | `[policy] dialect` is not the database of `[model] url` (every environment) |
| `deploy/env` | warning | `--deploy` reads `development` or `test` |
| `deploy/no-config` | warning | no file and no variable configures the environment |
| `deploy/cookie-insecure` | error | `[sessions] secure` or `[auth] secure` is `false` |
| `deploy/cookie-prefix` | warning | `[auth] cookie_name` has no `__Host-` prefix |
| `deploy/signing-key` | error | the variable that `[auth] signing_key_env` names is unset, or shorter than 32 bytes |
| `deploy/default-secret` | error | `[storage] secret` is the scaffold's development string, or shorter than 32 characters |
| `deploy/secret-in-file` | warning | a key named like a secret has its value in a file (the value is never printed) |
| `deploy/debug-logging` | warning | `[logging] level` is `debug` or `trace` |
| `deploy/jobs-migrate` | warning | `[jobs] migrate = true` |
| `deploy/database` | error | `[model]`, `[cache]` or `[jobs] url` is in memory or under a temporary directory |
| `deploy/csrf-off` | error | the project has `sessions` or `auth`, and no `Csrf` hook is registered |
| `deploy/hooks-not-run` | error | a hook is registered, and nothing in `src/` serves through `brocanth_server::serve` or `Bridge` |
| `deploy/hsts` | warning | nothing in `src/` sets `Strict-Transport-Security` |
| `deploy/admin-unscoped` | error | `src/` calls `unscoped_rows_unsafe_for_development()` |

The exit code is 1 when an error remains. `--strict` makes a warning fail
too. `--allow <ID>` silences one finding, like Django's
`SILENCED_SYSTEM_CHECKS`. It is on the command line, where a reviewer sees
it.

Limits:

- **The source checks read text, not types.** A line comment is not code.
  Every other line that contains the name is. If this scan cannot see a
  registration, it reports it as absent. Look at the code, then use
  `--allow`.
- **A scaffolded project fails `deploy/csrf-off` today.** That is correct:
  the scaffold writes the CSRF registration as a comment, and that comment
  says the two changes that make it live.
- **brocanth has no security-headers hook.** `deploy/hsts` says so. Set the
  header at the TLS terminator and allow the finding, or write a
  `ResponseHook`.
- **It does not stop the application.** A plugin's `configure` can already
  refuse to start. This command is the earlier, offline half.

## 4. `dp-prefetch` (Astro A22)

`crates/brocanth-middleware/assets/dpprefetch.js`, published as
`brocanth_middleware::dp::PREFETCH`. Put it inline in a `<script>` or serve
it beside dp-swap. It needs neither dp-swap nor a server half.

```html
<a href="/posts/7" dp-prefetch>                 <!-- hover, the default -->
<a href="/posts/7" dp-prefetch="tap">           <!-- on the press -->
<nav dp-prefetch="viewport">...</nav>           <!-- when a link scrolls into view -->
<a href="/pricing" dp-prefetch="load">          <!-- when the page has loaded -->
<html dp-prefetch="hover">                      <!-- every link on the page -->
<a href="/logout" dp-prefetch="false">          <!-- never this one -->
```

| Astro | brocanth |
|---|---|
| `data-astro-prefetch` | `dp-prefetch`, inherited from the nearest ancestor, like `dp-target` |
| `prefetchAll` + `defaultStrategy` | `dp-prefetch="<strategy>"` on `<html>` |
| `hover`, `tap`, `viewport`, `load` | the same four. Hover waits 80 ms and includes keyboard focus. A hover link also prefetches on the press, because a touch screen has no hover |
| `prefetch(url)` | `dpprefetch.prefetch(url)` |

The rules:

- **GET links on this origin only.** An `<a href>` over http or https, on
  `location.origin`, with no `download` and no `dp-post`, `dp-put`,
  `dp-patch` or `dp-delete`. A link to this page with another `#hash` is
  skipped.
- **Save-Data is respected.** Nothing is prefetched when
  `navigator.connection.saveData` is on (that is where the `Save-Data`
  request header comes from) or the connection reports 2g.
- **One request per URL per page.** It is `<link rel="prefetch">` where the
  browser supports it, else a low-priority `fetch` with the page's cookies.
- **Links that dp-swap brings in are seen.** Viewport and load links are
  scanned again on `dp:afterSettle`.
- **It loads once.** A second copy on the page returns at once, through the
  same `window.__dp…` guard that `dpmutate.js` uses.

What it does not do: dp-swap sends `DP-Request`, and the server answers with
`Vary`. So a prefetched whole page does not answer a dp-swap partial
request. Prefetch helps a plain navigation, and a boosted link that falls
back to one.

Tests: JavaScript has no test harness here. `tests/prefetch.rs` checks the
script as text, runs `node --check` on it, and runs it in `node` over a stub
document for the link rules. Without `node`, the last two say SKIPPED on
stderr.
