# brocanth-cli

`brocanth-cli` builds `brocanth`, the command line of the framework. It
writes a new project from a list of modules, adds a module to a project
that exists, checks the modules a project is missing, and copies a
shipped admin screen into a project. Inside a project it also runs the
project's own commands: `migrate`, `dbshell`, `settings` and the
configuration checks live in the project, not here. Use it to start and grow a brocanth application.
It is a binary, not a library: an application never depends on it. A
generated project's own binary (`my-app serve`, `my-app settings`) comes
from `brocanth-command`, not from this crate.

## Getting started

brocanth is not published. Install the CLI from a checkout:

```text
cargo install --path crates/brocanth-cli --profile dist
```

The `dist` profile strips symbols and debug info and uses thin LTO: about
2.6 MB instead of 3.7 MB with plain `release`. It is a named profile, not
`[profile.release]`, because a workspace profile applies to every member
and an application built in release should keep its symbols.

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

One function, `create::selection`, holds these rules. `create` (flags and
chooser), `add` and the generator itself (`create::scaffold`) all call it,
so no path writes a project that a refusal would stop. The `help:` line
fits the command: `--with rsx,rsx-tailwind` for `create`,
`brocanth add rsx rsx-tailwind` for `add`.

#### The chooser

The chooser has two tabs over one answer. **Expert** shows every module,
grouped by its main category (Middleware, Renderer, External, Enhancer,
Routes) and nested under its parent. A child stays under its parent, even
when its own category differs. The arrows follow the rows as drawn. **Wizard** asks one question per screen:
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
| A static site | `static`, `ci`, `ci-github`, `config`, `commands`, `rsx`, `rsx-tailwind`, `rsx-htmx`, `robots` | Data, Behaviour, Doors |
| Nothing yet | nothing | |

### `brocanth add`

Adds modules to a project that exists: `brocanth add jobs,cache`.

It runs the same generator as `create`, with more entries in the module
list of `brocanth.toml`. It rewrites the files it owns
(`src/plugins/mod.rs`, `src/commands/mod.rs`, `src/main.rs`,
`brocanth.toml`). It appends to `Cargo.toml` and `config/<env>.toml`, and
leaves every other byte alone. It writes your files when they are absent,
and rewrites one only while it still holds exactly what the generator
wrote: then nobody edited it. It refuses when a module would change a file
of yours that you edited, prints the change, and writes nothing. Make the
change by hand, keep your own lines, and run it again: it then goes
through. `--force` rewrites an owned file that you edited, and discards
the edit.

Every module is generated again, so a module that adapts to the new one
writes its adaptation too. `ci-github` adapts to `jobs`: in a project made
with the default modules, `brocanth add jobs` also rewrites
`.github/workflows/ci.yml`, if you have not edited it.

### `brocanth remove`

Takes modules out of a project: `brocanth remove jobs,cache`. It is the
inverse of `add`, and it runs the same generator with fewer entries in
the module list.

It rewrites the files it owns, and deletes the ones that only the removed
module needed. It takes the module's dependencies and features out of
`Cargo.toml`, and its sections out of `config/<env>.toml`. It deletes or
rewrites a file of yours only while it still holds exactly what the
generator wrote. Otherwise it refuses, prints your edit, and writes
nothing. A config section with a value you changed is refused too: copy
what you need, delete the section, and run it again.

It refuses to remove a module that a remaining module needs, or that is
the parent of a remaining module. The `help:` line names the modules to
remove with it: `brocanth remove model db`.

It keeps three things, so that a later `brocanth add` finds them again:

- **The module file, once it is edited.** `src/modules/<key>.rs` holds the
  module's choices: renames, hooks, integrations. An unedited one is
  deleted. An edited one stays, out of `src/modules/mod.rs`, so nothing
  compiles it. `add` writes a module file only when it is missing, so it
  finds the old choices again.
- **Ejected admin screens.** The copies under
  `src/plugins/admin/screens/` stay, because they are your work. Nothing
  compiles them while the admin is out. `brocanth add admin` installs
  them again.
- **Migrations.** `migrations/` is the history of a database that still
  has the tables. `remove model` does not delete it, and `add model`
  writes around it.

### Module versions

