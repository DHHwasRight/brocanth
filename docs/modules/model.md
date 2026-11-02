# `model`

SeaORM, validations, migrations; without it no SQL driver at all.

The wizard calls it: *An ORM with validations and migrations*.

| | |
|---|---|
| Categories | **Enhancer**, Middleware |
| Wizard question | Data |
| Default | off |
| Presets | HTML application, JSON API, Something that does work |
| Part of | nothing |
| Needs | none |
| After | none |
| Adapts to | [`config`](config.md), [`commands`](commands.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-model` (features `sqlite`) |
| Without it | no ORM, no SQL driver, no sea-orm anywhere in the lock file |

## What it writes

Files:

- `migrations/mod.rs`
- `src/plugins/model/mod.rs`

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs (the context): `let db: &crate::plugins::model::Db = cx.extract();`
- src/plugins/mod.rs: `pub mod model;`
- src/plugins/mod.rs (the provider list): `model crate::plugins::model::Model`
- config/<env>.toml: `[model]`

## Settings

Section `[model]`. Environment variable: `$<APP>_MODEL__<KEY>`. Flag: `--model.<key>`. Rename a flag in `src/flags.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `url` | text | `"sqlite://db/development.sqlite?mode=rwc"` | none (secret) | The database: a SQLite file, or a Postgres URL. |

## Read more

- [Migrations](../migrations.md)
- [brocanth-model](../../crates/brocanth-model/README.md)
- Source: [`crates/brocanth-cli/src/modules/model.rs`](../../crates/brocanth-cli/src/modules/model.rs)
