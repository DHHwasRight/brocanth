# `storage-s3`

The object store is an S3 bucket, or one on MinIO or R2; checked at start.

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
| Crates | `brocanth-storage` (feature `s3`) |
| Without it | another child of `storage` holds the files |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/storage_s3.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | `ObjectStore` (an object store): objects in an S3 bucket, under `[storage] prefix` |
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

It writes no file. `storage` writes `CloudBucket::s3` behind
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

**Credentials.** `access_key_id` and `secret_access_key`, or neither: then
the AWS credential chain answers (the `AWS_*` variables, a web identity
token on EKS, the container credentials on ECS, the instance role). The
secret has no flag: read it from a file.

```sh
brocanth create shop --storage --storage-s3
./shop serve --storage.bucket=shop-uploads --storage.region=eu-west-1
# MinIO or R2:
./shop serve --storage.bucket=uploads --storage.endpoint=http://localhost:9000 \
  --storage.access_key_id=minioadmin --storage.secret_access_key_file=/run/secrets/s3
```

## Settings

All under `[storage]`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `bucket` | text | `"uploads"` | `--storage.bucket` | The S3 bucket. It must exist. |
| `region` | text | `"us-east-1"` | `--storage.region` | The bucket's region. R2 takes `auto`. |
| `endpoint` | text | `""` | `--storage.endpoint` | An S3-compatible server's URL. Empty means AWS. |
| `path_style` | bool | `true` | `--storage.path_style` | Path-style requests, which MinIO wants. |
| `access_key_id` | text | `""` | `--storage.access_key_id` | The access key. Empty: the AWS credential chain. |
| `secret_access_key` | text | `""` | none (secret): `--storage.secret_access_key_file`, `--storage.secret_access_key_env` | The secret key. |
| `prefix` | text | `""` | `--storage.prefix` | A folder inside the bucket. |
| `probe` | bool | `true` | `--storage.probe` | Check the bucket and the credentials before the server listens. |
| `max_object_bytes` | integer | `67108864` | `--storage.max_object_bytes` | The largest object stored, in bytes. |

## Read more

- [`storage`](storage.md), and the design: [`docs/object-storage.md`](../object-storage.md)
- [brocanth-storage](../../crates/brocanth-storage/README.md)
- Source: [`crates/brocanth-storage/src/scaffold.rs`](../../crates/brocanth-storage/src/scaffold.rs)
