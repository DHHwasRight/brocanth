# Object storage: one abstraction, three backends

Status: accepted (2026-11-25). Owner decision: 2026-11-24. ADR 13 in
`PLAN.md` records the reversal of the §4 cut ("no S3").

Read first: `crates/brocanth-storage` (`storage.rs`, `local.rs`,
`postgres.rs`, `deferred.rs`, `routes.rs`), `docs/module-capabilities.md`
(Kinds, the resolver), `docs/modules/storage*.md`.

## 0. Decisions, one line each

| # | Decision | Reason |
|---|---|---|
| D1 | Two layers: `Storage` stays the content-addressed contract callers name; a new `Bucket` trait is the raw contract a backend implements. | `Storage` holds the security rules (addresses that cannot spell a path, sniffed media types, first upload owns the metadata). A backend author must not reimplement them for each cloud. |
| D2 | `ObjectStorage` implements `Storage` once, over any `Bucket`. | One copy of the address layout, the dedupe and the variant cascade for S3, GCS and a directory. |
| D3 | `LocalDisk` and `PgStorage` stay as they are and stay `Storage` backends. | No behaviour change for current projects: same files, same rows, same addresses. |
| D4 | A Kind `ObjectStore`, runtime `dyn brocanth_storage::Storage`, cardinality one. | Callers never name a backend. Two stores in one project is a bug until a project shows a need (D6). |
| D5 | Four provider modules: `storage-fs` (default), `storage-s3`, `storage-gcs`, `storage-db`. | One module per backend is what `brocanth explain`, `brocanth.toml` and `check --deploy` can see. |
| D6 | Exactly one provider per project. No named bindings in v1. | The routes, the tokens and the thumbnails all read one store. A second bucket (public assets) is a value the application builds itself from the same crate. |
| D7 | The resolver learns a **default provider**: a Kind with several providers may name one with `Manifest::default_for::<K>()`. A requirement with no chosen provider adds it, and says so. | The `db` rule (add the single provider) does not reach a Kind with four providers. The owner chose the filesystem as the default. |
| D8 | `brocanth add storage-s3` replaces the default provider; `brocanth remove storage-s3` puts the default back. | Swapping a backend is one command each way, and the selection never holds two stores. |
| D9 | A project recorded before the Kind existed keeps its backend: `storage` with `db` reads as `storage-db`, without `db` as `storage-fs`. | Today `storage` + `db` keeps the bytes in Postgres. An `add` must not move them to a disk in silence. |
| D10 | Client library: `object_store` 0.14 (Apache Arrow project), features `aws` and `gcp`. | One maintained crate for both clouds: streaming, ranges, listing, conditional create and signed URLs, on the tokio and reqwest 0.13 the workspace already links. See §4. |
| D11 | Backend settings stay in `[storage]`: `--storage.bucket=…`, `--storage.secret_access_key_file=…`. | One plugin file, one section. The current `[storage] root` keeps working. A swap rewrites one file. |
| D12 | Secrets have no value flag: `secret_access_key` and `credentials` are read through `_file` or `_env`. | A flag is in `ps` output and in shell history. |
| D13 | No key in the config means the platform's credential chain (AWS env, web identity, instance role; GCS ADC, workload identity). | A deployed service on EKS, ECS, GKE or Cloud Run has no key to give, and should not have one. |
| D14 | A bad config fails the start: empty `bucket` at `configure`, a missing bucket or a refused credential at the readiness check before bind. `probe = false` skips the network check. | The error comes at deploy, with a `help:` line, not at the first upload. The flag covers an offline start. |
| D15 | Metadata (media type, original name) is object metadata on S3 and GCS, a sidecar on a disk, a column in Postgres. Postgres holds no new data. | Owner rule: Postgres only for metadata, and no second database. The object carries what it needs to be served. |
| D16 | The download route keeps proxying the bytes. Signed URLs exist on the traits; the route does not redirect to one in v1. | The route sets `Content-Disposition`, `nosniff` and the CSP that make a hostile upload harmless. A redirect to a bucket loses them unless each URL carries response overrides; that is a design of its own. |
| D17 | Streaming is in `Bucket` (put and get). `Storage` keeps `&[u8]` and `Vec<u8>`, and gains `get_range` and `signed_url`. | The HTTP edge reads a whole multipart body under `max_body_bytes` (16 MiB) before it calls the store, so a streaming `Storage` would stream nothing today. Code that moves large files uses the `Bucket` it builds. |
| D18 | Tests: one conformance suite, run on every backend. Disk always; Postgres, MinIO and fake-gcs-server when their variable is set, like the Postgres tests. | The suite is the definition of a backend. A skip is loud. |

## 1. What exists

`brocanth-storage` has one trait, `Storage`, and two backends.

