# brocanth-command

`brocanth-command` gives a brocanth project its own command line, in the
style of Rails tasks. You write a `clap::Args` struct, put `#[cmd]` on it,
and implement `Command`. The struct is then a subcommand of the project's
binary, with parsed arguments and a `--help`. There is no list to edit:
the macro registers the command at link time through a
[`linkme`](https://docs.rs/linkme) distributed slice. Nothing is loaded or
scanned at run time. Use it for one-off and repeated jobs that need the
project's types: seed data, a migration, a report. Do not use it for the
global `brocanth` tool itself; that is `brocanth-cli`.

## Getting started

```toml
[dependencies]
brocanth-command = { git = "https://github.com/DHHwasRight/brocanth", branch = "main" }
```

The crate re-exports `clap`, `linkme` and `anyhow`. Your `Cargo.toml` does
not name them.

In a generated project, the `commands` module adds this crate, a
`src/commands/` directory with `serve`, and `main`:

```text
brocanth create my-app --with commands
brocanth add commands
```

The `commands` module has no settings. With it, each setting another
module declares is a global `clap` flag, `--<section>.<key>=VALUE`, which
goes before or after the subcommand name. See
[docs/module-flags.md](../../docs/module-flags.md).

## Examples

Define a command. The name is the struct name in `snake_case`:
`seed_demo`. The doc comment is the help text, because
`clap::Args::augment_args` copies it.

```rust
use brocanth_command::{anyhow, clap, cmd, Command, Context, Registered};

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

assert_eq!(<SeedDemo as Registered>::NAME, "seed_demo");
```

Give a name that no Rust identifier can spell:

```rust
use brocanth_command::{anyhow, clap, cmd, Command, Context, Registered};

/// Empty the cache.
#[cmd(name = "cache:clear")]
#[derive(clap::Args)]
pub struct CacheClear {
    #[arg(long)]
    pub force: bool,
}

impl Command for CacheClear {
    fn run(self, _ctx: &mut Context) -> anyhow::Result<()> {
        Ok(())
    }
}

assert_eq!(<CacheClear as Registered>::NAME, "cache:clear");
```

`main` is the whole command line. `main_with` adds arguments that every
command accepts. Mark each one `global(true)`. A generated project passes
its declared settings this way:

```rust,no_run
use brocanth_command::{anyhow, clap, main_with, Context};

fn flags(command: clap::Command) -> anyhow::Result<clap::Command> {
    Ok(command.arg(
        clap::Arg::new("sessions.cookie")
            .long("sessions.cookie")
            .global(true),
    ))
}

fn main() -> anyhow::Result<()> {
    main_with(Context::boot()?, flags)
}
```

`no_run`: it parses the process arguments and exits.

`Context::boot()` reads `$BROCANTH_ENV` (default `development`) and finds
the project root: the nearest directory with `brocanth.toml`, else the
current directory. `Context::env()` and `Context::root()` return them.

### Two traits

`Command::run` is yours. `Registered::NAME` belongs to the macro. They are
two traits because an `impl` block has one author. You never write
`impl Registered`. `entry` is bounded on it, so a struct without `#[cmd]`
is a type error, not a command that is silently absent.

### The linking gotcha

The linker drops a crate that nothing references, and the commands it
registers go with it. Your own crate is safe: `main` is in it. A
third-party crate that only adds commands is not. Reference it:

```rust,ignore
// In main.rs. `as _` imports no name. It tells the linker to keep the crate.
use some_task_pack as _;
```

`ignore`: `some_task_pack` is a placeholder crate name. Without that line
the crate compiles, the tests pass, and its commands are absent from
`--help`.

### Duplicate names

Two commands with one name stop the binary before `clap` runs. The error
names every collision, not only the first:

```text
two or more commands share a name: migrate, seed.
Every command name must be unique across this binary and every crate it links.
Rename one with `#[cmd(name = "...")]`.
```

`clap` keeps one of two duplicate subcommands and drops the other in
silence. A refusal is easier to debug than a missing command.

## How `brocanth` runs a project command

`brocanth` is installed once. It knows its own subcommands: `create`,
`add`, `migrate`, `dbshell` and `check`. It hands every other subcommand
to the project, the way `rails` hands off to `bin/rails`:

1. It walks up from the current directory to `brocanth.toml`. Without it
   there is no project.
2. It runs `cargo build`, and passes cargo's stderr through. The command
   you edited a second ago is the one that runs.
3. It asks `cargo metadata` where the binary is. `target/debug/<name>` is
   wrong under `CARGO_TARGET_DIR`, in a workspace, and for other profiles.
4. On Unix it `exec`s the binary, so the command owns the terminal, the
   exit code, and Ctrl-C. Elsewhere it spawns the binary and copies the
   exit code.

```text
$ brocanth seed_demo --count 3
/path/to/my-app (brocanth.toml)
seeding 3 rows into development
```

A project without the `commands` module has no command line. `brocanth`
then refuses and says `brocanth add commands`.

Inside a built project, `brocanth --help` lists the project's commands. It
reads them from the binary. There is no cargo alias: you can also run the
binary directly.

## Testing

```text
cargo test -p brocanth-command
```

No database and no environment variable is needed.

- Unit tests in `src/lib.rs` cover duplicate detection, the alphabetical
  order of `--help`, the derived name, the runner, and a global flag with
  dots after the subcommand.
- `tests/trybuild.rs` compiles `tests/ui/*.rs` and compares the
  compiler's output with the `.stderr` snapshots. It checks the macro's
  errors: `#[cmd]` on an enum, and an unknown argument.
- The README examples run as doctests of this crate. The examples of
  `brocanth-command-macros/README.md` run here too, because a proc-macro
  crate cannot use its own macro.

To test a command in an application, call `run` on a value you build:

```rust
# use brocanth_command::{anyhow, clap, cmd, Command, Context};
# #[cmd]
# #[derive(clap::Args)]
# pub struct SeedDemo {
#     #[arg(long, default_value_t = 100)]
#     pub count: u32,
# }
# impl Command for SeedDemo {
#     fn run(self, _ctx: &mut Context) -> anyhow::Result<()> { Ok(()) }
# }
let mut ctx = Context::boot()?;
SeedDemo { count: 3 }.run(&mut ctx)?;
# Ok::<(), anyhow::Error>(())
```

## Links

- [docs/module-flags.md](../../docs/module-flags.md): the global flags a
  generated project adds through `main_with`.
- [docs/modules.md](../../docs/modules.md): the `commands` module and the
  `CommandsMod` slot.
- [brocanth-command-macros](../brocanth-command-macros/README.md): what
  `#[cmd]` expands to.

## Not done

Shell completions. `clap` can write them, but a completion script is
installed once and the command list changes with each build.
