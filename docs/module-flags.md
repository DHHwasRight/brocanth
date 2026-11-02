# Module flags

A module declares each of its settings once. From that one declaration a
generated project gets two things:

- a flag on the binary: `--<module>.<key>=VALUE`;
- with the `config` module, a key in `config/<env>.toml`, under `[<module>]`.

The flag and the key cannot disagree about the name or the type, because
nobody writes either by hand.

## The request, and how this reads it

Max asked for this:

> A module comes with flags added to the command line
> (`--name_of_module.section.subsection...=value`). If config module is
> present it registers its name at top level and the sections. If the
> config is present the flags switch to false. Find a way for the user to
> rename those flags if needed and plug them to the main executable.

This design reads it as follows. Each point is a choice, and each can be
changed.

| Request | Reading |
|---|---|
| flags per module | One flag per declared setting. The name is the setting's path: `--sessions.cookie`, `--db.pool.max_size`. |
| config registers the module and its sections | The declaration writes `[sessions]`, and `[db.pool]` for a dotted key. |
| with config, the flags switch to false | With `config`, the binary takes no flag. The file and the environment are the sources. `WITH_CONFIG = true` in `src/flags.rs` turns the flags back on. |
| rename | `RENAMES` in `src/flags.rs`. The project owns that file. |
| plug into the main executable | With `commands`, the flags are global `clap` arguments. Without `commands`, `main` reads them. |

## Declaring a setting

A module calls `snippet::configurable` in place of `snippet::plugin`:

```rust
configurable(
    s,
    project,
    "sessions",
    "Sessions",
    vec![
        Setting::text("cookie", "_my_app_session", "The session cookie's name."),
        Setting::bool("secure", false, "Send the cookie over HTTPS only.")
            .note("Development only. Anywhere a browser reaches over HTTPS, this is true."),
    ],
);
```

| Part | Meaning |
|---|---|
| `key` | The path inside the section. A dot makes a subsection: `pool.max_size`. The same path is the field path in the plugin's `Settings` struct. |
| kind | `text`, `bool` or `integer`. The kind decides how the file quotes the value and whether a flag value stays a string. |
| value | The value the file starts with. `Settings::default()` stays the value without a file. The two can differ on purpose: `sessions.secure` is `false` in the development file and `true` by default. |
| `help` | One line. `--help` prints it, and it is the first comment line in the file. |
| `.note(...)` | More comment lines for the file only. |
| `.secret()` | No flag. Every user on the host can read a process's arguments in `ps`, and the shell keeps them in its history. `storage.secret` is one. |

`configurable` writes, in one call:

- the registration in `register!`;
- the provider line, as `snippet::plugin` does;
- with `config`, the `[section]` block, made from the declarations;
- without `config`, a registration that starts from `Settings::default()`
  and applies the flags.

`Scaffold::settings` records the declarations. The CLI reads them to write
the flag list, and to record which config sections a module owns. It no
longer parses `[name]` lines back out of the text for such a module.

Converted so far: `sessions` and `storage`. Every other module still writes
its section by hand and has no flag.

## Precedence

Lowest to highest:

1. the plugin's `Settings::default()`;
2. the config files, in their own order (`./config/<env>.toml` lowest,
   `$<APP>_CONFIG` highest);
3. the environment, `$<APP>_<SECTION>__<KEY>`;
4. a flag.

**Reason:** the narrower decision wins. A file is shared by every
deployment. A variable applies to every process one shell or one container
starts. A flag applies to one run. This is the order the environment layer
already used against the files, extended by one step.

Without `config` there are no files and no environment layer. The order is
then the default, then the flag.

A flag is a real layer in `brocanth-config`: `Loader::flag(path, flag, raw,
text)`. Its values carry `Provenance::Flag`. The dump names the flag as
typed, for example `--cookie` for a renamed flag, because that is what an
operator searches the shell history for.

## Typing

- With `config`, a `text` setting keeps the raw value as a string:
  `--sessions.cookie=123` is the string `"123"`. Every other kind is typed
  the way the environment layer types a variable: `true`, `false`, an
  integer, a float, else a string.
- Without `config`, the generated code parses the raw value into the
  field's own type with `FromStr`. A bad value stops the binary:
  `--storage.token_ttl is "abc": invalid digit found in string`.

## Renaming and hiding

`src/flags.rs` belongs to the project. The generator writes it when it is
missing and never again. That is why `brocanth add` keeps the renames, and
why a project that predates this file gets one the first time it adds a
module with a flag.

