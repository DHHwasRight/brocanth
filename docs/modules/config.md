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
| Needs | none |
| After | none |
| Adapts to | none |
| Conflicts | none |
| Outside the runtime | yes |
| Crates | `brocanth-config` |
| Without it | no TOML parser; main builds each plugin from a literal |

## What it writes

No file and no slot of its own.

`create.rs` writes the config files, not this module: its `contribute` is empty. With `config`, every module setting below has a key in `config/<env>.toml`, and the flags are off until `WITH_CONFIG = true` in `src/flags.rs`.

## Settings

None.

## Read more

- [Module settings and flags](../module-flags.md)
- [brocanth-config](../../crates/brocanth-config/README.md)
- Source: [`crates/brocanth-cli/src/modules/config.rs`](../../crates/brocanth-cli/src/modules/config.rs)
