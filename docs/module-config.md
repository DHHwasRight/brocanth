# Module configuration in the source: `src/modules/`

Status: **proposal**, partly built. The core side of hook toggles (§2.3,
§3.2) is built: `AppBuilder::hooks_off`, `hooks_off_from` and
`Pipeline::skipped`.

## The request

Max asked for this:

> I'd like a central place to be able to check and configure modules not
> the config.toml but the global config of every module. When a module is
> added it goes to src/modules/<name_of_module>.rs. There people can define
> if it adds the CLI options for configuration (or rebind them to different
> name), disable some features (like the integration into the admin
> interface, which is enabled by default) and some other static configs you
> might need to. This allows a central place and maybe in the mod.rs we can
> easily read what are the integrated modules.

And then added:

> The config of the module may activate or deactivate some hooks of the
> module as well.

| Request | Reading |
|---|---|
| a central place, not `config.toml` | `src/modules/`. One file per module, in Rust. The compiler checks it. |
| `src/modules/<name>.rs` when a module is added | `create` and `add` write it when it is missing, and never again. The project owns it. |
| the CLI options, or rename them | `flags` (on, off, or the project rule) and `rename` per module. They replace `WITH_CONFIG` and `RENAMES` in `src/flags.rs`. |
| disable a feature, such as the admin tab | `integrate`: one toggle per integration the module offers. On by default unless the module says otherwise. |
| some other static configs | `options`: compile-time values the module declares, such as the admin's mount point. |
| hooks on and off | `hooks`: one toggle per hook the module registers. A hook the module cannot work without has no toggle. |
| `mod.rs` shows the integrated modules | `src/modules/mod.rs`: one `pub mod` line per module with choices, and `ALL`, every module in `brocanth.toml`. |

## 1. The generated layout

```text
src/
  modules/
    mod.rs            generator   the list: `pub mod` lines and `ALL`
    brocanth.rs       project     the framework itself: [server], the flag rule
    admin.rs          project     one file per module that has a choice
    analytics.rs      project
    sessions.rs       project
    ...
  plugins/
    mod.rs            generator   reads crate::modules::*::MODULE
    shape.rs          generator   the types the files above fill
    <key>/mod.rs      project     the plugin's runtime code, as today
```

**Decision: `mod.rs` is a list, not a registry function.** It holds
`pub mod` lines and one `const`. A reader sees the modules in five
seconds. A function would hide the list behind calls. The registration
chain already exists in `src/plugins/mod.rs`, so a second registry adds
nothing.

**Decision: a file only for a module that has a choice.** A module has a
choice when it declares a setting with a flag, a hook toggle, an
integration toggle or an option. `observability-logs` has none, so it has
no file. `ALL` still names it. **Reason:** the `everything` golden has 39
modules. 39 files, most of them empty, hide the 15 that matter.

**Decision: the types live in a generated file, `src/plugins/shape.rs`.**
The CLI writes one Rust module per module file, from the manifest and the
declared settings. **Reason:** one source. A type in the module's crate
would repeat every `Setting` key by hand. It also cannot exist for a module
that the CLI hosts (`sessions`, `logging`, `model`) or for a module from
another repository without a runtime crate.

### `src/modules/mod.rs`

```rust
//! Written by `brocanth`. Do not edit -- `brocanth add` and
//! `brocanth remove` rewrite this file.
//!
//! The modules this project integrates. A `pub mod` line is a module with
//! choices, in the file next to this one. Those files are yours.

/// The framework itself: `[server]`, and whether flags work beside a
/// config file.
pub mod brocanth;

pub mod admin;
pub mod analytics;
pub mod middleware_rate_limit;
pub mod sessions;

/// Every module in `brocanth.toml`, in registration order. A module with no
/// `pub mod` line above has nothing to choose.
pub const ALL: &[&str] = &[
    "observability",
    "observability-health",
    "config",
    "sessions",
    "db",
    "analytics",
    "middleware",
    "middleware-rate-limit",
    "admin",
    "commands",
];
```

`ALL` is what `my-app settings` and a future `my-app modules` print. It is
the same list as `[modules] enabled` in `brocanth.toml`, so a test in the
CLI holds the two equal.

### `src/modules/analytics.rs`

As the generator writes it, in a project with `admin`:

