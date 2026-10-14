# Getting started

Every command here was run against this checkout on 2026-10-13, and the output
is pasted rather than recalled. Where something is not built yet it says so
instead of describing what it will do.

## Install the CLI

brocanth is not published, so the CLI comes from this checkout:

```
cargo install --path crates/brocanth-cli
```

That installs one binary, `brocanth`. To work on the CLI itself, skip the
install and use `cargo run -p brocanth-cli --` in front of every command below.

## Make a project

```
brocanth create blog --brocanth-path /path/to/brocanth/crates/brocanth
```

`--brocanth-path` is needed for the same reason as the install: nothing is on
crates.io yet, so the generated manifest has to point at your checkout. **It
takes the core crate's own directory — `crates/brocanth` — not the checkout
root**, which is a virtual manifest cargo will not take a path dependency on.
Passing the root is caught and answered with the command you meant:

```
error[brocanth-path]: /path/to/brocanth is a workspace, not a crate
  = note: cargo cannot take a path dependency on a virtual manifest
  = help: --brocanth-path /path/to/brocanth/crates/brocanth
```

Run with no name and a terminal on both stdin and stdout, `brocanth create`
opens a chooser instead. With a name it runs straight through, because every
other answer has a default.

What it writes:

```
  blog/Cargo.toml                848 bytes
  blog/.gitignore                  8 bytes
  blog/rust-toolchain.toml        29 bytes
  blog/mise.toml                 263 bytes
  blog/src/main.rs              4944 bytes
  blog/src/pages/mod.rs         3654 bytes
  blog/src/pages/index.rs        622 bytes
  blog/src/pages/about.rs        675 bytes
  blog/src/pages/users/[id].rs  1143 bytes
  blog/src/plugins/mod.rs        263 bytes
  blog/src/plugins/logging.rs   2364 bytes
  blog/config/development.toml   560 bytes
  blog/brocanth.toml              24 bytes
  blog/.cargo/config.toml        110 bytes
  blog/src/bin/manage.rs         556 bytes
  blog/src/commands/mod.rs          551 bytes
  blog/AGENTS.md                2696 bytes

    core      a router handing a request and a context to a handler
    logging   a line per request; the core assumes no log layer
    config    config/<env>.toml, merged; without it nothing links a TOML parser
    commands     a `manage` binary; a struct with #[cmd] becomes a subcommand
    sessions  not asked for: absent from the source, the binary and the lock file
    model     not asked for: absent from the source, the binary and the lock file
```

Those last two lines are the point of the whole design, not a progress
message. A capability nobody asked for is not compiled, not linked, and has no
entry in `Cargo.lock`. `cargo tree` on a fresh project will not show an ORM or
a SQL driver.

### `AGENTS.md`, and why the scaffold writes one

brocanth is meant to be written by a language model as much as by a person, so
every project gets a brief for one. It is generated rather than copied,
because the useful part is **this project's** facts:

```
**Absent:** `sessions`, `model` — not compiled, not linked, absent from
`Cargo.lock`. Do not write code that uses them; add the dependency and name
the plugin in `main` first.
```

A model that cannot see what is missing assumes the framework has it, writes
against it, and then tries to fix the compile error by guessing harder.
Naming the gap turns that into "add the dependency first". The brief also
carries the check-your-work loop, how to add a route, and the house style.

`docs/llm-authoring.md` in this checkout is the longer version: what cannot
compile and can therefore be relied on, what can still go wrong and therefore
needs a test, and a map of the crates.

### Choosing what goes in

```
brocanth create blog --with logging,config,commands,sessions,model
brocanth create blog --core-only
```

The default is `logging,config,commands`. `--core-only` gives a router, a context
and nothing else.

| `--with` | What it adds |
|---|---|
| `logging` | A request logger. The core assumes no log layer at all. |
| `config` | `config/<env>.toml`, four sources merged, typed per section. |
| `sessions` | Cookie sessions, and a `Session` on the context. |
| `model` | The model layer: SeaORM, validations and migrations. **Postgres by default** — see the warning at the end. |
| `commands` | A `manage` binary whose subcommands are your own structs. |

