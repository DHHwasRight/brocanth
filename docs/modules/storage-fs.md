# `storage-fs`

The object store is a directory on this machine's disk; the default.

The wizard shows it under `storage`, in the Data question.

| | |
|---|---|
| Categories | **Enhancer** |
| Wizard question | Data |
| Default | off |
| Presets | none |
| Part of | [`storage`](storage.md), off until you tick it |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-storage` |
| Without it | another child of `storage` holds the files |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/storage_fs.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | `ObjectStore` (an object store): objects in a directory under `[storage] root` |
| Requires | none |
| Uses | none |
| Adapts to | none |
| Contributes | none |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | none |
<!-- end generated: capabilities -->

## What it does

It writes no file. `storage` writes `LocalDisk::open(root)` into
`src/plugins/storage/mod.rs`. The objects live under `[storage] root`
(`var/uploads`): `objects/ab/cd/<address>`, a `.meta` sidecar with the
media type and the original name, and `variants/` for thumbnails. Every
write is atomic and `fsync`ed; the directory is `0700`, each file `0600`,
and the root holds a `.gitignore` of `*`.

**The default.** `storage` requires an object store. With none chosen,
`brocanth create` and `brocanth add` add this one and say `added
storage-fs: storage requires ObjectStore`. Reason: one binary with
embedded Postgres and a directory beside it needs no other service.

**One machine.** A second instance does not see these files, and a new
machine has none. `brocanth check --deploy` warns with
`deploy/storage-local`. `brocanth add storage-s3` (or `storage-gcs`,
`storage-db`) replaces this module.

## Settings

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `root` | text | `"var/uploads"` | `--storage.root` | Where uploads are stored. |

## Read more

- [`storage`](storage.md), and the design: [`docs/object-storage.md`](../object-storage.md)
- [brocanth-storage](../../crates/brocanth-storage/README.md)
- Source: [`crates/brocanth-storage/src/scaffold.rs`](../../crates/brocanth-storage/src/scaffold.rs)
