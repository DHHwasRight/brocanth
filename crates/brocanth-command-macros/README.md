# brocanth-command-macros

This crate holds the `#[cmd]` attribute macro. It registers a `clap::Args`
struct as a command of a brocanth project. Do not depend on it directly:
`brocanth-command` re-exports `cmd` with the traits, the registry and the
runner. Read the [brocanth-command README](../brocanth-command/README.md)
first.

## Getting started

Add `brocanth-command`, not this crate:

```toml
[dependencies]
brocanth-command = { git = "https://github.com/DHHwasRight/brocanth", branch = "main" }
```

The `commands` module of `brocanth create` adds it for you:
`brocanth create my-app --with commands`, or `brocanth add commands`. The
macro has no settings.

## What `#[cmd]` writes

For this input:

```rust
use brocanth_command::{anyhow, clap, cmd, Command, Context, Registered};

/// Fill the demo database.
#[cmd]
#[derive(clap::Args)]
struct SeedDemo {
    #[arg(long, default_value_t = 100)]
    count: u32,
}

impl Command for SeedDemo {
    fn run(self, _ctx: &mut Context) -> anyhow::Result<()> {
        Ok(())
    }
}

assert_eq!(<SeedDemo as Registered>::NAME, "seed_demo");
assert!(brocanth_command::COMMANDS.iter().any(|e| e.name == "seed_demo"));
```

the macro keeps the struct and adds two items:

1. `impl Registered for SeedDemo { const NAME: &'static str = "seed_demo"; }`.
   The name is the struct name in `snake_case`.
2. A `static` element of the distributed slice `brocanth_command::COMMANDS`,
   built by `brocanth_command::entry::<SeedDemo>()`. It carries
   `#[linkme(crate = ::brocanth_command::linkme)]`, so the project does not
   name `linkme`.

Every path it writes is absolute (`::brocanth_command::...`). A local item
called `Registered` or `entry` cannot capture it.

## Arguments

`#[cmd]` takes one optional argument, `name = "..."`, for a name that no
Rust identifier can spell:

```rust
use brocanth_command::{anyhow, clap, cmd, Command, Context, Registered};

/// Empty the cache.
#[cmd(name = "cache:clear")]
#[derive(clap::Args)]
struct CacheClear {
    #[arg(long)]
    force: bool,
}

impl Command for CacheClear {
    fn run(self, _ctx: &mut Context) -> anyhow::Result<()> {
        Ok(())
    }
}

assert_eq!(<CacheClear as Registered>::NAME, "cache:clear");
```

## Errors

Each error points at the code that is wrong and says what is allowed.

- On an enum, a function, an impl block, a module, a trait or a union:
  "`#[cmd]` works on a struct, not on an enum: a command's fields are its
  arguments, which is what `#[derive(clap::Args)]` reads".
- On any argument other than `name`: "unknown argument; `#[cmd]` accepts
  only `name = "..."`".
- On `name = ""`: "a command name cannot be empty".

```rust,compile_fail
use brocanth_command::cmd;

#[cmd]
enum NotACommand {
    A,
}
```

## Testing

A proc-macro crate cannot run its own macro in a doctest. The tests are in
`brocanth-command`:

```text
cargo test -p brocanth-command
```

- `tests/trybuild.rs` compiles `tests/ui/pass_command.rs` and the two
  failing cases, and compares the compiler's output with the `.stderr`
  snapshots.
- The examples in this README run as doctests of `brocanth-command`,
  through `MacrosReadme` in its `src/lib.rs` (`#[cfg(doctest)]`).

`cargo test -p brocanth-command-macros` builds the crate and runs no test.

## Links

- [brocanth-command](../brocanth-command/README.md): the traits, the
  registry, the runner, and the linking gotcha.
- [docs/modules.md](../../docs/modules.md): the `commands` module.