```rust
//! `analytics` -- page views counted on the server.
//!
//! Written once by `brocanth add analytics`. This file is yours: no command
//! rewrites it. A change here takes effect on the next `cargo build`.
//!
//! A value that differs per deployment is a setting: `[analytics]` in
//! config/<env>.toml, `$MY_BLOG_ANALYTICS__<KEY>`, or a flag. Not here.

use crate::plugins::shape::analytics::{Hooks, Integrate, Key, Module};
use crate::plugins::shape::{Flag, Flags};

// Every field is written out so you can read the choices. The last line
// gives a default to a field that a later version of the module adds.
#[allow(clippy::needless_update)]
pub const MODULE: Module = Module {
    // The command-line flags of [analytics]. `Flags::Project` follows
    // src/modules/brocanth.rs. `Flags::On` and `Flags::Off` decide here.
    flags: Flags::Project,

    // Another name for a flag, or no flag. The variable and the config key
    // keep the declared name. Examples:
    //   (Key::Site, Flag::As("site")),       // --site
    //   (Key::IpHeader, Flag::Hidden),       // no flag
    rename: &[],

    hooks: Hooks {
        // `analytics.count`: count each page this server answers.
        // Off for a server that only collects beacons from a static site.
        count: true,
        // `analytics.collect`: accept beacons at `collector_path` from
        // `collector_origins`. Nothing is accepted while that list is empty.
        collect: true,
        ..Hooks::DEFAULT
    },

    integrate: Integrate {
        // A tab at /admin/analytics with the page-view numbers. Without the
        // admin module this does nothing.
        admin: true,
        ..Integrate::DEFAULT
    },

    ..Module::DEFAULT
};
```

### `src/modules/sessions.rs`

As a developer edited it. The flags are on beside a config file, and
`--sessions.cookie` is `--cookie`:

```rust
//! `sessions` -- a cookie, and a `Session` the context hands a handler.
//!
//! Written once by `brocanth add sessions`. This file is yours.

use crate::plugins::shape::sessions::{Key, Module};
use crate::plugins::shape::{Flag, Flags};

#[allow(clippy::needless_update)]
pub const MODULE: Module = Module {
    flags: Flags::On,
    rename: &[(Key::Cookie, Flag::As("cookie"))],
    ..Module::DEFAULT
};
```

`sessions` mounts a layer and registers no hook, so its `Module` has no
`hooks` field. It offers no integration and declares no option.

### `src/modules/admin.rs`

```rust
//! `admin` -- a Django-shaped backoffice, locked until you say who may in.
//!
//! Written once by `brocanth add admin`. This file is yours. Who may open a
//! screen is in src/plugins/admin/mod.rs, not here.

use crate::plugins::shape::admin::{Key, Module};
use crate::plugins::shape::{Flag, Flags};

#[allow(clippy::needless_update)]
pub const MODULE: Module = Module {
    flags: Flags::Project,
    // (Key::Title, Flag::As("admin-title")),
    rename: &[],

    // Where the backoffice answers. A page of yours under this path is a
    // build error, not a race. Every environment serves the same routes,
    // so this is a constant and not a setting.
    mount_at: "/admin",

    ..Module::DEFAULT
};
```

`admin` adapts to `middleware-csrf`, `admin-model` and `commands`. None of
these is a toggle. Each is structural: without the CSRF token every admin
form is refused, and the other two follow from a module the project chose.
See §2.2 for the rule.

### `src/modules/middleware_rate_limit.rs`

A module with a required hook and an optional one:

```rust
//! `middleware-rate-limit` -- a request limit per client and rule.
//!
//! Written once by `brocanth add middleware-rate-limit`. This file is yours.
//!
//! Always on: `rate_limit.count`. Without it this module does nothing.

use crate::plugins::shape::middleware_rate_limit::{Hooks, Key, Module};
use crate::plugins::shape::{Flag, Flags};

#[allow(clippy::needless_update)]
pub const MODULE: Module = Module {
    flags: Flags::Project,
    rename: &[],
    hooks: Hooks {
        // `rate_limit.headers`: send RateLimit-* headers on each response.
        headers: true,
        ..Hooks::DEFAULT
    },
    ..Module::DEFAULT
};
```

### `src/modules/brocanth.rs`

```rust
//! The framework itself. Written once. This file is yours.

use crate::plugins::shape::brocanth::{Key, Module};
use crate::plugins::shape::Flag;

#[allow(clippy::needless_update)]
pub const MODULE: Module = Module {
    // With the `config` module, whether the binary reads flags too. Off:
    // the file and the environment are the sources, and a flag is refused
    // with a pointer to this line. Without `config`, flags are always read.
    // A module file with `Flags::On` or `Flags::Off` overrides this.
    flags_with_config: false,
    // (Key::ShutdownGraceSecs, Flag::As("grace")),
    rename: &[],
    ..Module::DEFAULT
};
```

