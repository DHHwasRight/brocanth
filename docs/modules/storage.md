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
- `src/plugins/storage/stores.rs`, with a named store only (see below)

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

## Named stores

A project has one default store and zero or more named ones
([`docs/object-storage.md`](../object-storage.md) §9.1):

```text
brocanth add storage-s3@public      # a named store on S3, beside the default one
brocanth remove storage-s3@public   # takes it out, and its config section
brocanth add storage-s3             # still swaps the default store
```

`create --with storage,storage-s3@public` works the same. A named binding
meets no requirement and is no second provider: `storage` beside
`storage-s3@public` alone still brings `storage-fs` as the default. The
provider enters `[modules] enabled` once, and the binding goes in
`[named_bindings]` of `brocanth.toml`.

| Rule | Reason |
|---|---|
| A name is `[a-z][a-z0-9_]{0,23}`, not `default`, not a key of `[storage]` or `[storage_routes]`, not a Rust keyword | it is a TOML table, a flag segment, a Rust field and a URL segment |
| One binding per name | the name addresses the store |
| `storage-db` backs one binding at most, default or named | its tables have fixed names |

What the project gets:

- `src/plugins/storage/stores.rs`: `NamedStores` on the context and in the
  services, one field per name, each a `NamedStore { store, tokens }`. A
  handler reads `ctx.provided::<NamedStores, _>()?.public.store`. `FileStore`
  does not change.
- Settings in `[storage.<name>]`, flags `--storage.<name>.<key>`. A named
  disk store defaults to `root = "var/uploads-<name>"`. A named cloud store
  has no default bucket: with none, the start stops with a `help:` line.
  `secret` and `token_ttl` stay in `[storage]`.
- A token key per store: the one secret, with the store name in the domain
  separator. A token of one store opens nothing in another. The default
  store's key does not change.
- Two stores at one location stop the start: the same directory, or the
  same endpoint, bucket and prefix.
- A named S3 or GCS store may redirect its downloads to a signed URL,
  with `downloads` and `signed_ttl` under `[storage_routes.<name>]`
  ([`storage-routes`](storage-routes.md#named-stores)).
- `check --deploy` runs `deploy/storage-local` per store, and names a named
  store on a disk.

## Read more

- [brocanth-storage](../../crates/brocanth-storage/README.md)
- Source: [`crates/brocanth-storage/src/scaffold.rs`](../../crates/brocanth-storage/src/scaffold.rs)
