# brocanth-command

Rails-style commands for a brocanth project. Define a struct in your own crate,
put `#[cmd]` on it, and it is a subcommand of your project's CLI — with its
arguments parsed and its `--help` written.

There is no list to add it to. Registration happens at **link time**, through a
[`linkme`](https://docs.rs/linkme) distributed slice, so nothing is loaded at
run time and nothing is scanned at build time.

## Define a command

```rust
use brocanth_command::{anyhow, clap, cmd, Context, Command};

/// Fill the demo database with sample rows.
#[cmd]
#[derive(clap::Args)]
pub struct SeedDemo {
    /// How many rows to write.
    #[arg(long, default_value_t = 100)]
    pub count: u32,
}

impl Command for SeedDemo {
    fn run(self, ctx: &mut Context) -> anyhow::Result<()> {
        println!("seeding {} rows into {}", self.count, ctx.env());
        Ok(())
    }
}
```

That is the whole of it. The name comes from the struct in `snake_case`, so
this is `seed_demo`. The help text comes from the doc comment, because
`clap::Args::augment_args` copies it onto the command — there is no second
description to keep in step.

For a name no Rust identifier can spell:

```rust
#[cmd(name = "cache:clear")]
```

Your project depends on `brocanth-command` alone. It re-exports `clap`, `linkme`
and `anyhow`, so none of those appear in your own `Cargo.toml`.

### Two traits, and why

`Task::run` is yours. `Registered::NAME` is the macro's. They are separate
traits because a single one could not hold both: an `impl` block has one
author, so you would have to write the name the macro exists to derive.

You never write `impl Registered` by hand. `entry` is bounded on it, so
forgetting `#[cmd]` is a type error at the registration site rather than a
command that quietly never appears.

## How `brocanth` delegates

`brocanth` is installed once, globally. The commands it can run are defined in a
project and compiled into that project's binary, so it cannot know them. It
hands off, the way `rails` hands off to `bin/rails`:

```
$ brocanth greet --name world
/path/to/my-project (brocanth.toml)
hello, world (env: development)
```

Four steps, and the order matters:

1. Walk up from the current directory for `brocanth.toml`. Without it there is
   no project, and guessing would run the wrong one.
2. `cargo build`, with cargo's stderr passed straight through. A command edited a
   second ago must be the one that runs, and cargo's own errors are better than
   any paraphrase.
3. Ask `cargo metadata` where the binary is. Guessing `target/debug/<name>` is
   wrong under `CARGO_TARGET_DIR`, inside a workspace, and on any profile but
   the default.
4. `exec` it on Unix, so the command owns the terminal, its exit code is the one
   your shell sees, and Ctrl-C reaches the command rather than a wrapper holding
   its hand. Elsewhere it is spawned and its exit code copied.

Subcommands `brocanth` knows itself — `create`, `routes`, `build`, `check`,
`generate`, `dev` — are handled directly and never delegated.

## Running one

```
$ brocanth --help
Commands:
  create  Scaffold an application
  serve   Serve the application over HTTP
  greet   Say hello, to prove the wiring works

$ brocanth greet --name brocanth
hello, brocanth (env: development)
```

`brocanth` is installed once and knows one command of its own, `create`.
Everything else it is given belongs to the project: it walks up for
`brocanth.toml`, builds, and execs the project's binary. A built project also
has its commands listed in that help, read out of the binary rather than
guessed at.

There is **no cargo alias**. A project's command line is a framework concern
and cargo is the build tool; hooking one into the other conflates them. Run
the binary directly if you would rather not go through `brocanth`.

## The linking gotcha

**A crate that is never referenced is dropped by the linker, and its commands go
with it.**

The slice is populated by the linker from the object files it actually keeps.
Your own crate is fine — `main` is in it. A *third-party* crate whose only
purpose is to contribute commands is not: nothing in your code mentions it, so
nothing keeps it.

Reference it explicitly:

```rust
// in src/bin/manage.rs. The `as _` imports no names; it only tells the
// linker the crate is wanted, which keeps the commands it registers.
use some_task_pack as _;
```

Without that line the crate compiles, the tests pass, and the commands are simply
absent from `--help`. It is the one failure here that is silent, which is why
it has its own section.

## Duplicate names

Two commands under one name is an error, reported before any command is built and
naming **every** collision rather than only the first:

```
two or more commands share a name: migrate, seed.
Every command name must be unique across this binary and every crate it links.
Rename one with `#[cmd(name = "...")]`.
```

clap's own behaviour for a duplicate subcommand is to keep one silently, and a
command that vanished is far worse to debug than one that refused to start.

## Testing

```
cargo test --workspace
```

- Unit tests cover duplicate detection, alphabetical ordering, and the derived
  name.
- `trybuild` snapshots cover the macro's errors — on an enum, and on an unknown
  argument. A macro that merely *fails* is not enough: the failure is the only
  thing the author sees.
- `assert_cmd` tests run the real `demo-app` binary, which is the only way to
  see what the linker produced. A unit test can call the runner with a slice
  built by hand; only a built binary shows whether `#[cmd]` in two separate
  modules actually reached the registry.
- One `#[ignore]`d test scaffolds a project, compiles it, and runs its example
  command. It is ignored because it takes minutes, not milliseconds.

## Next step, not done here

Shell completions. clap can generate them, and the command is already built
from the registry, so the pieces are present — but a completion script is
generated once and installed, while the command list changes whenever the project
is rebuilt, so the two need a story before it is worth shipping.