## Run it

```
cd blog
cargo run
```

```
[logging] level debug
blog: plugins: [config, logging]
GET / -> 200
GET    /           200 <!doctype html><title>Blog</title><h1>Blog</h1><
GET /users/7 -> 200
GET    /users/7    200 <!doctype html><title>User 7</title><h1>User 7</
DELETE /about      405 Allow: GET, HEAD
GET    /nowhere    404 Not Found
```

**That is a real socket.** Open `http://127.0.0.1:3000` in a browser, or:

```
$ curl -i -X DELETE http://127.0.0.1:3000/about
HTTP/1.1 405 Method Not Allowed
allow: GET, HEAD
```

A path that exists with a verb it does not answer is a 405 carrying an
`Allow` list, never a 404.

## See what this project can do

```
brocanth --help
```

```
Commands:
  create  Scaffold an application
  serve   Serve the application over HTTP
  help    Print this message or the help of the given subcommand(s)
```

`create` is `brocanth`'s own and the only one it has. Everything below it is
**your project's**, read out of the built binary — so a command you add appears
here once it has been compiled. Nothing is listed that this project cannot run.

Outside a project the same help shows `create` alone, because there is no
project to ask.

`-D` has exactly one job, and it belongs to `create`. Run with no name, the
chooser opens; `-D` then prints the command line that would have given the
same answers with nothing to ask:

```
$ brocanth -D create
debug: brocanth create blog --title Blog --with logging,config,commands --git
```

Copy that into a script and the same project comes out. It is written in
full rather than minimally, because a line that leans on a default changes
meaning when the default does. A run that was already driven by flags prints
nothing: it was already the line.

## Add a page

There is no generator, so this is two edits by hand. They are the two a
generator would have made.

**1. Write the file.** `src/pages/contact.rs`:

```rust
//! `/contact` -- one file, one route.

use crate::pages::{Ctx, Error, IntoResponse};

pub fn handle(_ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    Ok("<!doctype html><title>Contact</title><h1>Contact</h1>")
}
```

**2. Add two lines to `src/pages/mod.rs`** — the `mod`, and the row in the
table:

```rust
pub mod contact;
```

```rust
routes.file("contact.rs", "", Verb::Get, page(contact::handle))?;
```

`routes.file` names the *file* and the framework derives the URL from it, so
the table and the filesystem cannot drift apart. `page(..)` is what lets every
handler return its own type — see *What a handler returns* below. Then
`brocanth serve` and `curl http://127.0.0.1:3000/contact`.

A handler returns a value and never writes a response itself, which is what
makes a half-written body on an error impossible rather than unlikely.

## What a handler returns

`Result<impl IntoResponse, Error>`. Two halves, and both earn their place.

**The `Result`** is why `?` works. Anything that fails converts into `Error`
through a blanket `From`, so a parse or a query is one character:

```rust
let id: u64 = ctx.require("id")?.parse()?;
```

One place turns a failed handler into a response — what Echo calls an
`HTTPErrorHandler` — and it lives in `dispatch` in `src/main.rs`. The
difference from Echo is that the handler has no writer, so there is no
already-sent 200 for the error path to fail to take back.

**The `impl IntoResponse`** is the Crystal original's idea: a page returns the
simplest thing that says what it means. Each handler still returns exactly one
type; the trait is what lets *different* handlers return *different* ones.

| Return | Means |
|---|---|
| `Ok("<h1>Hi</h1>")` or `Ok(string)` | 200 with that body |
| `Ok(())` | 200, no body |
| `Ok(404u16)` | that status, no body |
| `Ok((body, 201))` | body and status |
| `Ok((body, 201, headers))` | body, status and headers |

Past that, build the struct. Rust requires **every** field — there are no
implicit zero values, so omitting one is a compile error, not a `0` — and `..`
fills in the rest:

```rust
Ok(Response { status: 201, ..Default::default() })          // 201, no body
Ok(Response { body: html, ..Default::default() })           // 200 with a body
Ok(Response { status: 404, ..Response::ok(page) })          // from any value
```

