# `config`

Config/<env>.toml, merged; without it nothing links a TOML parser.

The wizard calls it: *Settings in config/<env>.toml, layered per environment*.

| | |
|---|---|
| Categories | **Enhancer** |
| Wizard question | Foundations |
| Default | on in a non-interactive `create` |
| Presets | HTML application, JSON API, Something that does work, Static site |
| Part of | nothing |
| Conflicts | none |
| Outside the runtime | yes |
| Crates | `brocanth-config` |
| Without it | no TOML parser; main builds each plugin from a literal |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/config.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | `ConfigSource` (settings read from a file): config/<env>.toml, a layer under the environment and the flags |
| Requires | none |
| Uses | none |
| Adapts to | [`commands`](commands.md): a `check` command runs the configuration and deploy checks |
| Contributes | none |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | none |
<!-- end generated: capabilities -->

## What it writes

No file and no slot of its own.

`create.rs` writes the config files, not this module: its `contribute` is empty. With `config`, every module setting below has a key in `config/<env>.toml`, and the flags are off until `flags_with_config: true` in `src/modules/brocanth.rs`.

## Settings

None.

## Read more

- [Module settings and flags](../module-flags.md)
- [brocanth-config](../../crates/brocanth-config/README.md)
- Source: [`crates/brocanth-cli/src/modules/config.rs`](../../crates/brocanth-cli/src/modules/config.rs)