| Piece | What it does |
|---|---|
| `Storage` | `put`, `put_at`, `get`, `meta`, `exists`, `delete`, `get_variant`, `put_variant`. Async, object-safe, `&self`. Bodies are slices. |
| `ObjectKey` | 32 bytes, rendered as 64 hex characters. SHA-256 or HMAC-SHA256 of the content, or random. No other value exists. |
| `LocalDisk` | `<root>/objects/ab/cd/<hex>`, a `.meta` sidecar, `<root>/variants/ab/cd/<hex>/<spec>`. Atomic writes, `fsync`, `0700`/`0600`. |
| `PgStorage` (feature `postgres`) | Two `bytea` tables, `ON CONFLICT DO NOTHING`, a cascade from object to variants, a 32 MiB default limit. |
| `Deferred` | A store set at `before_start`, for a pool that opens late. |
| routes, variants, tokens | Read `Arc<dyn Storage>` only. |

The trait already says "nothing above the trait changes when an object
store arrives". This design keeps that promise: the routes, the tokens,
the thumbnails and every generated handler keep `Arc<dyn Storage>`.

What the trait lacks for a cloud: a range read, a signed URL, and a way
to delete variants without a directory to remove. The first two are new
default methods. The third is `Bucket::list`, used by `ObjectStorage`.

## 2. The traits

### 2.1 `Bucket`: what a backend implements

A flat namespace of named objects: a directory, an S3 bucket, a GCS
bucket.

```rust
#[async_trait]
pub trait Bucket: Send + Sync + 'static {
    async fn put(&self, path: &ObjectPath, body: Body, options: PutOptions)
        -> Result<ObjectHead, StorageError>;
    async fn get(&self, path: &ObjectPath, range: Option<ByteRange>)
        -> Result<GetObject, StorageError>;      // head + a byte stream
    async fn head(&self, path: &ObjectPath) -> Result<ObjectHead, StorageError>;
    async fn delete(&self, path: &ObjectPath) -> Result<(), StorageError>; // idempotent
    async fn list(&self, prefix: &ObjectPath) -> Result<Vec<ObjectHead>, StorageError>;
    async fn signed_url(&self, method: SignedMethod, path: &ObjectPath, ttl: Duration)
        -> Result<Option<String>, StorageError>; // None: this backend cannot sign
    async fn probe(&self) -> Result<(), StorageError>; // the start-time check
}
```

- **`ObjectPath`** is validated once: segments of `[A-Za-z0-9._-]`, no
  empty, `.` or `..` segment, no leading `/`, at most 1024 bytes. A
  client-sent string is never one; `ObjectStorage` builds every path
  from hex and a `VariantSpec`. On a disk the same rule is the traversal
  defence.
- **`Body`** is bytes or a stream of chunks. `PutOptions` holds the media
  type, user metadata, `max_bytes`, and `create_only`.
- **Size limit.** The backend counts while it reads the stream and stops
  at `max_bytes` with `StorageError::TooLarge`. Nothing past the limit is
  stored.
- **`create_only`** maps to S3 `If-None-Match: *` and GCS
  `ifGenerationMatch=0`; on a disk, a link that fails on an existing
  name. It is what keeps "the first upload owns the metadata" true when
  two instances store one file at the same moment.
- **`list`** answers every object under a prefix. It is for the store's
  own bookkeeping (the variants of one object) and for code that owns a
  prefix. `Storage` still has no list: an enumeration of a
  content-addressed store is an enumeration of everything anybody
  uploaded.
- **`signed_url`** for `Get` and `Put`. A disk answers `None`.
- **Errors** are `StorageError`. A cloud error becomes `NotFound`,
  `TooLarge`, or `Backend` with a `help:` line that names the setting to
  check. The message names the operation, never the object path.

### 2.2 `Storage`: what callers name

Unchanged, plus two default methods:

```rust
async fn get_range(&self, key: &ObjectKey, range: ByteRange) -> Result<Vec<u8>, StorageError>;
async fn signed_url(&self, key: &ObjectKey, ttl: Duration) -> Result<Option<String>, StorageError>;
```

The defaults read the whole object and slice, and answer `None`.
`ObjectStorage` overrides both. A signed `Put` is not on `Storage`: the
address is the hash of the bytes, unknown before they arrive, and a
direct upload would skip the sniffing.

### 2.3 `ObjectStorage`: `Storage` over a `Bucket`

```text
objects/ab/cd/<hex>             the bytes; media type and filename as metadata
variants/ab/cd/<hex>/<spec>     a derived image
```

The layout is `LocalDisk`'s. `FsBucket` writes metadata in `LocalDisk`'s
sidecar format, so `ObjectStorage` over `FsBucket` and `LocalDisk` read
each other's files (a test proves it). `put` sniffs the type, puts with
`create_only`, and treats "already there" as success. `delete` lists and
deletes the variants first, then the object: a thumbnail of a deleted
document is still the document. `put_variant` refuses a variant of an
object that is gone, and takes it back if a delete ran during the write,
as `LocalDisk` does. `with_max_object_bytes` sets the limit (default
64 MiB).