`brocanth.toml` records the version of each module that wrote the
project: `[modules.<key>] version`. A module from the brocanth workspace
has the CLI's version, because every crate in the workspace has the
workspace version. A module from another repository declares its own
crate's version with `Manifest::version`. The admin declares it too,
because `brocanth admin eject` writes the same number into the header of
each screen it copies.

`add` and `remove` write the version again. `brocanth check` warns
(`check/module-version`) when the installed CLI writes a module at another
version than the recorded one. `add` and `remove` compare each file with
what the installed CLI writes, so after an upgrade a file that you did not
edit can read as edited.

### Migrations: a command of the project, not of `brocanth`

`brocanth` has no `migrate` command. The `model` module (SeaORM) writes
the migration commands into the project, because only a project with an
ORM has migrations, and because `ids` and `make` parse Rust source: the
parser (`syn`) stays out of this CLI.

| Command | Where it runs | Does |
|---|---|---|
| `migrate ids` | the `migrate` tool, `src/bin/migrate.rs` | Writes an id on each model and field that has none. An id never changes, so a rename keeps it. |
| `migrate make <NAME>` | the `migrate` tool | Compares the models with `migrations/schema.json` by id. Writes `migrations/m<UTC time>_<NAME>.rs` and a new snapshot, and lists it in `migrations/mod.rs`. Applies nothing. `--snapshot-only` writes the snapshot alone. |
| `migrate sync` | the `migrate` tool | Lists every `migrations/m*.rs` in `migrations/mod.rs`, for a file written by hand. |
| `migrate up` | the application's binary | Applies each pending migration. Needs `commands`. |
| `migrate down [N]` | the application's binary | Rolls back the newest `N` migrations (default 1). |
| `migrate status` | the application's binary | Lists each migration as applied or pending. |

With `commands`, all six are one command of the project:
`cargo run -- migrate make add_posts`. `brocanth migrate make add_posts`
still works: `brocanth` hands every command it does not know to the
project's binary. Without `commands`, run the tool directly:
`cargo run --bin migrate -- make add_posts`.

`up` refuses when a file in `migrations/` is not in `migrations/mod.rs`:
the binary was built without it. Run `migrate sync`, then `up` again.

See [docs/migrations.md](../../docs/migrations.md).

### `brocanth check`

Two halves. The CLI runs the module checks: a module that the chosen ones
recommend and the project lacks, and a module that `brocanth.toml`
records at another version than the installed CLI writes. Then it runs the project's own `check`
command with the same arguments, which reads the configuration of one
environment and `src/` as text. That half exists when the project has
`config` and `commands`; it lives in the project because only the project
knows how its configuration is read, and the CLI links no configuration
loader.

- `--deploy` adds the production checks: secrets, cookies, CSRF, HSTS,
  the admin, the database. The environment is then `production`.
- `--env <ENV>` loads another environment.
- `--allow <ID>` silences one finding, such as `deploy/hsts`. Repeat it.
- `--strict` exits 1 on a warning too.

Each finding has an id, a location and a `help:` line. The exit code is 1
when an error remains. See [docs/tooling.md](../../docs/tooling.md).

### Project commands: `dbshell`, `migrate`, `settings`

These are commands of the generated project, and `brocanth <name>` hands
them to the project's binary, like any project command:

- `dbshell` (alias `dbconsole`) comes with `db` and `commands`. It opens
  `psql` or `sqlite3` on `[db] url`; a Postgres password goes in
  `PGPASSWORD`, not on the command line.
- `migrate` comes with `model`.
- `settings` prints every configuration layer and the merged values.

### `brocanth admin`

Works on the project's admin. It has two subcommands, `eject` and `diff`.
It needs the `admin` module.

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
| `SCREEN` | One of `layout`, `index`, `list`, `detail`, `form`, `delete`, `error`, or a block of the layout: `brand`, `userlinks`. |
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
- `--model` on `layout`, `index`, `error`, `brand` or `userlinks`
  (`admin/not-per-model`);
- a model key that is not a Rust module name, such as `blog-post`
  (`admin/model-key`), because `rsx` turns each directory into a module.

When `mount` no longer has the line `let admin = Arc::new(Admin::new(`,
it writes the copy and warns (`admin/not-installed`): add the install line
yourself. With the `commands` module, the generated project
has `admin eject` and `admin diff` too (`src/commands/admin.rs`). It runs
this command in the project root with the same arguments and exits with
its status. See level 8 of
[docs/admin-customization.md](../../docs/admin-customization.md).

