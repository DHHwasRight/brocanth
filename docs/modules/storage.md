# `storage`

Uploads on a local disk, addressed by content hash; no object store and no S3.

The wizard calls it: *Uploads on disk, addressed by content hash*.

| | |
|---|---|
| Categories | **Enhancer**, Middleware |
| Wizard question | Data |
| Default | off |
| Presets | none |
| Part of | nothing |
| Needs | none |
| After | [`config`](config.md) |
| Adapts to | [`config`](config.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-storage` |
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
- .gitignore: `/var/uploads`

## Settings

Section `[storage]`. Environment variable: `$<APP>_STORAGE__<KEY>`. Flag: `--storage.<key>`. Rename or hide a flag in `src/modules/storage.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `root` | text | `"var/uploads"` | `--storage.root` | Where uploads are stored. |
| `secret` | text | `"development only: replace before anything is uploaded in anger"` | none (secret) | Signs file tokens and keys object addresses. |
| `token_ttl` | integer | `120` | `--storage.token_ttl` | Seconds a file token lives. |

## Read more

- [brocanth-storage](../../crates/brocanth-storage/README.md)
- Source: [`crates/brocanth-storage/src/scaffold.rs`](../../crates/brocanth-storage/src/scaffold.rs)
