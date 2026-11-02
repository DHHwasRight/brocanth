# brocanth-cli

`brocanth-cli` builds `brocanth`, the command line of the framework. It
writes a new project from a list of modules, adds a module to a project
that exists, writes migrations, opens the database client, checks a
configuration before a deploy, and copies a shipped admin screen into a
project. Inside a project it also runs the
project's own commands. Use it to start and grow a brocanth application.
It is a binary, not a library: an application never depends on it. A
generated project's own binary (`my-app serve`, `my-app settings`) comes
from `brocanth-command`, not from this crate.

## Getting started

brocanth is not published. Install the CLI from a checkout:

```text
cargo install --path crates/brocanth-cli
```

To work on the CLI itself, use `cargo run -p brocanth-cli --` in place of
`brocanth`.

```text
brocanth create my-app                       # the default modules, no questions
brocanth create my-app --with config,model,rsx
brocanth create                              # no name on a terminal: the chooser
cd my-app && brocanth add sessions
brocanth check --deploy
```

## Commands

Global options, for every command:

| Option | Meaning |
|---|---|
| `-C, --dir <DIR>` | The package root. Default: the nearest directory with a `Cargo.toml` and `src/pages`. |
| `--no-color` | Never colour the output. `NO_COLOR` does the same, and `CLICOLOR_FORCE` forces colour. |
| `-q, --quiet` | Print only the result and what is wrong with it. |
| `-D, --debug` | After the chooser, print the command line that gives the same answers with no questions. |

Each error carries a `help:` line that says what to do next.

### `brocanth create`

Writes a new project. Alias: `brocanth new`.

| Argument | Meaning |
|---|---|
| `NAME` | The directory to create, and the cargo package name. |
| `--title <TEXT>` | The human name, used in `[app] name` and on the pages. Default: `NAME` in title case. |
| `--with <MODULE>` | The modules to include. Repeat the flag or separate with commas. |
| `--core-only` | The core alone: a router, a context, nothing else. |
| `--brocanth-path <DIR>` | Depend on a local checkout. `DIR` is `crates/brocanth`, not the checkout root. |
| `--git` / `--no-git` | Make a git repository, or not. Default: no repository. |
| `--force` | Write into a directory that exists. |

Without a `NAME`, and with a terminal on stdin and stdout, `create` opens
the chooser. Without a terminal it stops and names the flag, because a
prompt in CI waits until it times out. The chooser can set nothing that a
flag cannot.

With a `NAME` and no `--with`, `create` takes the default modules:
`observability`, `observability-logs`, `observability-health`,
`observability-metrics`, `config`, `commands`, `docker`, `ci`,
`ci-github`, `middleware`, `middleware-security-headers`.

`--with` never adds a module you did not name. A module that `needs`
another is refused without it, and the error names the fix. These picks
are refused too:

- `logging` with `observability-logs`: both write a line per request.
- `static` with any module that needs a running server.
- A child (`rsx-tailwind`) without its parent (`rsx`).
- A module marked "not available yet".

#### The chooser

The chooser has two tabs over one answer. **Expert** shows every module,
nested under its parent. **Wizard** asks one question per screen:
Shape, Foundations, Data, Pages, Behaviour, Doors, Operations, and Extras
for a module that names no question. A module picks its question with
`Manifest::asked_in`.

The first question is the shape. Each shape ticks a set of modules, which
you then edit, and skips the questions it makes pointless:

| Shape | Ticks | Skips |
|---|---|---|
| An HTML application | `observability` (+ `-logs`, `-health`, `-metrics`), `docker`, `ci`, `ci-github`, `config`, `commands`, `model`, `sessions`, `rsx`, `rsx-tailwind`, `rsx-htmx` | |
| A JSON API | `observability` (+ `-logs`, `-health`, `-metrics`), `docker`, `ci`, `ci-github`, `config`, `commands`, `model`, `validation`, `openapi` | Pages |
| Something that does work | `observability` (+ `-logs`, `-health`, `-metrics`), `docker`, `ci`, `ci-github`, `config`, `commands`, `model`, `jobs`, `cache` | Pages |
| A static site | `static`, `ci`, `ci-github`, `config`, `commands`, `rsx`, `rsx-tailwind`, `rsx-htmx` | Data, Behaviour, Doors |
| Nothing yet | nothing | |

### `brocanth add`

Adds modules to a project that exists: `brocanth add jobs,cache`.