`brocanth admin diff [SCREEN] [--model <KEY>] [--shipped <DIR>]` compares
ejected copies with the shipped screens, after an upgrade. It prints a
unified diff from the shipped screen to each copy, or every copy without
`SCREEN`.

```text
brocanth admin diff                       # every ejected copy
brocanth admin diff list --model invoice  # one copy
```

| Argument | Meaning |
|---|---|
| `SCREEN` | The copy to compare. Without it, every copy on disk. |
| `--model <KEY>` | The copy for this model. Needs `SCREEN`. |
| `--shipped <DIR>` | A `brocanth-admin` directory to compare with, not the installed one. |

The shipped screen is the template of the `brocanth-admin` the project
installed, found with `cargo metadata --offline`. When cargo cannot say,
the command warns (`admin/no-installed`) and compares with the screens
this CLI carries. It prints the version each copy came from, from its
header line, and the version it compares with. When the two differ, the
diff holds your edits and the release's changes together. It exits 0
whether the copies differ or not.

It refuses a screen that is not ejected (`admin/not-ejected`, with the
eject command in its help line) and a `--shipped` directory with no
`src/templates/` (`admin/shipped`).

### A project's own commands

Inside a project, every other subcommand belongs to the project:
`brocanth seed_demo --count 3`. `brocanth` finds `brocanth.toml`, runs
`cargo build`, asks `cargo metadata` for the binary, and runs it (`exec`
on Unix). A built project's commands are listed in `brocanth --help`. A
project without the `commands` module has no command line, and
`brocanth` says `brocanth add commands`. See
[brocanth-command](../brocanth-command/README.md).

## Modules

The CLI offers 47 modules, grouped here by their main category
([docs/module-map.md](../../docs/module-map.md) has every category, the
seams and the dependency graph). `brocanth create --help` prints each
module with what it costs.

| Category | Hooks in through | Modules |
|---|---|---|
| Middleware | request and response hooks, layers, context items | `logging`, `observability`, `observability-logs`, `observability-tracing`, `analytics`, `beacon`, `sessions`, `auth`, `policy`, `validation`, `middleware`, `middleware-csrf`, `middleware-security-headers`, `middleware-rate-limit`, `middleware-conditional-get`, `middleware-compression`, `i18n`, `i18n-routing`, `gtag` |
| Routes | a mount at a prefix, or a page file | `observability-health`, `observability-metrics`, `openapi`, `resource`, `admin`, `admin-model`, `realtime-sse`, `realtime-websocket`, `robots`, `sitemap`, `search` |
| Enhancer | a plugin with a lifecycle, a subcommand, settings | `config`, `db`, `model`, `cache`, `jobs`, `jobs-memory`, `storage`, `realtime`, `input`, `client`, `commands` |
| Renderer | a file type in `src/pages`, or a function on `pages` | `rsx`, `rsx-htmx`, `jt`, `sanitize`, `pagination`, `filters`, `meta`, `errors` |
| External | files only: build, ship, check | `rsx-tailwind`, `static`, `docker`, `docker-kubernetes`, `ci`, `ci-github` |

A module that is listed and not available yet gives the reason on its
row; none is blocked today. A module from another crate
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
- **A module crate's own goldens.** The crate is also a library,
  `brocanth_cli::testing`, so a module crate generates projects in its
  own tests. `brocanth-robots` keeps its two shapes in
  `crates/brocanth-robots/tests/golden/`. Bless them the same way:
  `BROCANTH_BLESS=1 cargo test -p brocanth-robots --test scaffold`.
- **Binary tests.** `tests/migrate.rs` and `tests/plain_text.rs` run the
  built `brocanth`
  over projects on disk. The configuration checks are tested in
  `brocanth-config` (`src/check.rs`) and `dbshell` in `brocanth-db`
  (`src/shell.rs`), the crates that own them. `tests/lean.rs` fails if the
  CLI's graph regains a runtime crate. `tests/readme.rs` checks that this README names
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
- [docs/migrations.md](../../docs/migrations.md): the `migrate` command a project with `model` gets.
- [docs/tooling.md](../../docs/tooling.md): `dbshell` and `check`.
- [docs/admin-customization.md](../../docs/admin-customization.md):
  `brocanth admin eject` (level 8).
