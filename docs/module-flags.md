# Module settings and flags

A module declares each of its settings once. From that one declaration a
generated project gets:

- a flag on the binary: `--<module>.<key>=VALUE`;
- an environment variable: `$<APP>_<MODULE>__<KEY>`;
- with the `config` module, a key in `config/<env>.toml`, under `[<module>]`;
- a line in the settings report, which shows every layer on its own.

The flag, the variable and the key cannot disagree about the name or the
type, because nobody writes them by hand.

## The request, and how this reads it

Max asked for this:

> A module comes with flags added to the command line
> (`--name_of_module.section.subsection...=value`). If config module is
> present it registers its name at top level and the sections. If the
> config is present the flags switch to false. Find a way for the user to
> rename those flags if needed and plug them to the main executable.

And then decided the open questions:

1. A project without `config` reads the environment too.
2. A flag passed while the flags are off is refused with a `help:` line.
3. An `Option` setting gets a flag. Empty means `None`. A module may
   declare one more value that means `None`, such as `-1`.
4. A report shows each layer on its own, then the merge, with and without
   `config`.

| Request | Reading |
|---|---|
| flags per module | One flag per declared setting. The name is the setting's path: `--sessions.cookie`, `--db.pool.max_size`. |
| config registers the module and its sections | The declaration writes `[sessions]`, and `[db.pool]` for a dotted key. |
| with config, the flags switch to false | With `config`, the binary refuses a flag and says why. `WITH_CONFIG = true` in `src/flags.rs` turns the flags on. |
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
| `.secret()` | No flag. Every user on the host can read a process's arguments in `ps`, and the shell keeps them in its history. The variable and the file still set it, and the report masks it. `storage.secret` is one. |
| `.optional()` | The field is an `Option`. See [Optional settings](#optional-settings). |
| `.none_as("-1")` | Optional, and `-1` means `None` too. |

`configurable` writes, in one call:

- the registration in `register!`;
- the provider line, as `snippet::plugin` does;
- with `config`, the `[section]` block, made from the declarations;
- without `config`, a registration that starts from `Settings::default()`
  and applies the environment, then the flags.

`Scaffold::settings` records the declarations. The CLI reads them to write
the settings list and to record which config sections a module owns. It
does not parse `[name]` lines back out of the text for such a module.

## Precedence

Lowest to highest:

1. the plugin's `Settings::default()`;
2. the config files, in their own order (`./config/<env>.toml` lowest,
   `$<APP>_CONFIG` highest), with `config` only;
3. the environment, `$<APP>_<SECTION>__<KEY>`;
4. a flag.

**Reason:** the narrower decision wins. A file is shared by every
deployment. A variable applies to every process that one shell or one
container starts. A flag applies to one run.

With `config`, `brocanth-config` does the layering. A flag is a real layer
there: `Loader::flag(path, flag, raw, text)`, with `Provenance::Flag`.
Without `config`, the generated `given()` collects the variables, then the
flags, and `<section>_settings` applies them in that order to the default.
A later value for one key replaces an earlier one.

## Typing

- With `config`, a `text` setting keeps the raw value as a string:
  `--sessions.cookie=123` is the string `"123"`. Every other kind is typed
  the way the environment layer types a variable: `true`, `false`, an
  integer, a float, else a string.
- Without `config`, the generated code parses the raw value into the
  field's own type with `FromStr`. A bad value stops the binary:
  `--storage.token_ttl is "abc": invalid digit found in string`.

## Optional settings

A field typed `Option<T>` is declared with `.optional()` or
`.none_as(alias)`.

| Given | Means |
|---|---|
| nothing | Not decided at this layer. The layer below decides. |
| `--x=` or `X=` (empty) | `None`, decided here. It does not fall back to the default. |
| the declared alias, for example `--x=-1` | `None`, decided here. |
| anything else | `Some(value)`. |

The empty rule holds for every type, so an operator learns one rule. An
alias is per setting and never global: `-1` is invalid for an unsigned
field and a real value for some signed ones. `--help` and the file's
comment say what means `None`: "Empty or `-1` means none."

The file writes an optional key commented out, because TOML has no `None`.

In `brocanth-config` an empty flag or variable is `ValueKind::Empty`. It
survives a merge, unlike `Unset` (JSON and YAML `null`), which removes the
key. `Empty` reads as `None` into an `Option` and as `""` into a string.
Into a number or a boolean it is a type error. `Loader::none_as(path,
alias)` turns the alias into `Empty` after the merge, whatever layer it
came from, so `limit = -1` in a file works too.

## Flags off, with `config`

With `config` and `WITH_CONFIG = false`, a declared flag is refused by
name before anything starts:

```text
error: `--sessions.cookie` is off: this project reads its settings from config/<env>.toml
help: set WITH_CONFIG = true in src/flags.rs, or set `cookie` under [sessions] in config/<env>.toml
```

A flag nobody declared is still `clap`'s error.

## Renaming and hiding

`src/flags.rs` belongs to the project. The generator writes it when it is
missing and never again. That is why `brocanth add` keeps the renames, and
why a project that predates this file gets one the first time it adds a
module with a setting.

```rust
pub const WITH_CONFIG: bool = false;

pub const RENAMES: &[(&str, &str)] = &[
    ("sessions.cookie", "cookie"), // --cookie instead of --sessions.cookie
    ("storage.root", ""),          // no flag at all
];
```

A rename changes the flag only. The variable and the file key keep the
canonical name, because they are shared with every other deployment.

The list of settings is in `src/plugins/mod.rs`, which `brocanth add`
rewrites. It holds the canonical names only.

**Why a Rust file, and not `brocanth.toml`:** `brocanth.toml` is generated
and records what the generator did. A rename there would take effect only
after a regeneration. A constant in `src/flags.rs` takes effect on the next
`cargo build`, and the compiler checks its type.

### What a rename may not do

The binary checks these at startup, before it binds a socket. Each failure
prints an error with a `help:` line.

- Rename a key that no flag sets. Such a rename would do nothing.
- Use a character other than a lower-case letter, a digit, `.`, `_` or
  `-`, or start with `-`, `.` or `_`.
- Take `help`, `version` or `settings`.
- Give two flags one name.
- With `commands`, take a name that a command already uses, such as
  `serve --addr`. `clap` panics on that in a debug build and picks one
  silently in a release build, so `flag_args` refuses it first.

A canonical name always contains a dot, and no built-in argument does. Only
a rename can collide.

### Collisions between modules

Two modules that declare the same `section.key` are a generator problem.
`Scaffold::setting` names both modules, and `brocanth create` writes
nothing.

## The settings report

`my-app settings` with `commands`, `my-app --settings` without. Each layer
is printed alone, lowest first, with only the keys it sets. An empty layer
says `(sets nothing)`. The merged view follows and names the layer of each
value.

```text
layers, lowest precedence first

   1  defaults
        sessions.cookie  = "_my_app_session"
        sessions.secure  = true
   2  ./config/development.toml
        sessions.cookie  = "_my_app_session"  line 9
        sessions.secure  = false              line 12
   3  environment
        (sets nothing)
   4  flags
        sessions.cookie  = "x"                --cookie

merged
  sessions.cookie        = "x"                4 flags
  sessions.secure        = false              2 ./config/development.toml
```

- The defaults layer is the plugin's `Default`, formatted with `Debug`. An
  optional default prints `Some(..)` or `None`.
- With `config`, the report is `Config::layers()` in `brocanth-config`,
  so a project that calls the crate directly has it too. The defaults are
  passed in, because only the application can build its settings types.
- Without `config` there are three layers: defaults, environment, flags.
- A secret prints `(secret)` in every layer. So does any key the standard
  redaction masks by name.

## How the binary reads the flags

| Project | Reader |
|---|---|
| `commands` | `main` calls `brocanth_command::main_with(ctx, plugins::flag_args)`. Each flag is a global `clap` argument, so it may come before or after the subcommand, and `--help` lists it. |
| no `commands` | `main` calls `plugins::read_line()`. It accepts only the declared flags, answers `--help` and `--settings`, and refuses every other argument. |

In both cases `plugins::given()` reads `--name=VALUE` and `--name VALUE`
from the process arguments and stops at `--`.

## Files a generated project gets

| File | Owner | Holds |
|---|---|---|
| `src/plugins/mod.rs` | generator | `SETTINGS`, `given`, `flag_args` or `read_line`, `settings_report`; with config `config()` and `refuse_off_flags`; without config one `<section>_settings` per section |
| `src/flags.rs` | project | `WITH_CONFIG`, `RENAMES` |
| `src/commands/settings.rs` | project | the `settings` command, with `commands` |
| `src/main.rs` | generator | `mod flags;` and the reader call |
| `config/<env>.toml` | shared | the declared sections, with config |

A project with no declared setting gets none of this.

## Tests

- `brocanth-config`: `a_flag_outranks_the_environment_and_names_itself`,
  `a_flag_without_a_section_is_refused_by_name`,
  `an_empty_flag_or_variable_is_none_for_an_option_and_empty_for_a_string`,
  `each_layer_alone_then_the_merge`.
- `brocanth-command`: `a_global_flag_with_dots_is_accepted_after_the_subcommand`.
- `brocanth-scaffold`: `a_dotted_key_is_a_subsection_after_the_plain_keys`,
  `one_key_declared_twice_names_both_modules`.
- `brocanth-cli` goldens: `flags` (no config, no command line),
  `flags-commands` (no config, `clap`), `everything` (config).
- `brocanth-cli`, ignored because it runs cargo:
  `declared_flags_reach_the_plugin_with_and_without_a_config_file`. It
  builds three generated binaries and checks a flag, a variable, a secret,
  a rename, the refusal while the flags are off, and the report.

## Open questions

1. **Hooking modules.** `brocanth-config` has no config-file form for a
   plugin registered with `plugin_hooking`. `csrf`, `security-headers` and
   `rate-limit` therefore build a literal config even with `config` chosen,
   and a `[security_headers]` section is never read, although a generated
   comment says it is. A declaration cannot fix that alone: it needs
   `plugin_hooking_from_config` in `brocanth-config`, and `Deserialize` on
   the middleware config types.
2. **The `[server]` section.** `shutdown_grace_secs` belongs to the
   framework, not to a module, so it has no flag yet.