`Default` is written by hand rather than derived, and that matters: a derived
one would give `status: 0`, which is not an HTTP status, so
`Response { body: .., ..Default::default() }` would compile and be wrong.

For several headers, chain instead — Rust has no variadic generics, so a tuple
return is fixed-arity and this is the unbounded form:

```rust
Ok(Response::empty(303).with_header("location", "/users/7"))
```

## Add a command

Tasks are the part that is finished. A struct with `#[cmd]` on it becomes a
subcommand of your project's own binary — there is no list to add it to, because
registration happens at link time.

`src/commands/mod.rs` already has one:

```rust
/// Say hello, to prove the wiring works.
#[cmd]
#[derive(clap::Args)]
pub struct Greet {
    /// Who to greet.
    #[arg(long, default_value = "world")]
    pub name: String,
}

impl Command for Greet {
    fn run(self, ctx: &mut Context) -> anyhow::Result<()> {
        println!("hello, {} (env: {})", self.name, ctx.env());
        Ok(())
    }
}
```

Two ways to run it. Through the globally installed CLI, from anywhere inside
the project:

```
$ brocanth greet --name max
hello, max (env: development)
```

Or by running the binary, which is the same thing without the walk-up:

```
$ cargo run -- greet --name max
```

There is no cargo alias. A project's command line is a framework concern and
cargo is the build tool; hooking one into the other conflates them.

`brocanth` knows one command of its own, `create`, and treats everything else
as the project's: it walks up
for `brocanth.toml`, runs `cargo build`, asks `cargo metadata` where the binary
landed, and `exec`s it. So the command owns the terminal, its exit code is the one
your shell sees, and Ctrl-C reaches the command rather than a wrapper.

The help text comes from the doc comment, because `clap::Args::augment_args`
copies it onto the command. There is no second description to keep in step.

Put a new command in its own file under `src/commands/`, add a `mod` line, and it is
a subcommand.

### The one failure here that is silent

**A crate that is never referenced is dropped by the linker, and its commands go
with it.** Your own crate is fine — `main` is in it. A *third-party* crate whose
only job is to contribute commands is not: nothing in your code mentions it, so
nothing keeps it. Reference it explicitly in `src/bin/manage.rs`:

```rust
use some_task_pack as _;
```

Without that line it compiles, the tests pass, and the commands are simply absent
from `--help`.

## What is not built yet

**The global tool scaffolds and nothing else.** `brocanth` has one command of
its own, `create`. There is no `routes`, no `check`, no `build`, no `dev` and
no `generate`: they were removed, because a project's command line belongs to
the project, where it can be tuned without forking anything.

So adding a page is two edits by hand, as above, and watching for changes is
whatever you already use — `cargo watch`, `mise`, an editor task. If you want
`myapp routes`, write it: a struct with `#[cmd]` under `src/commands/` is a
subcommand, and the route table is `pages::router()`.

## One thing to know if you pass `--with model`

The model layer now defaults to **Postgres**. The generated manifest names
`brocanth-model` with no features, so a new project gets the Postgres driver
and expects a server. For the small single-machine setup, edit the dependency
yourself:

```toml
brocanth-model = { path = "...", default-features = false, features = ["sqlite"] }
```

The generator does not know to write that, which is a rough edge rather than a
decision.

## Where to go next

- `docs/llm-authoring.md` — the brief for writing brocanth with a language
  model: the 36 mistakes that cannot compile, the ones that still can, and the
  crate map. Worth reading even if you are writing by hand, because it is the
  shortest statement of what the framework guarantees.
- `PLAN.md` — the argument the whole project is built on. §8 (modularity) is
  the one that explains why a scaffold tells you what it left out.
- `crates/brocanth-command/README.md` — commands in full.
- `docs/async-handlers.md` — how a route gets to `.await`, what it costs, and
  what is still missing.
- `docs/pocketbase-surface.md` — the thing brocanth is measured against, with
  what is deliberately declined.