### 2.4 Backends

| Backend | Type | Feature | Bucket ops |
|---|---|---|---|
| disk | `LocalDisk` (unchanged); `FsBucket` | none | `FsBucket`: all but signed URLs |
| Postgres | `PgStorage` (unchanged) | `postgres` | not a `Bucket`: `Storage` only |
| S3 and compatible | `CloudBucket::s3(S3Config)` | `s3` | all |
| GCS | `CloudBucket::gcs(GcsConfig)` | `gcs` | all |

S3 and GCS live in `brocanth-storage` behind features, like `postgres`
does. Reason: the generator that writes `CloudBucket::s3` lives beside
it, and a feature that is off means the code is absent (§8.2). The CLI
links the crate with no default features and none of these.

## 3. Modules and the resolver

### 3.1 The Kind

```rust
kind!(ObjectStore, group = "storage", runtime = "dyn brocanth_storage::Storage",
      gist = "an object store", One);
```

`storage` (the tokens, the secret, the `FileStore` context) declares
`requires::<ObjectStore>`. It stops declaring `uses::<PgPool>`;
`storage-db` requires `PgPool` instead.

| Module | Provides | Requires | Crate feature | Writes |
|---|---|---|---|---|
| `storage-fs` | `ObjectStore`, default | none | none | nothing; `storage` writes `LocalDisk` |
| `storage-db` | `ObjectStore` | `PgPool` | `postgres` | nothing; `storage` writes `PgStorage` |
| `storage-s3` | `ObjectStore` | none | `s3` | nothing; `storage` writes `CloudBucket::s3` |
| `storage-gcs` | `ObjectStore` | none | `gcs` | nothing; `storage` writes `CloudBucket::gcs` |

The backend modules write no file. `storage` asks
`project.provider::<ObjectStore>()` and writes the matching open code
into `src/plugins/storage/mod.rs`, with the backend's settings. Reason:
the token and secret logic is one block of a few hundred lines; four
copies of it would drift. The provider keys are this crate's own modules,
so the branch stays inside one crate (`module-capabilities.md` §9).

### 3.2 The default provider

`Manifest::default_for::<K>()` marks the module as the default provider
of `K`. Rules, enforced by a test over the registry: at most one default
per Kind, and only for a Kind of cardinality one with two or more
providers. The default is never development-only.

`resolve` adds, for a requirement nobody meets: the single deployable
provider (as for `db`), else the default, else it refuses and names each
provider. It prints `added storage-fs: storage requires ObjectStore (an
object store)`.

Requirements with a single provider resolve first. Then the defaults.
So `storage storage-db jobs` adds `db` once, for both.

### 3.3 Swapping

- `brocanth add storage-s3` in a project with `storage-fs`: a Kind with
  a default is a backend choice, so the new provider replaces the old
  one (`replaced storage-fs: storage-s3 provides ObjectStore`). Without
  that, the selection holds two stores and is refused.
- `brocanth remove storage-s3`: the default comes back (`added
  storage-fs`). Removing the only provider of a Kind without a default
  is still refused.
- `brocanth create --storage --storage-s3`: the requirement is met, so
  nothing is added.

### 3.4 Projects from before

`add` and `remove` read `[modules] enabled`. When it holds `storage` and
no provider of `ObjectStore`, the CLI reads `storage-db` if `db` is
there, else `storage-fs`, and says so once. The generated storage file
for each is byte for byte what the project already has, so the add finds
no edit and rewrites only `brocanth.toml`.

## 4. The client library

| Option | For | Against |
|---|---|---|
| **`object_store` 0.14** (Apache Arrow, MIT/Apache-2.0) | S3, GCS, Azure, local in one crate. Streaming put (multipart) and get, ranges, list, `PutMode::Create`, a `Signer` for S3 and GCS. Credential chains for both clouds. tokio and reqwest 0.13, which the workspace already links; `aws-lc-rs` is already in the lock. Used by DataFusion, Polars, delta-rs; released monthly. | No per-request response headers on a signed URL (D16 does not need them). |
| `aws-sdk-s3` + `google-cloud-storage` | Official. | Two SDKs, two HTTP stacks, two credential systems. `aws-sdk-s3` alone is the largest dependency tree on crates.io (smithy runtime, its own hyper client). The Google crate moved to gRPC. |
| `rust-s3` | Small. | S3 only; a second crate for GCS anyway. |
| `opendal` | Many services. | A larger abstraction of its own over ours; more surface than the four calls we need. |

`object_store` it is. Features `aws` and `gcp` only, no default features
(no `fs`: `FsBucket` keeps `LocalDisk`'s discipline: `fsync`, `0600`,
the sidecar).