### `src/plugins/shape.rs` (generated, an excerpt)

```rust
//! Written by `brocanth`. Do not edit -- `brocanth add` rewrites this file.
//!
//! The shape of each file in src/modules/. Each type has a `DEFAULT`, so a
//! field that a later version adds never breaks a file that predates it.

/// Whether a module's settings have flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flags {
    /// On without a config file. With one, `brocanth::MODULE.flags_with_config`.
    Project,
    On,
    Off,
}

/// The flag of one setting, when it is not the declared name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flag {
    /// `--<name>` in place of `--<section>.<key>`.
    As(&'static str),
    /// No flag. The variable and the config key still set the value.
    Hidden,
}

pub mod analytics {
    use super::{Flag, Flags};

    /// A setting of `[analytics]` that has a flag. A secret or a list has
    /// no flag, so it has no variant here and cannot be renamed.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Key {
        Enabled,
        Site,
        RetentionDays,
        FlushInterval,
        RespectGpc,
        IpHeader,
        CountryHeader,
        Migrate,
        CollectorPath,
    }

    impl Key {
        pub const fn path(self) -> &'static str {
            match self {
                Key::Enabled => "analytics.enabled",
                Key::Site => "analytics.site",
                // ...
                Key::CollectorPath => "analytics.collector_path",
            }
        }
    }

    pub struct Hooks {
        pub count: bool,
        pub collect: bool,
    }

    impl Hooks {
        pub const DEFAULT: Self = Self { count: true, collect: true };

        /// The hook names this choice turns off, for `hooks_off`.
        pub fn off(&self) -> Vec<&'static str> {
            let mut off = Vec::new();
            if !self.count {
                off.push("count");
            }
            if !self.collect {
                off.push("collect");
            }
            off
        }
    }

    pub struct Integrate {
        pub admin: bool,
    }

    impl Integrate {
        pub const DEFAULT: Self = Self { admin: true };
    }

    pub struct Module {
        pub flags: Flags,
        pub rename: &'static [(Key, Flag)],
        pub hooks: Hooks,
        pub integrate: Integrate,
    }

    impl Module {
        pub const DEFAULT: Self = Self {
            flags: Flags::Project,
            rename: &[],
            hooks: Hooks::DEFAULT,
            integrate: Integrate::DEFAULT,
        };
    }
}
```

## 2. What a module file holds

Four groups. Each is typed, and the compiler checks each name.

### 2.1 Flags: on, off, rename, hide

| Field | Type | Replaces |
|---|---|---|
| `flags` | `Flags::Project`, `On` or `Off` | `WITH_CONFIG`, per module |
| `rename` | `&[(Key, Flag)]` | the module's lines of `RENAMES` |

**Decision: both a project rule and a module override.** The project rule
is `brocanth::MODULE.flags_with_config`. A module file says
`Flags::Project` by default and follows it. **Reason:** the common change
is "flags on for everything" or "flags on for one module". Only the
project rule makes the first change one line. Only the override makes the
second one possible.

How the generated code resolves it:

```rust
// src/plugins/mod.rs, generated
fn flags_on(rule: Flags) -> bool {
    match rule {
        Flags::On => true,
        Flags::Off => false,
        Flags::Project => !HAS_CONFIG || crate::modules::brocanth::MODULE.flags_with_config,
    }
}
```

`Flags::Off` in a project without `config` leaves a setting with the
variable only. That is valid: it is the same as `Flag::Hidden` on each key.

A flag given while it is off is refused, as today. The `help:` line names
the module file:

```text
error: `--sessions.cookie` is off: this project reads its settings from config/<env>.toml
help: set `flags: Flags::On` in src/modules/sessions.rs, or set `cookie` under [sessions] in config/<env>.toml
```

**`Key` is an enum, not a string.** A typo, a key that the module no longer
declares, and a rename of a secret are compile errors. Today they are
startup errors.

### 2.2 Integrations

An integration is code that one module writes because another module is
present, and that the project can do without. The analytics tab in the
admin is one. The CSRF token on admin forms is not: without it the admin
breaks. That stays `adapts_to`, with no toggle.