It runs the same generator as `create`, with more entries in the module
list of `brocanth.toml`. It rewrites the files it owns
(`src/plugins/mod.rs`, `src/commands/mod.rs`, `src/main.rs`,
`brocanth.toml`). It appends to `Cargo.toml` and `config/<env>.toml`, and
leaves every other byte alone. It writes your files only when they are
absent. It refuses when a module would change a file you own, and prints
what it would have written. `--force` rewrites an owned file that you
edited, and discards the edit.

`ci-github` writes `.github/workflows/ci.yml`, which is yours, and adapts
to `model`, `jobs` and `cache`. So in a project made with the default
modules, `brocanth add jobs` is refused until you apply the printed
workflow change by hand.

### `brocanth migrate`

Writes migrations from the models in `src/models/`.

| Subcommand | Does |
|---|---|
| `ids` | Writes an id on each model and field that has none. An id never changes, so a rename keeps it. |
| `make <NAME>` | Compares the models with `migrations/schema.json` by id. Writes `migrations/m<UTC time>_<NAME>.rs` and a new snapshot. Applies nothing. `--snapshot-only` writes the snapshot alone. |
| `up` | Applies each pending migration, through the project's binary. Needs `model` and `commands`. |
| `down [N]` | Rolls back the newest `N` migrations (default 1). |
| `status` | Lists each migration as applied or pending. |

See [docs/migrations.md](../../docs/migrations.md).

### `brocanth dbshell`

Opens `psql` or `sqlite3` on the database in `[model] url`. Alias:
`brocanth dbconsole`. `--env <ENV>` picks the environment (default
`$<APP>_ENV`, else `development`). `--print` prints the command line
without the password. Arguments after `--` go to the client. A Postgres
password goes in `PGPASSWORD`, not on the command line. See
[docs/tooling.md](../../docs/tooling.md).

### `brocanth check`

Reads `brocanth.toml`, the configuration of one environment, and `src/`
as text. It builds nothing and connects to nothing, so it runs in CI.

- `--deploy` adds the production checks: secrets, cookies, CSRF, HSTS,
  the admin, the database. The environment is then `production`.
- `--env <ENV>` loads another environment.
- `--allow <ID>` silences one finding, such as `deploy/hsts`. Repeat it.
- `--strict` exits 1 on a warning too.

Each finding has an id, a location and a `help:` line. The exit code is 1
when an error remains. The ids are `check/dialect` and `deploy/env`,
`deploy/no-config`, `deploy/cookie-insecure`, `deploy/cookie-prefix`,
`deploy/signing-key`, `deploy/default-secret`, `deploy/secret-in-file`,
`deploy/debug-logging`, `deploy/jobs-migrate`, `deploy/database`,
`deploy/csrf-off`, `deploy/hooks-not-run`, `deploy/hsts`,
`deploy/admin-unscoped`. See [docs/tooling.md](../../docs/tooling.md).

### `brocanth admin`

Works on the project's admin. It has one subcommand, `eject`. It needs the
`admin` module.

`brocanth admin eject <SCREEN>` copies a shipped admin screen into the
project and installs it over the default. The copy renders the same HTML
until you edit it. It is yours: an upgrade of `brocanth-admin` does not
change it.

```text
brocanth admin eject list                  # every model's list
brocanth admin eject list --model invoice  # the invoice list only
brocanth admin eject layout                # the shell around every screen
```

| Argument | Meaning |
|---|---|
| `SCREEN` | One of `layout`, `index`, `list`, `detail`, `form`, `delete`, `error`. |
| `--model <KEY>` | Install the copy for this model only. Only `list`, `detail`, `form` and `delete` take it. |

It writes `src/plugins/admin/screens/<screen>.rsx`, or
`src/plugins/admin/screens/models/<model>/<screen>.rsx` with `--model`.
It writes `src/plugins/admin/ejected.rs` again from the copies on disk.
It adds `rsx` to `[dependencies]`, `mod ejected;` to
`src/plugins/admin/mod.rs`, and `let config = ejected::apply(config);` in
`mount`, each once and only when missing. To drop a copy, delete its file
and run any eject again.

It refuses:

- a project without the `admin` module (`admin/no-admin`);
- a screen that is already ejected (`admin/exists`). Edit that file, or
  delete it to start again from the shipped screen;
- `--model` on `layout`, `index` or `error` (`admin/not-per-model`);
- a model key that is not a Rust module name, such as `blog-post`
  (`admin/model-key`), because `rsx` turns each directory into a module.

