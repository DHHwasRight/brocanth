# `storage`

Uploads addressed by content hash, in the project's object store: a disk, S3, GCS or Postgres.

The wizard calls it: *Uploads, addressed by content hash*.

| | |
|---|---|
| Categories | **Enhancer**, Middleware |
| Wizard question | Data |
| Default | off |
| Presets | none |
| Part of | nothing |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-storage`; the backend module adds its feature |
| Without it | no file storage; an upload has nowhere to go and no address to be read back from |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/storage.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | none |
| Requires | `ObjectStore` (an object store): the files live in the project's object store; with none chosen, a directory on this machine; providers: [`storage-fs`](storage-fs.md), [`storage-db`](storage-db.md), [`storage-s3`](storage-s3.md), [`storage-gcs`](storage-gcs.md) |
| Uses | none |
| Adapts to | none |
| Contributes | `AgentsSection` `storage`: where the files live, and how code reaches them |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | `[storage]`: `root`, `secret` (secret), `token_ttl` |
<!-- end generated: capabilities -->

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
| `root` | text | `"var/uploads"` | `--storage.root` | With `storage-fs` only. Where uploads are stored. The store writes a `.gitignore` that holds `*` into it, so the ignore rule follows this setting. |
| `migrate` | bool | `true` | `--storage.migrate` | With `storage-db` only. Create the two storage tables at startup if they are absent. Development only. |
| `bucket`, `region`, `endpoint`, `path_style`, `access_key_id`, `secret_access_key`, `prefix`, `probe`, `max_object_bytes` | | | `--storage.<key>` | With `storage-s3`. See [`storage-s3`](storage-s3.md). |
| `bucket`, `credentials`, `endpoint`, `prefix`, `probe`, `max_object_bytes` | | | `--storage.<key>` | With `storage-gcs`. See [`storage-gcs`](storage-gcs.md). |
| `secret` | text | `"development only: replace before anything is uploaded in anger"` | none (secret) | Signs file tokens and keys object addresses. |
| `token_ttl` | integer | `120` | `--storage.token_ttl` | Seconds a file token lives. |

Outside development, the plugin refuses to start with the generated secret
or a secret under 32 characters. Reason: anybody who read this repository
knows the generated one, and with it can mint a token for any object.

## Where the files live

`storage` requires the `ObjectStore` Kind. Four children provide it, and a
project takes exactly one:

| Module | The files live | Every instance sees them |
|---|---|---|
| [`storage-fs`](storage-fs.md) (default) | in a directory under `root` | no: `check --deploy` warns with `deploy/storage-local` |
| [`storage-s3`](storage-s3.md) | in an S3 bucket, or one on MinIO or R2 | yes |
| [`storage-gcs`](storage-gcs.md) | in a Google Cloud Storage bucket | yes |
| [`storage-db`](storage-db.md) | in two Postgres tables on the `db` pool | yes |

With none chosen, `brocanth create` adds `storage-fs` and says so. `brocanth
add storage-s3` replaces the current backend; `brocanth remove storage-s3`
puts `storage-fs` back. The backend modules write no file: this module
writes the open code for the bound one into `src/plugins/storage/mod.rs`,
and every caller holds `Arc<dyn Storage>`. A project made before the Kind
keeps its backend: `storage` with `db` reads as `storage-db`. The design is
in [`docs/object-storage.md`](../object-storage.md).

## Read more

- [brocanth-storage](../../crates/brocanth-storage/README.md)
- Source: [`crates/brocanth-storage/src/scaffold.rs`](../../crates/brocanth-storage/src/scaffold.rs)