**Decision: the toggle is in the file of the module that is integrated
(the guest), keyed by the host.** `analytics.rs` holds
`integrate.admin`. **Reason:** a person thinks "do I want analytics in
the admin?", and looks for the answer in `analytics.rs`. The admin can host
many tabs. One file per tab keeps `admin.rs` short.

This moves one declaration. Today `admin` declares
`adapts_to("analytics", ...)` and writes `ANALYTICS_PANEL` into
`src/plugins/admin/mod.rs`. After this, `analytics` declares the
integration and writes the panel line into a new slot, `Slot::AdminPanels`.
`admin` reads the slot into a generated function in `src/plugins/mod.rs`.

**How the generated code reads it: the code exists when both modules
exist, and a `const` guards it.**

```rust
// src/plugins/mod.rs, generated with `admin` and `analytics`
pub fn admin_panels(mut config: brocanth_admin::AdminConfig) -> brocanth_admin::AdminConfig {
    if crate::modules::analytics::MODULE.integrate.admin {
        config = config.panel(brocanth_analytics::admin::AnalyticsPanel::installed());
    }
    config
}
```

```rust
// src/plugins/admin/mod.rs, the project's file: one line, written once
config = crate::plugins::admin_panels(config);
```

Three shapes were compared:

| Shape | Change the choice | Cost | Verdict |
|---|---|---|---|
| Code written only when the toggle is on | The CLI must read the Rust file, then regenerate | The CLI parses user Rust. A choice needs `brocanth add` to take effect. | Rejected |
| Code always written, `const` guard | Edit the file, `cargo build` | Does not compile: the code names `brocanth_analytics`, which is linked only with `analytics` | Impossible |
| **Code written when both modules exist, `const` guard** | Edit the file, `cargo build` | None at run time: rustc removes `if false`. The `admin` feature of `brocanth-analytics` stays linked. | **Chosen** |

**Why `add` never needs to rewrite the module file:** the integration code
lives in `src/plugins/mod.rs`, which the generator owns. When `admin`
arrives after `analytics`, `add` rewrites `src/plugins/mod.rs` and adds
`admin_panels`. `analytics.rs` already holds `integrate.admin`, because the
field exists whether `admin` is present or not (§4).

### 2.3 Hooks

A hook is one `registrar.request(..)`, `request_async(..)`, `response(..)`
or `error(..)` call in a plugin's `install_hooks`. Its id is
`<instance>.<name>`. Real ids today:

| Module | Hooks | Toggle |
|---|---|---|
| `analytics` | `analytics.count`, `analytics.collect` | both, on |
| `middleware-csrf` | `csrf.check`, `csrf.mint` | none: both required |
| `middleware-rate-limit` | `rate_limit.count`, `rate_limit.headers` | `headers`, on; `count` required |
| `middleware-security-headers` | `security_headers.set` | none: required |
| `gtag` | `gtag.consent-log`, `gtag.insert` | `consent_log`, on; `insert` required |
| `i18n` (when it is a module) | `i18n.negotiate`, `i18n.content-language` | `content_language`, on; `negotiate` required |

**A hook is on by default.** A module may declare a hook off by default
(`Hook::off()`), for behaviour that a project must ask for.

**A required hook has no field.** `Hooks` for `middleware-csrf` has no
`check` field, so a file cannot turn it off. The compiler enforces it, and
the file's header comment lists the hooks that are always on. When every
hook is required, `Module` has no `hooks` field.

**Decision: the plugin is registered, and the core drops the off hooks.**

```rust
// src/plugins/mod.rs, the registration chain, generated
let builder = builder
    .plugin_hooking_from_config::<brocanth_analytics::Analytics>("analytics")
    .hooks_off("analytics", crate::modules::analytics::MODULE.hooks.off());
```

`AppBuilder::hooks_off(instance, names)` is new in the core. The
`HookRegistrar` of that instance skips a hook whose name is in the list,
and records it as skipped. A name that the plugin never registers stops the
build: `analytics has no hook "colect"`. Generated code cannot produce that
error, but a hand-written call can.

**Reasons for filtering, not "do not register the plugin":**

- A plugin carries more than hooks. `analytics` also publishes its recorder
  (the admin tab reads it), its store and a readiness check that creates
  the tables. A server that only collects beacons needs all of these
  without `analytics.count`.
- Registration is all or nothing per plugin. A plugin has several hooks.
- The type of the hook record (`HookedAnalytics`, `Both<...>`) stays the
  same. The generator does not read the module file, so the types must not
  depend on it. An off hook is never pushed into the `Pipeline`, so it
  costs nothing per request.

