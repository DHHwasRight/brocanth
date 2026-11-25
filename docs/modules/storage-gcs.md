# `storage-gcs`

The object store is a Google Cloud Storage bucket; checked at start.

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
| Crates | `brocanth-storage` (feature `gcs`) |
| Without it | another child of `storage` holds the files |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/storage_gcs.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | `ObjectStore` (an object store): objects in a GCS bucket, under `[storage] prefix` |
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

It writes no file. `storage` writes `CloudBucket::gcs` behind
`ObjectStorage` into `src/plugins/storage/mod.rs`. The objects live in the
bucket under `prefix`, with the layout `storage-fs` uses on disk.

**Checked at start.** The client is built at `configure`: an empty
`bucket`, half a key or a bad endpoint stops the start there. Before the
server listens, one list of at most one object checks the bucket and the
credentials. A bucket that does not exist, a refused key and a server that
does not answer each stop the start with a `help:` line.
`--storage.probe=false` skips the check for a start with no network.

**Metadata.** The media type is the object's `Content-Type`, the original
name a metadata entry (`filename`). Postgres holds nothing for it.

**Downloads.** `storage-routes` still proxies the bytes, so the headers that
make a hostile upload harmless stay on the response. `Storage::signed_url`
answers a URL that reads the object directly, for code that wants one.

**Credentials.** A service-account key in JSON, read from a file with
`--storage.credentials_file`, or none: then the application default
credentials answer (`GOOGLE_APPLICATION_CREDENTIALS`, `gcloud`, the
metadata server for GKE workload identity and Cloud Run). A signed URL
needs the key, or a service account allowed to sign blobs.

```sh
brocanth create shop --with storage,storage-gcs
./shop --storage.bucket=shop-uploads --storage.credentials_file=/run/secrets/gcs.json
```

`endpoint` is for an emulator such as fake-gcs-server; with it, requests
carry no OAuth token.

## Settings

All under `[storage]`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `bucket` | text | `"uploads"` | `--storage.bucket` | The GCS bucket. It must exist. |
| `credentials` | text | `""` | none (secret): `--storage.credentials_file`, `--storage.credentials_env` | A service-account key, as JSON. Empty: application default credentials. |
| `endpoint` | text | `""` | `--storage.endpoint` | An emulator's URL. Empty means Google. |
| `prefix` | text | `""` | `--storage.prefix` | A folder inside the bucket. |
| `probe` | bool | `true` | `--storage.probe` | Check the bucket and the credentials before the server listens. |
| `max_object_bytes` | integer | `67108864` | `--storage.max_object_bytes` | The largest object stored, in bytes. |

## Read more

- [`storage`](storage.md), and the design: [`docs/object-storage.md`](../object-storage.md)
- [brocanth-storage](../../crates/brocanth-storage/README.md)
- Source: [`crates/brocanth-storage/src/scaffold.rs`](../../crates/brocanth-storage/src/scaffold.rs)
