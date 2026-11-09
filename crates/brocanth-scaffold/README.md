# brocanth-scaffold

`brocanth-scaffold` is the contract between `brocanth create` and a module.
A module is a unit struct that implements `Module`. Its `Manifest` says
what it is called, what it costs and what it needs. Its `contribute`
method writes files it owns and lines into named `Slot`s of files the
generator owns. Use this crate when you write a module that `brocanth
create` and `brocanth add` can offer. Do not use it at run time: the
plugin half of a module implements `brocanth::plugin_api::Plugin`, and an
application never links this crate. The crate has no dependencies, so it
costs a module crate nothing.

## Getting started

Take the crate behind a `scaffold` feature that is off by default. An
application that depends on your crate then never links the generator
(PLAN.md §8):

```toml
[features]
default = []
scaffold = ["dep:brocanth-scaffold"]

[dependencies.brocanth-scaffold]
git = "https://github.com/DHHwasRight/brocanth"
branch = "main"
optional = true
```

Then add two lines to the CLI and rebuild it. There is no distributed
slice: the list in the CLI is the receipt for what a generated binary
contains, so a module cannot arrive through a transitive dependency.

```toml
# crates/brocanth-cli/Cargo.toml
brocanth-correlation = { git = "https://github.com/you/brocanth-correlation", features = ["scaffold"] }
```

```rust,ignore
// crates/brocanth-cli/src/modules/mod.rs, inside ALL
&brocanth_correlation::scaffold::Correlation,
```

`ignore`: it is one line of an array in the CLI, and the crate does not
exist.

```text
cargo install --path crates/brocanth-cli
brocanth create my-app --with correlation
brocanth add correlation
```

This crate is not a module, so it has no key and no settings.

## The contract, in short

[docs/module-authoring.md](../../docs/module-authoring.md) has the full
guide. A summary:

- **Two halves.** The scaffold module runs once, in `create` or `add`,
  and writes source. The plugin runs in the generated binary. A module
  that writes all its code into the project, such as `logging`, has no
  plugin crate.
- **`Manifest::new(key, gist)`**, then builders: `part_of` /
  `part_of_but_off` (a parent), `because` (why it sits in the parent),
  `after` (order only), `needs` (a requirement with its reason),
  `krate(Dep)` (dependencies of the generated project), `absent` (what
  `brocanth.toml` says when the module is not chosen), `blocked`,
  `asked_in` (which wizard question), `outside_runtime`, `static_output`.
- **Manifest rules.** Three relations a module declares itself, so a
  module from another repository needs no edit to the CLI:
  - `category(Category::X)`: the kinds of seam it hooks into, one call
    per kind. The first is the main one. `Category` is `Middleware`,
    `Renderer`, `External`, `Enhancer` or `Routes`.
    `Slot::open_to` says which categories may write each slot, and a CLI
    test fails when a module writes outside its categories.
  - `conflicts(key, why)`: it cannot be chosen beside `key`. Declare it
    on the side that gives way.
  - `adapts_to(key, what_changes)`: it writes different code when `key`
    is chosen. Declare one for each key `contribute` asks `Project::has`
    about that is not a need or the parent. A CLI test fails on an
    undeclared question and on a declaration nothing asks.
- **`clash(a, b)`** returns a `Clash` when two manifests cannot both be
  chosen: a `conflicts` entry on either side, or the static-site rule (a
  `static_output` module takes only `outside_runtime` modules). `create`,
  `add` and the chooser all ask it.
- **`Project`** is read-only: `name`, `title`, `has(key)`, `ident()`.
  `has` on a key that no module declares returns `false` and is reported,
  so a typo never reads as "not chosen".
- **`Scaffold`** collects the output: `file`, `at(slot)` with `line`,
  `once` and `block`, `binary`, `setting`, `refuse`. Two modules that
  write one path, declare one binary, or declare one setting are a
  problem that names both. The CLI then writes nothing.
- **Slots** are a closed list: `AppUses`, `AppPlugins`, `AppSetup`,
  `AppContext`, `PluginsMod`, `PluginsInstalled`, `PluginsHooks`,
  `PluginsLayers`, `AdminPanels`, `ServerWrap`, `MainMods`,
  `PagesRenderers`, `RouterMounts`, `PagesRoutes`, `PagesItems`,
  `CommandsMod`, `ConfigSections`, `GitIgnore`, `MiseTasks`,
  `NodeDependencies`, `AgentsSections`. A new seam is a change to this
  crate.
- **Snippets** write lines that must agree, from one call:
  `snippet::plugin` (registration and provider list), `snippet::hooking`
  (plus the hook record), `snippet::configurable` (plus the declared
  settings and, with `config`, the `[section]` block), and
  `snippet::configurable_hooking` (a hooking plugin from a library crate
  with declared settings). `configurable_hooking` takes a
  `snippet::Hooking`: the section is the plugin's instance name, and
  `config_ty` names the library's configuration type, which
  `Scaffold::settings_type` records. With `config`, the registration goes
  through `brocanth-config`'s `plugin_hooking_from_config`.
- **Settings**: `Setting::text`, `bool`, `integer`, `float`, `list`, with
  `.note`, `.secret`, `.optional`, `.none_as(alias)`, `.commented`. Each
  setting gives a generated project a flag `--<section>.<key>`, a
  variable `$<APP>_<SECTION>__<KEY>`, and with `config` a key under
  `[<section>]`. See [docs/module-flags.md](../../docs/module-flags.md).