**A hook off is absent for ordering.** Hook order is declared in each
crate's code (`Order::new().after(..)`, `before_if_present(..)`):

- `after_if_present("x.y")` with `x.y` off: the constraint does nothing,
  as when the plugin is absent. That is what `_if_present` means.
- `after("x.y")` with `x.y` off: **the build is refused at startup**, before
  a socket is bound. Today the core already refuses an `after` that nothing
  answers. The message gains the reason and the fix:

```text
error: `audit.trail` runs after `rate_limit.headers`, which src/modules/middleware_rate_limit.rs turns off
help: set `headers: true` in src/modules/middleware_rate_limit.rs, or change the order of `audit.trail`
```

**Why startup, not `brocanth check`:** the order lives in the crates'
`install_hooks`. The CLI links no module runtime (see commit `e305bc6`), so
it cannot see an `Order`. The startup check runs in every `cargo test` that
builds the application, so CI finds it before a deploy.

**`analytics.enabled` and `hooks.count` are two different questions.**
`enabled` is a setting: this deployment counts, or does not (development
does not). `hooks.count` is static: this binary never counts its own pages.
Both stay. With `count: false` the settings report prints
`analytics.enabled = true  (hook analytics.count is off in src/modules/analytics.rs)`.

**Relation to the `middleware` children.** Today one child module maps to
one plugin, not to one hook: `middleware-csrf` is the `csrf` plugin with
`check` and `mint`. A child decides whether the plugin, its crate feature,
its `[csrf]` section and its `AGENTS.md` text exist. A hook toggle decides
which hooks of a chosen plugin run. The two never overlap, so no child
becomes a toggle and no toggle replaces a child.

Rules that follow:

- To remove all of a module, use `brocanth remove`. A toggle cannot remove
  a dependency or a config section.
- A plugin that installs no hook prints a warning at startup:
  `warning: rate_limit installs no hook; help: brocanth remove middleware-rate-limit`.
  With required hooks this cannot happen, but a module whose hooks are all
  optional can reach it.

### 2.4 Static options

An option is a compile-time value a module declares. It is the same in
every environment, and a change needs a rebuild in any case.

| Module | Option | Default | Today |
|---|---|---|---|
| `admin` | `mount_at: &'static str` | `"/admin"` | a literal in the `RouterMounts` line of `src/pages/mod.rs` |
| `realtime-sse` | none: its path is the page file `src/pages/realtime/events.rs` | | |
| `analytics` | none today | | |

`robots` disallows `/admin` when the admin is present. That is not an
option: it is an integration, `integrate.admin` in `robots.rs`, on by
default.

The generated code reads an option at the place that used a literal:

```rust
// src/pages/mod.rs, the RouterMounts line
crate::plugins::admin::mount(routes, crate::modules::admin::MODULE.mount_at)?;
```

Kinds: `&'static str`, `bool`, `i64`, `u64`. An enum option is a
follow-up, when a module needs one.

**The test for "option or setting":** would two deployments of one image
want different values? Then it is a setting. Does the value change which
routes exist, which code runs, or what the type system sees? Then it is an
option.

### 2.5 What must not go in a module file

| Not here | Why | Where |
|---|---|---|
| A secret: a database URL, `storage.secret`, `metrics_token` | The file is in git. The value is compiled into the binary, and `strings` prints it. | the environment, or a secret store through config |
| A value per environment: the cookie name of production, a pool size | One image runs in every environment. A constant cannot differ between them. | `config/<env>.toml`, a variable, a flag |
| Whether a module is in the project | A constant cannot remove a crate from `Cargo.toml`, a section from the config file, or a file from `src/`. | `brocanth add`, `brocanth remove`; recorded in `brocanth.toml` |
| A rename of the variable or the config key | They are shared with every other deployment and with the docs. | not renamed; only the flag is |
| Code: a policy, a principal, a handler | The module file is data that the generator understands. | `src/plugins/<key>/mod.rs`, `src/app.rs` |

## 3. Manifest and API changes

### 3.1 In `brocanth-scaffold`

```rust
Manifest::new("analytics", "...")
    .hook("count", "count each page this server answers", Hook::on())
    .hook("collect", "accept beacons from collector_origins", Hook::on())
    .integration("admin", "a tab at /admin/analytics with the page-view numbers", true)

Manifest::new("middleware-csrf", "...")
    .hook("check", "refuse an unsafe request without the token", Hook::required())
    .hook("mint", "set the token cookie", Hook::required())

Manifest::new("admin", "...")
    .option(Opt::text("mount_at", "/admin", "Where the backoffice answers."))
```