## 5. Configuration

All in `[storage]`. `secret` and `token_ttl` stay as they are.

| Backend | Setting | Default | Note |
|---|---|---|---|
| fs | `root` | `var/uploads` | unchanged |
| db | `migrate` | `true` | unchanged |
| s3 | `bucket` | `uploads` | empty fails at `configure`; a missing bucket fails the start check |
| s3 | `region` | `us-east-1` | R2: `auto` |
| s3 | `endpoint` | empty (AWS) | MinIO, R2: the URL. `http://` allows plain HTTP |
| s3 | `path_style` | `true` | `false` for virtual-hosted style |
| s3 | `access_key_id` | empty | empty: the AWS credential chain |
| s3 | `secret_access_key` | empty | secret: `_file` or `_env` only |
| gcs | `bucket` | `uploads` | as for S3 |
| gcs | `credentials` | empty | service-account JSON; secret: `_file` or `_env` only. Empty: ADC |
| gcs | `endpoint` | empty | an emulator's URL (fake-gcs-server) |
| s3, gcs | `prefix` | empty | a folder inside the bucket, so two applications share one |
| s3, gcs | `probe` | `true` | `false` skips the start-time bucket check |
| s3, gcs | `max_object_bytes` | 67108864 | `ObjectStorage`'s limit |

The start check (D14) lists one object under the prefix. The answers
map so:

| Answer | Error |
|---|---|
| no such bucket | `[storage] bucket "x" does not exist` / `help: create it, or set --storage.bucket` |
| 401, 403 | `[storage] the credentials cannot read bucket "x"` / `help: check access_key_id and secret_access_key_file (or the role's policy)` |
| connect error | `[storage] cannot reach <endpoint>` / `help: check --storage.endpoint, or set --storage.probe=false to start offline` |

`brocanth check --deploy` keeps its rule for `storage-fs`: one machine's
disk, said as a warning.

## 6. Interactions

- **`storage-routes`.** Unchanged. It reads `Arc<dyn Storage>` and the
  token service. Downloads proxy the bytes (D16). The upload limit stays
  `max_bytes`, under the server's `max_body_bytes`, and the backend's own
  limit is the backstop.
- **Thumbnails and `image`.** Variants are stored through
  `Storage::put_variant`, so on S3 they live in the same bucket under
  `variants/`. Delete removes them first. Nothing changes for a caller.
- **`static`.** `static` conflicts with `storage` (it needs a server) and
  still does. An export that copies assets to a bucket is a later use of
  `Bucket`, not a v1 feature.
- **Backoffice.** The Files tab is still blocked on an index
  (`docs/backoffice.md` §4). `Bucket::list` is not the index: it lists
  addresses, not records, and holds no owner. An index is a Postgres
  table of metadata, which is in the owner's rule; it is deferred.
- **Embedded Postgres.** The default (`storage-fs`) fits the single
  binary: Postgres in the process, files beside it. `storage-db` stays
  for a deployment that wants one thing to back up.

## 7. Testing

| Suite | Backends | Runs when |
|---|---|---|
| `Storage` conformance (put, dedupe, first-owner metadata, get, range, meta, exists, delete cascade, variants, limit, signed URL shape) | `LocalDisk`, `ObjectStorage<FsBucket>`, `PgStorage`, `ObjectStorage<S3>`, `ObjectStorage<GCS>` | disk always; others with their variable |
| `Bucket` conformance (put, streaming put, limit, create-only, get, range, head, metadata, delete, list, signed get and put) | `FsBucket`, S3, GCS | as above |
| cross-read | `LocalDisk` and `ObjectStorage<FsBucket>` on one root | always |

Variables: `BROCANTH_STORAGE_TEST_S3` (an endpoint URL; MinIO's
`minioadmin` keys) and `BROCANTH_STORAGE_TEST_GCS` (a fake-gcs-server
URL). Each test makes its own bucket, so runs never share state. A skip
prints a banner past libtest's capture, as the Postgres tests do.

Images, pinned by digest:
`pgsty/minio:RELEASE.2026-08-04T00-00-00Z` (a community build of MinIO;
the `minio/minio` images are no longer published) and
`fsouza/fake-gcs-server:1.52.2`. CI starts both with `docker run` in the
`database` job: a service container cannot pass the arguments both
need (`server /data`, `-scheme http`).

## 8. Deferred

- Named bindings (two stores in one project).
- A download route that redirects to a signed URL, with response
  overrides for `Content-Disposition` and the media type.
- Streaming through `Storage` and the upload route.
- A metadata index in Postgres, and the backoffice Files tab on it.
- Azure Blob Storage (one more `object_store` feature and module).
- S3 session tokens and assumed roles from settings (the credential
  chain covers them from the environment).