- **Choices** a project makes in its `src/modules/<key>.rs`:
  `Manifest::hook(name, what, Hook::on() | Hook::off() | Hook::required())`,
  `Manifest::integration(host, what, default)` and
  `Manifest::option(Opt::text | bool | integer | unsigned)`. Read them in
  generated code with `snippet::module_ref`, and guard an integration with
  `snippet::integration`. A hooking plugin with a toggle records its
  instance (`Scaffold::hooking_instance`; `configurable_hooking` does it).
  See [docs/module-config.md](../../docs/module-config.md).

## Examples

A module that registers a plugin with declared settings:

```rust
use brocanth_scaffold::{snippet, Dep, Manifest, Module, Project, Scaffold, Setting, Slot};

pub struct Cache;

impl Module for Cache {
    fn manifest(&self) -> Manifest {
        Manifest::new("cache", "a cache in Postgres, on an unlogged table")
            .krate(Dep::brocanth("brocanth-cache"))
            .after("config")
            .absent("cache", "no cache; every read goes to the database")
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/cache/mod.rs", "// the plugin\n");
        s.at(Slot::PluginsMod).line("pub mod cache;");
        snippet::configurable(
            s,
            project,
            "cache",
            "Cache",
            vec![Setting::text("url", "postgres://localhost/app", "Where the cache table is.")
                .secret()],
        );
    }
}

// What the CLI does with it, in short:
let known = ["cache", "config"];
let on = ["cache", "config"];
let project = Project::new("my-app", "My App", &on, &known);
let mut s = Scaffold::new();
s.now_writing("cache");
Cache.contribute(&project, &mut s);

assert!(s.problems().is_empty());
assert_eq!(s.slot(Slot::PluginsMod).collect::<Vec<_>>(), ["pub mod cache;\n"]);
let section: String = s.slot(Slot::ConfigSections).collect();
assert!(section.contains("[cache]"));
assert_eq!(s.settings().count(), 1);
```

The TOML that declared settings write. A dotted key is a subsection, and
an optional key is written commented out:

```rust
use brocanth_scaffold::{config_section, Setting};

let text = config_section(
    "db",
    &[
        Setting::text("url", "sqlite://db", "Where the database is.").secret(),
        Setting::integer("pool.max_size", 5, "Connections at most."),
        Setting::integer("pool.limit", -1, "Rows per page.").none_as("-1"),
    ],
);
let expected = [
    "",
    "[db]",
    "# Where the database is.",
    "url = \"sqlite://db\"",
    "",
    "[db.pool]",
    "# Connections at most.",
    "max_size = 5",
    "# Rows per page. Empty or `-1` means none.",
    "# limit = -1",
    "",
];
assert_eq!(text, expected.join("\n"));
```

Two modules that claim one path are a problem that names both:

```rust
use brocanth_scaffold::Scaffold;

let mut s = Scaffold::new();
s.now_writing("cache");
s.file("src/plugins/thing/mod.rs", "// cache\n");
s.now_writing("jobs");
s.file("src/plugins/thing/mod.rs", "// jobs\n");
assert_eq!(s.problems(), ["`cache` and `jobs` both write src/plugins/thing/mod.rs"]);
```

A module that does the same work as another declares a conflict. `clash`
reads it from either side and says which one to drop:

```rust
use brocanth_scaffold::{clash, Category, Manifest};

let logging = Manifest::new("logging", "one line per request, written into the project")
    .category(Category::Middleware)
    .conflicts("observability-logs", "both write one line per request");
let logs = Manifest::new("observability-logs", "structured request logs")
    .category(Category::Middleware);

let found = clash(&logs, &logging).expect("they clash");
assert_eq!(found.about, "logging");
assert_eq!(found.help, "keep `observability-logs` and drop `logging`");
assert!(clash(&logging, &Manifest::new("config", "a config file")).is_none());
```

The CLI orders the chosen modules by their `after` lists:

```rust
use brocanth_scaffold::order;

let after = |key: &str| match key {
    "cache" => vec!["config"],
    _ => vec![],
};
assert_eq!(order(&["cache", "config"], after), Ok(vec!["config", "cache"]));
```

## Testing

```text
cargo test -p brocanth-scaffold
```

No database and no environment variable is needed.

A module's real tests live in the CLI, because they need the generator:

- `crates/brocanth-cli/src/modules/mod.rs` checks every manifest in `ALL`:
  unique keys, real parents and `after` names, a `because` for each child,
  a reason for each `needs`, a short `blocked` reason, a `gist` with no
  full stop, at least one category, writes only into slots its categories
  may write, and an `adapts_to` for each `Project::has` question.
- `crates/brocanth-cli/src/commands/create.rs` compares each shape with a
  golden file, runs rustfmt over every generated `.rs` file, and checks
  that `add` writes what `create` writes. Bless the goldens and read the
  diff:

  ```text
  BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- generator_writes
  ```

Then generate a project and build it, because a golden proves only the
text:

```text
brocanth create try --with correlation --brocanth-path ~/src/brocanth/crates/brocanth
cd try && cargo build && cargo test && cargo clippy --all-targets -- -D warnings
```

A module crate tests itself with the `testing` feature:
`brocanth_scaffold::testing` checks a generated project (rustfmt, a golden
file, a build), and `brocanth_cli::testing::generate` generates it with
the module as a `&dyn Module`. No fork of the CLI is needed.
`crates/brocanth-robots/tests/scaffold.rs` is the example, and
`docs/module-authoring.md` §4 has the walkthrough.

```text
cargo test -p brocanth-scaffold --features testing
```

## Links

- [docs/module-authoring.md](../../docs/module-authoring.md): the
  contract, a walkthrough, every hook, the rules and the checklist.
- [docs/module-map.md](../../docs/module-map.md): the 38 modules by
  category, and how they depend on each other.
- [docs/modules.md](../../docs/modules.md): why the seams are where they
  are.
- [docs/module-flags.md](../../docs/module-flags.md): declared settings,
  flags and the settings report.