| New item | Meaning |
|---|---|
| `Manifest::hook(name, what, Hook)` | `Hook::on()`, `Hook::off()`, `Hook::required()`. `name` is the name the plugin passes to the registrar, without the instance. |
| `Manifest::integration(host, what, default)` | Implies `adapts_to(host, what)`, so the existing test on `project.has` still holds. |
| `Manifest::option(Opt)` | `Opt::text`, `Opt::bool`, `Opt::integer`, `Opt::unsigned`, each with a key, a default and one line of help. |
| `Slot::AdminPanels` | Read by `admin` into `plugins::admin_panels`. Open to `Category::Enhancer` and `Category::Middleware`. |
| `snippet::module_ref(key, path)` | Returns `crate::modules::<ident>::MODULE.<path>`. A snippet never spells that path by hand. |
| `snippet::integration(s, guest, host, slot, code)` | Writes `code` into `slot`, inside `if crate::modules::<guest>::MODULE.integrate.<host> { .. }`. |
| `snippet::hooks_off(key, instance)` | Returns the `.hooks_off(..)` line for the registration chain. `configurable_hooking` calls it. |

**Why the manifest, not `contribute`:** hooks, integrations and options are
facts of the module, the same in every project. `shape.rs` must not change
when another module arrives (§4). Settings stay in `contribute`, because
their values depend on the project (`sessions.cookie` is
`_<ident>_session`), but their keys do not.

**A test holds the hook list to the code.** Each crate with a `HookSource`
has a test under its `scaffold` feature: it installs the plugin into a test
registrar and compares the names with `Manifest::hook`. A crate that adds a
hook and forgets the manifest fails its own test.

### 3.2 In the core (`brocanth`)

- `AppBuilder::hooks_off(instance, impl IntoIterator<Item = &'static str>)`.
- `HookRegistrar` skips the listed names and records them.
- `Pipeline::skipped()`, for the settings report and the routes command.
- The "nothing answers to that name" error names a skipped hook and its
  module file.

**Built** (`crates/brocanth`, tests in `tests/hooks_off.rs`):

- `hooks_off(instance, names)` and `Pipeline::skipped() -> &[(&'static str, &'static str)]`,
  as above. Two calls for one instance add up. An empty list does nothing.
- `hooks_off_from(instance, file, names)`: the same, with the module file.
  **Reason:** the core does not know which module registered an instance
  (`rate_limit` comes from `middleware-rate-limit`), so it cannot find the
  file. Only the generator knows it. With the file, a hard `after` on an
  off hook gives the message of §2.3 exactly. The field in `help:` is the
  hook name with `-` written as `_` (`consent-log` is `consent_log`).
  Without the file, the message names the call:
  ``which `hooks_off("rate_limit", ..)` turns off``, and
  ``help: remove "headers" from `hooks_off("rate_limit", ..)` ``. The
  generated line is therefore
  `.hooks_off_from("rate_limit", "src/modules/middleware_rate_limit.rs", crate::modules::middleware_rate_limit::MODULE.hooks.off())`.
- `build()` refuses an unknown instance, an unknown hook name
  (`analytics has no hook "colect"`, with `did you mean` and the names the
  instance registers), and a hard `after` or `before` on an off hook. The
  last check runs before the sort, per chain: an `Order` resolves inside
  one chain only (request, response, error, or one model event), so an off
  response hook does not refuse an `after` of a request hook. The check
  also covers an `after("rate_limit")` when every hook of `rate_limit` in
  that chain is off.
- `after_if_present` and `before_if_present` on an off hook do nothing.
- Not built: the warning for a plugin that installs no hook (§2.3). The
  core does not know the module key for its `help:` line. The generated
  code can read `Pipeline::skipped()` and print it.

### 3.3 Where each mistake is found

| Mistake | Found | Message |
|---|---|---|
| A field that does not exist (typo, or a toggle that a newer module dropped) | compile | rustc E0560: `struct Module has no field named ...` |
| A value of the wrong type | compile | rustc |
| A rename of a key that has no flag (unknown, secret, list) | compile | rustc: no variant `Key::...` |
| An attempt to turn off a required hook | compile | rustc: no field |
| A field that a newer module adds | never | `..DEFAULT` gives it the default |
| A flag name that is not spelled as a flag, `help`, `version`, `settings` | startup, and the generated test `flag_names_are_valid` | as today, with the module file in `help:` |
| Two flags with one name, or a flag that a command already uses | startup, and the same test | as today, naming both module files |
| A hard `after` on a hook that is off | startup | §2.3 |
| A plugin with no hook | startup warning | §2.3 |
| An integration on while its host is absent | not an error | the field does nothing; the file comment says so |
| A recommended module that is missing | `brocanth check` | `check/recommended`, as today |

