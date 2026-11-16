# Module settings and flags

A module declares each of its settings once. From that one declaration a
generated project gets:

- a flag on the binary: `--<module>.<key>=VALUE` (a secret gets two
  pointer flags instead, see [Secrets](#secrets));
- an environment variable: `$<APP>_<MODULE>__<KEY>`;
- a value in code: `set` in the module file `src/modules/<key>.rs`;
- with a `ConfigSource` (the `config` module), a key in
  `config/<env>.toml`, under `[<module>]`;
- a line in the settings report, which shows every layer on its own.

The flag, the variable, the code value and the key cannot disagree about
the name or the type, because nobody writes them by hand.

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

**Changed on 2026-11-16** (`docs/module-capabilities.md` §6, stage 3): the
flags no longer switch off with `config`. A config module keeps the flags
and adds a file layer. A secret gets pointer flags. Code is a layer.

| Request | Reading |
|---|---|
| flags per module | One flag per declared setting. The name is the setting's path: `--sessions.cookie`, `--db.pool.max_size`. |
| config registers the module and its sections | The declaration writes `[sessions]`, and `[db.pool]` for a dotted key. |
| with config, the flags switch to false | Replaced: the flags stay on beside a file. `flags: Flags::Off` in a module's file turns that module's flags off, and the binary refuses such a flag by name. |
| rename | `rename` in `src/modules/<key>.rs`. The project owns that file. |
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
| kind | `text`, `bool`, `integer`, `float` or `list`. The kind decides how the file quotes the value and whether a flag value stays a string. |
| value | The value the file starts with. `Settings::default()` stays the value without a file. The two can differ on purpose: `sessions.secure` is `false` in the development file and `true` by default. |
| `help` | One line. `--help` prints it, and it is the first comment line in the file. |
| `.note(...)` | More comment lines for the file only. |
| `.secret()` | No value flag: two pointer flags, `_file` and `_env`. See [Secrets](#secrets). The variable and the file still set it, and the report masks it. `storage.secret` is one. |
| `.optional()` | The field is an `Option`. See [Optional settings](#optional-settings). |
| `.none_as("-1")` | Optional, and `-1` means `None` too. |
| `.commented()` | The file writes the key commented out, as an example. For a string whose empty default turns a feature off, such as `observability.metrics_bind`. |

A `list` of strings has a flag too: the items joined by commas,
`--csrf.exempt=/hooks,/api`. Empty is the empty list. The module file and,
without a file, the variable take the same spelling. With a file, the
variable sets a list by index (`$<APP>_CSRF__EXEMPT__0`), as
`brocanth-config` reads it.

A list of records, such as rate-limit rules or robots groups, has no
declaration and no flag (owner decision 1 in
`docs/module-capabilities.md` §13). It lives in code beside the
registration, or in the file.

`configurable` writes, in one call:

- the one registration: the plugin, built from
  `crate::plugins::<key>_settings(&layers)`;
- the provider line, as `snippet::plugin` does;
- with a `ConfigSource`, the `[section]` block, made from the declarations.

The registration is the same text in every project. The CLI writes the
reader behind it in `src/plugins/mod.rs`, so a module never asks whether
the project has `config`:

```rust
let builder = builder.plugin(
    <crate::plugins::sessions::Sessions as brocanth::plugin_api::Plugin>::new(
        crate::plugins::sessions_settings(&layers)?,
    ),
);
```

The trait is named in the call, so no file needs it in scope.

`Scaffold::settings` records the declarations. The CLI reads them to write
the settings list and to record which config sections a module owns. It
does not parse `[name]` lines back out of the text for such a module.

## Which modules declare settings

Every module with a section: `admin`, `auth`, `cache`, `jobs`, `logging`,
`model`, `observability` (each child adds its keys), `openapi`, `policy`,
`realtime`, `sessions`, `storage`, `validation`, and the others listed in
`docs/modules/`.

Every URL setting (`db.url`) is a secret: a database URL can carry a
password. So are `storage.secret`, `mail.password`,
`observability.metrics_token` and the OAuth client secrets.

## Precedence

Lowest to highest:

1. the plugin's `Settings::default()`;
2. code: `set` in the module file `src/modules/<key>.rs`;
3. the config files, in their own order (`./config/<env>.toml` lowest,
   `$<APP>_CONFIG` highest), with a `ConfigSource` only;
4. the environment, `$<APP>_<SECTION>__<KEY>`;
5. a flag.

**Reason:** the narrower decision wins. Code is shared by every
deployment of the binary. A file is shared by every deployment that reads
it. A variable applies to every process that one shell or one container
starts. A flag applies to one run. The environment stays, because
containers and platforms set variables, not flags.

`src/plugins/mod.rs` has the same three items in every project:

| Item | Without a file | With a file |
|---|---|---|
| `Layers` | `Vec<Given>`: the code, the variables, the flags | `brocanth_config::Config` |
| `layers()` | `given()` | a `brocanth_config::Loader` with `code`, `flag` and `flag_list` |
| `<section>_settings(&layers)` | the default, then each `Given` in order, parsed with `FromStr` | `layers.section("<section>")`, typed by serde |

`brocanth-config` has the code layer as a real layer:
`Loader::code(path, file, raw, text)` and `Loader::code_list`, with
`Provenance::Code`, under the files. A flag is `Loader::flag(path, flag,
raw, text)` or `Loader::flag_list`, with `Provenance::Flag`. A later value
for one key replaces an earlier one.

Commands that read a setting outside the plugin read the same layers:
`migrate`, `dbshell`, `rls` and `createsuperuser` call
`crate::plugins::db_settings(&crate::plugins::layers()?)`, so a flag such
as `--db.url_file` reaches them too. `dbshell` no longer takes `--env`:
set `$<APP>_ENV`.

## Typing

- With a file, a `text` setting keeps the raw value as a string:
  `--sessions.cookie=123` is the string `"123"`. Every other kind is typed
  the way the environment layer types a variable: `true`, `false`, an
  integer, a float, else a string. A list is a list of strings.
- Without a file, the generated code parses the raw value into the
  field's own type with `FromStr`, and each item of a list into the item's
  type. A bad value stops the binary:
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

## Secrets

A flag is visible in `ps` to every user on the host, and the shell keeps it
in its history. So a secret has no flag that carries its value. It has two
pointer flags:

- `--<section>.<key>_file=PATH` reads the value from the file at `PATH`,
  without its trailing newline. Docker and Kubernetes mount secrets so.
- `--<section>.<key>_env=VAR` reads the value from the variable `VAR`, in
  place of the default `$<APP>_<SECTION>__<KEY>`.

Both are flags, so they are the top layer. An unreadable file or an unset
variable stops the binary with a `help:` line. The settings report shows
the pointer, never the value:

```text
merged
  storage.secret     = (secret)  4 flags
...
   4  flags
        storage.secret  = (secret)  --storage.secret_file=/run/secrets/storage
```

A pointer flag follows its module's `flags` choice, and no file renames
it: `Key` has no variant for a secret. The generated `Debug` of a settings
struct still prints `<redacted>` for a secret (`snippet::redact_secrets`).

## Turning flags off

A module's flags are on in every project, with a file or without one.
`flags: Flags::Off` in a module's file turns that module's flags off. The
variable, the code value and the file still set each value. A flag that is
off is refused by name before anything starts:

```text
error: `--storage.token_ttl` is off: src/modules/storage.rs turns the flags of its module off
help: set `flags: Flags::On` in src/modules/storage.rs, or set `storage.token_ttl` in the environment
```

`Flags::Project` (the default) and `Flags::On` both read the flags.
`flags_with_config` in `src/modules/brocanth.rs` is gone; an old
`src/flags.rs` with `WITH_CONFIG = true` moves without it.

A flag nobody declared is still `clap`'s error.

## Renaming, hiding and setting in code

Each module file in `src/modules/` belongs to the project. The generator
writes it when it is missing and never again, so `brocanth add` keeps the
renames (`docs/module-config.md`). `src/modules/brocanth.rs` holds the
`[server]` renames and values.

```rust
// src/modules/sessions.rs
pub const MODULE: Module = Module {
    flags: Flags::Project,
    rename: &[(Key::Cookie, Flag::As("cookie"))], // --cookie
    set: &[(Key::Secure, "true")],                // a value in code
    ..Module::DEFAULT
};

// src/modules/storage.rs
    rename: &[(Key::Root, Flag::Hidden)], // no flag at all
```

`Key` is an enum with one variant per setting that has a value flag,
generated in `src/plugins/shape.rs`. A typo and a secret are compile
errors. A list has a variant.

A value in `set` is spelled as a flag spells it: the raw text, a list
joined by commas. It is the layer over the default and under every other.

A rename changes the flag only. The variable and the file key keep the
canonical name, because they are shared with every other deployment.

The list of settings is in `src/plugins/mod.rs`, which `brocanth add`
rewrites. It holds the canonical names only.

**Why a Rust file, and not `brocanth.toml`:** `brocanth.toml` is generated
and records what the generator did. A rename there would take effect only
after a regeneration. A constant in `src/modules/` takes effect on the next
`cargo build`, and the compiler checks its type.

### What a rename may not do

The binary checks these at startup, before it binds a socket. Each failure
prints an error with a `help:` line.

- Use a character other than a lower-case letter, a digit, `.`, `_` or
  `-`, or start with `-`, `.` or `_`.
- Take `help`, `version` or `settings`.
- Give two flags one name. A pointer flag's name counts.
- With `commands`, take a name that a command already uses, such as
  `serve --addr`. `clap` panics on that in a debug build and picks one
  silently in a release build, so `flag_args` refuses it first.

A canonical name always contains a dot, and no built-in argument does. Only
a rename can collide. A rename of a key that no flag sets does not
compile: `Key` has no variant for it. The generated test
`flag_names_are_valid` runs the startup check in `cargo test`.

### Collisions between modules

Two modules that declare the same `section.key` are a generator problem.
`Scaffold::setting` names both modules, and `brocanth create` writes
nothing.

## Build-time options

An `Opt` in a manifest (`mount_at`, `path`, a lockout limit) shapes the
build: a route table, the i18n exceptions and the robots rules are
generated from it and checked by the compiler. It is a constant in the
module file, and no flag, variable or file of the binary changes it
(`docs/module-capabilities.md` §6, 3d).

`brocanth create` and `brocanth add` set it when they write the module
file:

```sh
brocanth add admin --admin.mount_at=/backoffice
```

The value is checked against the option's type. An option for a module
the command does not write, or whose file exists, is refused with a
`help:` line: edit the file by hand.

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
   2  code
        sessions.secure  = true                 src/modules/sessions.rs
   3  ./config/development.toml
        sessions.cookie  = "_my_app_session"  line 9
        sessions.secure  = false              line 12
   4  environment
        (sets nothing)
   5  flags
        sessions.cookie  = "x"                --cookie

merged
  sessions.cookie        = "x"                5 flags
  sessions.secure        = false              3 ./config/development.toml
```

- The defaults layer is the plugin's `Default`, formatted with `Debug`. An
  optional default prints `Some(..)` or `None`.
- With a file, the report is `Config::layers()` in `brocanth-config`,
  so a project that calls the crate directly has it too. The defaults are
  passed in, because only the application can build its settings types.
- Without a file there are four layers: defaults, code, environment,
  flags.
- A secret prints `(secret)` in every layer. So does any key the standard
  redaction masks by name. A pointer flag shows its path or variable name.

## How the binary reads the flags

| Project | Reader |
|---|---|
| `commands` | `main` calls `brocanth_command::main_with(ctx, plugins::flag_args)`. Each flag is a global `clap` argument, so it may come before or after the subcommand, and `--help` lists it. |
| no `commands` | `main` calls `plugins::read_line()`. It accepts only the declared flags, answers `--help` and `--settings`, and refuses every other argument. |

In both cases `plugins::given()` reads `--name=VALUE` and `--name VALUE`
from the process arguments and stops at `--`. It reads the file or the
variable a pointer flag names.

## Files a generated project gets

| File | Owner | Holds |
|---|---|---|
| `src/plugins/mod.rs` | generator | `SETTINGS`, `Name`, `names`, `code`, `given`, `layers`, `flag_args` or `read_line`, `refuse_off_flags`, one `<section>_settings` per section, `settings_report`; `ServerSettings` and `shutdown_grace` |
| `src/modules/<key>.rs` | project | `flags`, `rename`, `set`, and the module's other choices |
| `src/modules/brocanth.rs` | project | the `[server]` renames and values |
| `src/plugins/shape.rs` | generator | `Flags`, `Flag`, and each module's `Key` |
| `src/commands/settings.rs` | project | the `settings` command, with `commands` |
| `src/main.rs` | generator | `mod modules;` and the reader call |
| `config/<env>.toml` | shared | the declared sections, with a `ConfigSource` |

Every project gets these files, because the framework declares
`[server] shutdown_grace_secs` (see below).

## Tests

- `brocanth-config`: `a_flag_outranks_the_environment_and_names_itself`,
  `code_is_under_the_file_and_a_list_flag_splits_at_commas`,
  `a_flag_without_a_section_is_refused_by_name`,
  `an_empty_flag_or_variable_is_none_for_an_option_and_empty_for_a_string`,
  `each_layer_alone_then_the_merge`.
- `brocanth-command`: `a_global_flag_with_dots_is_accepted_after_the_subcommand`.
- `brocanth-scaffold`: `a_dotted_key_is_a_subsection_after_the_plain_keys`,
  `one_key_declared_twice_names_both_modules`,
  `every_plugin_registration_shape_is_what_rustfmt_writes`.
- `brocanth-cli`: `one_registration_form_with_and_without_a_config_file`,
  `a_build_time_option_is_written_into_the_module_file_once`; goldens
  `flags` (no config, no command line), `flags-commands` (no config,
  `clap`), `everything` (config).
- `brocanth-cli`, ignored because it runs cargo:
  `declared_flags_reach_the_plugin_with_and_without_a_config_file`. It
  builds three generated binaries and checks each layer over the one under
  it (code, file, variable, flag), a rename, a module with its flags off, a
  secret through `_file` and `_env` with the pointer in the report, and an
  unset pointer variable.

## Hooking plugins from a library crate

`csrf`, `security-headers` and `rate-limit` register plugins that install
hooks, and their configuration types live in `brocanth-middleware`, not
in the generated project. `snippet::configurable_hooking` declares their
settings the same way `snippet::configurable` does, with three
differences:

- **The section is the plugin's instance name, not the module key.**
  `[csrf]`, `[security_headers]`, `[rate_limit]`, so the flags are
  `--csrf.secure`, `--security_headers.behind_tls`. `brocanth-config`
  checks a section against the instance names, and these are the names
  the plugins already had.
- **The settings type is the library's.** `Scaffold::settings_type`
  records `brocanth_middleware::CsrfConfig` for `csrf`, and the generated
  readers start from its `Default` and set its public fields.
- **The registration goes through `plugin_hooking`**, with the plugin
  built from `crate::plugins::<section>_settings(&layers)`. An `adjust`
  chain adds a value only code can give, after the layers: the admin's
  inline script hash is one.

`brocanth-middleware`'s `config` feature derives `Deserialize` on the
configuration types. The module turns it on only when the project has
`config`, through `Dep::feature_with("config", "config")`: without a file
nothing deserialises, so nothing links a parser for it.

**Lists of records stay in code or the file.** CSRF's `exempt` is a list of
strings, so it has a flag: `--csrf.exempt=/callbacks`. A rate-limit rule is
a record of six fields (name, paths, methods, limit, window, key), and a
list of records is `[[rate_limit.rules]]` in TOML. That shape already
deserialises into `Vec<Rule>`, needs no new syntax, and reads the same as
the Rust builder. Without a file, the example rule is code beside the
registration: `.rule(login)` on the settings the layers built. The
rate-limit module declares `uses::<ConfigSource>` for that choice.

The ignored test
`hook_settings_reach_a_request_from_the_file_and_from_the_flags` builds
two generated servers and sends real requests: one edits
`config/development.toml`, the other passes flags and a variable. Both
change the CSRF cookie name, the policy header and the referrer policy
that a response carries. The file's one-GET rule answers the second GET
with 429.

## The framework's own settings: `[server]`

The framework declares `[server] shutdown_grace_secs` the way a module
declares its settings. So it has the same ways in:

- the flag `--server.shutdown_grace_secs=SECS`;
- the variable `$<APP>_SERVER__SHUTDOWN_GRACE_SECS`;
- `set` in `src/modules/brocanth.rs`;
- with a `ConfigSource`, the key in `config/<env>.toml`, as before.

The settings report shows it beside the modules' settings, and
`src/modules/brocanth.rs` can rename it.

**Decision: every project declares it.** Every project serves, and every
`serve` drains for the grace period. So every project gets the flag
machinery, `src/modules/brocanth.rs` and, with `commands`, the `settings`
command, also `--core-only`.

**Reason:** one rule for every setting. A setting that has a flag in some
projects and not in others is a rule an operator must look up.

How it is built:

- The CLI records the declaration after the modules, under the owner
  `brocanth`, which is not a module key. A module that declares
  `server.shutdown_grace_secs` too is named in the error.
- The type is `crate::plugins::ServerSettings`, generated in
  `src/plugins/mod.rs`. Its `Default` is `crate::app::SHUTDOWN_GRACE`
  (25 s), so the constant in `src/app.rs` stays the one default.
- `plugins::shutdown_grace` reads the raw `[server]` section of
  `plugins::layers()` with a file, and `server_settings(&layers()?)`
  without one.
- `config/<env>.toml` still writes `[server]` itself, beside `[app]`,
  because `brocanth-config` owns both sections (`CORE_SECTIONS`).

## Open questions

None now.