```rust
pub const WITH_CONFIG: bool = false;

pub const RENAMES: &[(&str, &str)] = &[
    ("sessions.cookie", "cookie"), // --cookie instead of --sessions.cookie
    ("storage.root", ""),          // no flag at all
];
```

The list of flags is in `src/plugins/mod.rs`, which `brocanth add`
rewrites. It holds the canonical names only.

**Why a Rust file, and not `brocanth.toml`:** `brocanth.toml` is generated
and records what the generator did. A rename there would take effect only
after a regeneration. A constant in `src/flags.rs` takes effect on the next
`cargo build`, and the compiler checks its type.

### What a rename may not do

The binary checks these at startup, before it binds a socket. Each failure
prints an error with a `help:` line.

- Rename a key that no module declares. Such a rename would do nothing.
- Use a character other than a lower-case letter, a digit, `.`, `_` or
  `-`, or start with `-`, `.` or `_`.
- Take `help` or `version`.
- Give two flags one name.
- With `commands`, take a name that a command already uses, such as
  `serve --addr`. `clap` panics on that in a debug build and picks one
  silently in a release build, so `flag_args` refuses it first.

A canonical name always contains a dot, and no built-in argument does. Only
a rename can collide.

### Collisions between modules

Two modules that declare the same `section.key` are a generator problem.
`Scaffold::setting` names both modules, and `brocanth create` writes
nothing. A section is the module's key, so this happens only when a module
writes into another module's section.

## How the binary reads the flags

| Project | Reader |
|---|---|
| `commands` | `main` calls `brocanth_command::main_with(ctx, plugins::flag_args)`. Each flag is a global `clap` argument, so it may come before or after the subcommand, and `--help` lists it. `clap` refuses a flag that is not declared or is turned off. |
| no `commands` | `main` calls `plugins::read_line()`. It accepts only the declared flags, answers `--help` with the list, and refuses every other argument. |

In both cases `register!` calls `plugins::given_flags()`. It reads
`--name=VALUE` and `--name VALUE` from the process arguments and stops at
`--`.

## Files a generated project gets

| File | Owner | Holds |
|---|---|---|
| `src/plugins/mod.rs` | generator | `FLAGS`, `flag_names`, `given_flags`, `flag_args` or `read_line`, and without config one `flagged_<section>` per section |
| `src/flags.rs` | project | `WITH_CONFIG`, `RENAMES` |
| `src/main.rs` | generator | `mod flags;` and the reader call |
| `config/<env>.toml` | shared | the declared sections, with config |

A project with no declared flag gets none of this.

## Tests

- `brocanth-config`: `a_flag_outranks_the_environment_and_names_itself`,
  `a_flag_without_a_section_is_refused_by_name`.
- `brocanth-command`: `a_global_flag_with_dots_is_accepted_after_the_subcommand`.
- `brocanth-cli` goldens: `flags` (no config, no command line),
  `flags-commands` (no config, `clap`), `everything` (config).
- `brocanth-cli`, ignored because it runs cargo:
  `declared_flags_reach_the_plugin_with_and_without_a_config_file`. It
  builds two generated binaries and checks that a flag reaches the plugin,
  that a renamed flag works and its old name is refused, and that a secret
  has no flag.

## Open questions

1. **Hooking modules.** `brocanth-config` has no config-file form for a
   plugin registered with `plugin_hooking`. `csrf`, `security-headers` and
   `rate-limit` therefore build a literal config even with `config` chosen,
   and a `[security_headers]` section is never read, although a generated
   comment says it is. A declaration cannot fix that alone: it needs
   `plugin_hooking_from_config` in `brocanth-config`, and `Deserialize` on
   the middleware config types.
2. **Env without config.** Should a project without `config` read
   `$<APP>_<SECTION>__<KEY>` too? Today it does not, because the
   environment layer lives in `brocanth-config`.
3. **A flag that is off.** With `config` and `WITH_CONFIG = false`, `clap`
   answers `unexpected argument '--sessions.cookie'`. A hint that names
   `src/flags.rs` would be kinder.
4. **The dump.** `Config::dump` lists files and the environment as sources.
   It does not list the command line as a source yet, although each value
   names its flag.
5. **Remaining modules.** `cache`, `jobs`, `auth`, `policy`, `openapi`,
   `schema`, `realtime`, `model`, `logging` and `observability` still write
   their sections by hand. Each conversion is mechanical, but one detail
   needs care: a field typed `Option<T>` does not parse with `FromStr`, so
   it needs a kind of its own or stays without a flag.