**Why not `brocanth check` for the value checks:** the CLI reads
`brocanth.toml` as text and does not compile the project. To read a
`const` it must build the project, which the project's own `check` command
already does. That command calls `plugins::flag_names()`, so
`brocanth check` on a project with `commands` reports the same errors
without new code in the CLI.

**Why not a `const` assertion for collisions:** a `const` panic cannot
format a message, so it cannot name the two flags. The startup check can,
and the generated test runs it in `cargo test`.

## 4. Ownership and lifecycle

| File | Written | Rewritten | Owner |
|---|---|---|---|
| `src/modules/mod.rs` | `create`, `add`, `remove` | every time the module list changes | generator |
| `src/plugins/shape.rs` | `create`, `add`, `remove` | every time; after an upgrade of a module | generator |
| `src/modules/<key>.rs` | `create`, `add`, when it is missing | never | project |
| `src/modules/brocanth.rs` | `create`, or the first `add` after this lands | never | project |

**The shape that survives upgrades: a struct literal with `..DEFAULT`.**

- A newer module adds a toggle. `shape.rs` gains the field and puts its
  default in `DEFAULT`. The old file compiles and takes the default. `add`
  prints the new field so the person can choose:
  `analytics 0.5 adds hooks.collect (on). Your file takes the default.`
- A newer module drops a toggle. The old file names a field that does not
  exist, and the build fails with rustc's error. `add` sees this first,
  because it knows both shapes, and prints the line to delete with a
  `help:` line. This is rare, and a silent drop would hide a choice that
  no longer has an effect.
- The file writes every field out. **Reason:** the file is where a person
  reads what can be configured. A file with only `..DEFAULT` shows nothing.
  Clippy's `needless_update` warns when every field is written and
  `..DEFAULT` follows, so the generator writes
  `#[allow(clippy::needless_update)]` on `MODULE`.

Two other shapes were weighed:

| Shape | On a new toggle | On a dropped toggle | Verdict |
|---|---|---|---|
| One `pub const` per choice (`pub const ADMIN_TAB: bool = true;`) | the generated code names a `const` that the old file lacks: compile error | the file has a dead `const`: a warning | Rejected: every upgrade breaks the build |
| Defaults in the crate, overrides in the file (`impl Overrides for ...`) | works | works | Rejected: trait machinery for a list of booleans; the file no longer shows the defaults |
| **Struct literal with `..DEFAULT`** | works | compile error naming the field | **Chosen** |

**Fields do not depend on the other modules.** `Integrate::admin` exists
in a project without `admin`. **Reason:** `remove admin` must not break
`analytics.rs`, which the project owns and `remove` must not edit. The same
rule makes `add admin` work without a change to `analytics.rs`.

**`Key` variants can depend on a child.** `observability` declares
`live_path` only with `observability-health`. After
`remove observability-health`, a rename of `Key::LivePath` in
`observability.rs` no longer compiles. `remove` checks the module files
for such a rename and prints the line to delete before it writes anything.

### Interaction with `brocanth remove` and module versions

Another agent builds `brocanth remove` and module versions now. This
design asks for the following, and builds none of it:

- **`remove <key>` and the module file.** An unedited file (byte equal to
  what the generator writes for the recorded version) is deleted. An
  edited file stays, out of `mod.rs`, so it does not compile. `remove`
  says: `kept src/modules/analytics.rs: your choices return with brocanth add analytics`.
  `add` writes a file only when it is missing, so a re-add picks the old
  choices up. This is item 1 of the module-system list in
  `docs/module-catalog.md`: a rename survives a remove and a re-add.
- **`remove <host>`.** A guest's `integrate.<host>` stays and does
  nothing. `remove` rewrites `src/plugins/mod.rs`, so `admin_panels` goes.
- **Versions.** `brocanth.toml` records the version of each module. `add`
  and an upgrade compare the old and the new `Manifest::hook`,
  `integration` and `option` lists, and print each new or dropped field.
  The module file can carry the version that wrote it in its header, for
  the "unedited" test above.

## 5. Which file answers which question