When `mount` no longer has the line `let admin = Arc::new(Admin::new(`,
it writes the copy and warns (`admin/not-installed`): add the install line
yourself. With the `commands` module, the generated project
has `admin eject` too (`src/commands/admin.rs`). It runs this command in
the project root with the same arguments and exits with its status. See level 8 of
[docs/admin-customization.md](../../docs/admin-customization.md).

### A project's own commands

Inside a project, every other subcommand belongs to the project:
`brocanth seed_demo --count 3`. `brocanth` finds `brocanth.toml`, runs
`cargo build`, asks `cargo metadata` for the binary, and runs it (`exec`
on Unix). A built project's commands are listed in `brocanth --help`. A
project without the `commands` module has no command line, and
`brocanth` says `brocanth add commands`. See
[brocanth-command](../brocanth-command/README.md).

## Modules

The CLI offers 38 modules, grouped here by their main category
([docs/module-map.md](../../docs/module-map.md) has every category, the
seams and the dependency graph). `brocanth create --help` prints each
module with what it costs.

| Category | Hooks in through | Modules |
|---|---|---|
| Middleware | request and response hooks, layers, context items | `logging`, `observability`, `observability-logs`, `observability-tracing`, `sessions`, `auth`, `policy`, `validation`, `middleware`, `middleware-csrf`, `middleware-security-headers`, `middleware-rate-limit` |
| Routes | a mount at a prefix, or a page file | `observability-health`, `observability-metrics`, `openapi`, `resource`, `admin`, `admin-model`, `realtime-sse`, `realtime-websocket` |
| Enhancer | a plugin with a lifecycle, a subcommand, settings | `config`, `model`, `cache`, `jobs`, `storage`, `realtime`, `input`, `client`, `commands` |
| Renderer | a file type in `src/pages`, or a function on `pages` | `rsx`, `rsx-htmx`, `jt` |
| External | files only: build, ship, check | `rsx-tailwind`, `static`, `docker`, `docker-kubernetes`, `ci`, `ci-github` |

`admin-model`, `realtime-sse` and `realtime-websocket` are listed and not
available yet; the reason is on their row. A module from another crate
joins the list with two lines in this crate: a dependency with
`features = ["scaffold"]`, and an entry in `ALL` in
`src/modules/mod.rs`. See [docs/module-authoring.md](../../docs/module-authoring.md).

Each setting a module declares reaches the generated binary as a flag
`--<section>.<key>`, a variable `$<APP>_<SECTION>__<KEY>`, and with
`config` a key in `config/<env>.toml`. See
[docs/module-flags.md](../../docs/module-flags.md).

## Testing

```text
cargo test -p brocanth-cli
```

No database and no environment variable is needed.

- **Goldens.** `the_generator_writes_what_it_wrote_yesterday` in
  `src/commands/create.rs` generates each shape in `shapes()` and
  compares it with `tests/golden/<shape>.txt`. The `everything` shape
  takes every module that can stand together, so a new module lands
  there with no test change. When the generator changes, bless the
  goldens and read the diff before you commit it:

  ```text
  BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- generator_writes
  git diff crates/brocanth-cli/tests/golden/
  ```

  A blessed snapshot that nobody read asserts whatever the bug does.
- **Formatting.** `every_generated_rust_file_is_already_formatted` runs
  rustfmt over every generated `.rs` file. A file must come out the same.
- **Modules.** `src/modules/mod.rs` checks each manifest in `ALL`.
- **Binary tests.** `tests/check.rs`, `tests/dbshell.rs`,
  `tests/migrate.rs` and `tests/plain_text.rs` run the built `brocanth`
  over projects on disk. `tests/readme.rs` checks that this README names
  every command and every module that `brocanth --help` lists, and no
  command that does not exist.
- **Slow tests.** Some tests generate a project and run `cargo build` on
  it. They take minutes and are `#[ignore]`d:

  ```text
  cargo test -p brocanth-cli -- --ignored
  ```

The binary crate has no doctests. Its examples are shell commands, and
`tests/readme.rs` keeps them true.

## Links

- [docs/getting-started.md](../../docs/getting-started.md): a first
  project, with real output.
- [docs/module-map.md](../../docs/module-map.md): the modules by
  category, and the dependency graph.
- [docs/module-authoring.md](../../docs/module-authoring.md): write a
  module.
- [docs/module-flags.md](../../docs/module-flags.md): settings, flags and
  the settings report.
- [docs/migrations.md](../../docs/migrations.md): `brocanth migrate`.
- [docs/tooling.md](../../docs/tooling.md): `dbshell` and `check`.
- [docs/admin-customization.md](../../docs/admin-customization.md):
  `brocanth admin eject` (level 8).
