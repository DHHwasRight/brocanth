# `storage`

Uploads on a local disk, or in Postgres with `db`, addressed by content hash; no S3.

The wizard calls it: *Uploads on disk or in Postgres, addressed by content hash*.

| | |
|---|---|
| Categories | **Enhancer**, Middleware |
| Wizard question | Data |
| Default | off |
| Presets | none |
| Part of | nothing |
| Needs | none |
| Uses | `PgPool`: the objects live in the `db` database, on its pool, and not on one machine's disk |
| After | [`config`](config.md) |
| Adapts to | [`config`](config.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-storage` (feature `postgres` with [`db`](db.md)) |
| Without it | no file storage; an upload has nowhere to go and no address to be read back from |

## What it writes

Files:

- `src/plugins/storage/mod.rs`

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs (the context): `let files: &plugins::storage::FileStore = cx.extract();`
- src/plugins/mod.rs: `pub mod storage;`
- src/plugins/mod.rs (the provider list): `storage crate::plugins::storage::Files`
- config/<env>.toml: `[storage]`

## Settings

Section `[storage]`. Environment variable: `$<APP>_STORAGE__<KEY>`. Flag: `--storage.<key>`. Rename or hide a flag in `src/modules/storage.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `root` | text | `"var/uploads"` | `--storage.root` | Without `db` only. Where uploads are stored. The store writes a `.gitignore` that holds `*` into it, so the ignore rule follows this setting. |
| `migrate` | bool | `true` | `--storage.migrate` | With `db` only. Create the two storage tables at startup if they are absent. Development only. |
| `secret` | text | `"development only: replace before anything is uploaded in anger"` | none (secret) | Signs file tokens and keys object addresses. |
| `token_ttl` | integer | `120` | `--storage.token_ttl` | Seconds a file token lives. |

Outside development, the plugin refuses to start with the generated secret
or a secret under 32 characters. Reason: anybody who read this repository
knows the generated one, and with it can mint a token for any object.

With `db`, the objects live in Postgres on the `db` pool (`PgStorage`), so
every instance sees them. Without it, they are on one machine's disk, and
`brocanth check --deploy` warns with `deploy/storage-local`.

## Read more

- [brocanth-storage](../../crates/brocanth-storage/README.md)
- Source: [`crates/brocanth-storage/src/scaffold.rs`](../../crates/brocanth-storage/src/scaffold.rs)