| Question | File | Owner | Changes take effect |
|---|---|---|---|
| Which modules does this project contain? | `brocanth.toml` (`[modules] enabled`) and `src/modules/mod.rs` (`ALL`) | generator | `brocanth add`, `brocanth remove` |
| Which crates, files and config sections did each module bring? | `brocanth.toml` (`[modules.<key>]`) | generator | `add`, `remove` |
| Does a module have flags? Under which names? | `src/modules/<key>.rs` (`flags`, `rename`) | project | `cargo build` |
| Which hooks of a module run? | `src/modules/<key>.rs` (`hooks`) | project | `cargo build` |
| Does a module plug into another one (admin tab)? | `src/modules/<key>.rs` (`integrate`) | project | `cargo build` |
| Where is the admin mounted? | `src/modules/admin.rs` (`mount_at`) | project | `cargo build` |
| What is the cookie name in production? | `config/production.toml`, `$<APP>_SESSIONS__COOKIE`, `--sessions.cookie` | shared | restart |
| What does the plugin do at run time? Who may open the admin? | `src/plugins/<key>/mod.rs` | project | `cargo build` |
| In which order are plugins registered? How are flags parsed? | `src/plugins/mod.rs` | generator | `brocanth add` |
| What type does a module file fill? | `src/plugins/shape.rs` | generator | `brocanth add`, an upgrade |

## 6. Migration

### `src/flags.rs`

`src/flags.rs` goes away. Its two items move:

| Before | After |
|---|---|
| `WITH_CONFIG` | `flags_with_config` in `src/modules/brocanth.rs` |
| a `RENAMES` line for `sessions.cookie` | `rename` in `src/modules/sessions.rs`, as `(Key::Cookie, Flag::As("cookie"))` |
| `("storage.root", "")` | `(Key::Root, Flag::Hidden)` in `src/modules/storage.rs` |
| a rename of `server.shutdown_grace_secs` | `rename` in `src/modules/brocanth.rs` |
| `mod flags;` in `src/main.rs` | `mod modules;` |

The first `brocanth add` after this lands migrates an existing project:

1. It reads `src/flags.rs` as text. It accepts the two forms the generator
   wrote: `pub const WITH_CONFIG: bool = <bool>;` and `RENAMES` entries of
   two string literals.
2. It writes `src/modules/` with those values in place of the defaults.
3. It deletes `src/flags.rs`, and prints each moved line.
4. When the file holds anything else, `add` writes nothing and prints the
   lines to move by hand, with
   `help: move these lines to src/modules/<key>.rs, then delete src/flags.rs`.
   This follows rule 3 of `add` in `docs/modules.md`: an edit is never
   overwritten.

### `src/plugins/admin/mod.rs`

A project generated before this has the analytics panel line in the admin
file, without a guard. That line still compiles and still works, but
`integrate.admin` does not reach it.

- An unedited admin file is rewritten (rule 2 of `add`), with
  `config = crate::plugins::admin_panels(config);` in place of the panel.
- An edited admin file gets the change printed, and `add` goes on. This is
  the `src/app.rs` exception: without the line the project still works,
  and the tab stays on.

### The goldens

Every golden gains `src/modules/mod.rs`, `src/modules/brocanth.rs`,
`src/plugins/shape.rs` and one file per module with a choice. `src/flags.rs`
leaves every golden. The ignored test
`declared_flags_reach_the_plugin_with_and_without_a_config_file` edits
`src/modules/sessions.rs` in place of `src/flags.rs`, and gains a case for a
hook that is off and an integration that is off.

## 7. Decisions (Max, 2026-11-09)

1. **The framework's own file is `src/modules/brocanth.rs`.** It holds the
   project flag rule and the `[server]` renames. `src/flags.rs` goes away.
2. **Every module gets a file**, also a module with no choice yet. Reason:
   Max asked that an added module "goes to `src/modules/<name>.rs`"; one
   rule for every module is easier to read than a rule with exceptions, and
   a module that gains a choice later already has its file. A module with
   no choice gets a file with `..Module::DEFAULT` and a comment. This
   replaces the "only with a choice" proposal in §1.
3. **The guest holds an integration toggle** (`analytics.rs` holds
   `integrate.admin`). The analytics panel line moves to
   `Slot::AdminPanels`.
4. **A hook that is off, and a hard `after` that needs it: refuse at
   startup with a `help:` line.** Nothing is turned off silently.
5. **Every field is written out, with its comment, then `..DEFAULT`.**
