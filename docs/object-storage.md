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

The four are children of `storage` (`part_of_but_off`), as
`cache-redis` is of `cache`; two of them in one project are refused,
because `storage` itself provides no store. The backend modules write no
file. `storage` asks
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
- `brocanth create <name> --with storage,storage-s3`: the requirement is met, so
  nothing is added.

### 3.4 Projects from before

`add` and `remove` read `[modules] enabled`. When it holds `storage` and
no provider of `ObjectStore`, the CLI reads `storage-db` if `db` is
there, else `storage-fs`. The generated storage file for each holds the
same open code the project already has (`storage-db` byte for byte,
`storage-fs` with one comment line changed), so the files stay where they
are.

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

Variables: `BROCANTH_STORAGE_TEST_S3`
(`http://key:secret@host:port/bucket`; the bucket must exist) and
`BROCANTH_STORAGE_TEST_GCS` (`http://host:port/bucket`; the run makes the
bucket on the emulator). Each run works under a prefix of its own, so
parallel runs share a bucket and delete only what they wrote. A skip
prints a banner past libtest's capture, as the Postgres tests do. The S3
and GCS runs also check the start probe: a missing bucket, a wrong secret
and an unreachable server each fail with a `help:` line.

Images, pinned by digest:
`pgsty/minio:RELEASE.2026-08-04T00-00-00Z` (a community build of MinIO;
the `minio/minio` images are no longer published) and
`tustvold/fake-gcs-server` (the fork the `object_store` project tests
against: upstream `fsouza/fake-gcs-server`, 1.52.2 and the 2026-08 build,
sends an XML-API PUT to its JSON upload handler). CI starts both with
`docker run` in the `database` job: a service container cannot pass the
arguments both need (`server /data`, `-scheme http`).

What the emulator cannot show, and so only S3 proves: a create-only put
refused on a taken path (the emulator ignores `ifGenerationMatch=0`), and
an upload in parts (it has no XML multipart API). Real GCS has both.

## 8. Deferred

- Named bindings (two stores in one project).
- A download route that redirects to a signed URL, with response
  overrides for `Content-Disposition` and the media type.
- Streaming through `Storage` and the upload route.
- A metadata index in Postgres, and the backoffice Files tab on it.
- Azure Blob Storage (one more `object_store` feature and module).
- S3 session tokens and assumed roles from settings (the credential
  chain covers them from the environment).

## 9. Named stores and signed downloads

Status: accepted (2026-11-27). The owner took every recommended answer
in §9.7. It reverses D6 and D16. §0 to §8 stay as the record of v1.

Two items from §8:

1. Named bindings: two or more stores in one project, for example private
   uploads on S3 and public assets on a disk or in a second bucket.
2. A download route that redirects to a signed URL, with response
   overrides for `Content-Disposition` and the media type.

### 9.0 Decisions, one line each

| # | Decision | Reason |
|---|---|---|
| D19 | A project has one **default** store and zero or more **named** stores. `ObjectStore` keeps cardinality one for the default. Replaces D6. | D6 blocked a second store because the routes, the tokens and the thumbnails read one store. D25 to D27 give each of them one store per binding. The default path stays the path D6 describes. |
| D20 | A Kind accepts names when `kind!` says `named`. Only `ObjectStore` does. A named binding never meets a `requires`, never counts in the two-provider conflict, and goes in `brocanth.toml` under `[named_bindings]`. | `explain`, `check --deploy` and the feature merge must see each store, so the resolver records it. No other Kind needs names, so this is not a general "many" cardinality. |
| D21 | CLI: `brocanth add storage-s3@public`, `brocanth remove storage-s3@public`, `create --with storage,storage-s3@public`. `add storage-s3` without `@` still swaps the default (D8 unchanged). | One command each way, as for the default. The provider module enters `[modules] enabled` once, so its crate feature (`s3`, `gcs`, `postgres`) is merged as today. |
| D22 | A name matches `[a-z][a-z0-9_]{0,23}`. It is not `default` and not a key of `[storage]` or `[storage_routes]`. One binding per name. `storage-db` backs at most one binding. | The name is a TOML table, a flag segment, a Rust field and a URL segment. A name equal to a setting key would make one TOML key mean two things. `storage-db` has fixed table names, so two bindings would share rows. |
| D23 | The default store keeps `[storage]`. A named store reads `[storage.<name>]` and `--storage.<name>.<key>=…`. `secret` and `token_ttl` stay in `[storage]` only. | The config layer and the flag parser already take nested tables (`brocanth-config` `given_layer`, `brocanth-scaffold` `config_section`). One plugin file and one section, as D11 wants. |
| D24 | Two stores whose locations overlap fail the start: the same disk root, or the same endpoint, bucket and prefix. A named cloud store has no default bucket. | Two bindings on one location delete each other's variants and objects. A shared default (`uploads`) would make that the zero-config case. |
| D25 | Code: `FileStore` (the default) does not change. A project with names also gets `NamedStores` on the context and in the services, with one typed field per name, each `{ store, tokens }`. | A typo in a store name fails at compile time. A one-store project keeps the same files. `Services` is keyed by type, so a second `Arc<dyn Storage>` cannot go in beside the first. |
| D26 | A named store's token key derives from `[storage] secret` with the store name in the domain separator. The default store's derivation does not change. | A token then opens one address in one store. The token format does not change, so tokens minted before stay valid, and so do the goldens. |
| D27 | `storage-routes` serves the default store at `path` and each named store at `path/<name>`, with `[storage_routes.<name>]` for `path`, `public`, `max_bytes` and `downloads`. Each store has its own upload guard function. Thumbnails and variants live in the store of their object. | An address is 64 hex characters, so `path/<name>` cannot collide with `path/<address>`. A public assets store and a private uploads store need different `public` and guard answers. |
| D28 | A signed redirect is opt-in per store: `downloads = "proxy"` (default) or `"redirect"`. Replaces D16 for a store that opts in. | A redirect needs a private bucket, a CSP change and a key that can sign. Proxy needs none of them and stays the safe default. |
| D29 | What a redirect carries: a sniffed `image/png`, `image/jpeg`, `image/gif` or `image/webp` goes inline, with `response-content-type` set to the sniffed type. Every other object goes with `response-content-disposition: attachment; filename…` and `response-content-type: application/octet-stream`. The overrides are inside the signature. | D16's reason was that a redirect loses the headers that make a hostile upload harmless. `object_store` 0.14.2 signs response overrides (§9.2), and the bearer cannot remove them, because the signature then fails. §9.4 says what is lost and why the rule covers it. |
| D30 | `HEAD`, thumbnails and a request with `&fmt=` keep proxying. | A URL signed for `GET` refuses `HEAD`, and `meta` answers it locally at no cost. A thumbnail is small, derived by this process and already cached. |
| D31 | `Storage::signed_url` applies the D29 rule every time. A cloud `put` of an object that is not a safe image also stores `Content-Disposition: attachment` on the object. | Defence in depth: any URL that reaches the object, with overrides or not, downloads and does not render. Today `signed_url` hands out a URL to an object whose stored type can be `text/html`, because `ContentType::resolve` keeps a declared type that is not an image. |
| D32 | `signed_ttl`: default 300 s, from 1 s to 7 days. The redirect is `307`, with `Cache-Control: private, max-age=<min(token time left, signed_ttl / 2)>` and `Referrer-Policy: no-referrer`. | A browser must not keep a redirect past the life of its target. The signed URL is a bearer credential that cannot be revoked, like a file token, so its TTL is the leak window. |
| D33 | A disk or Postgres store cannot sign. `downloads = "redirect"` on one fails `configure` with a `help:` line. | The backend is fixed at build time, so the setting is a mistake, not a fallback. A silent proxy would hide it until production. |
| D34 | The start check of a redirecting store signs one URL. With `middleware-security-headers`, the start fails when `img-src` does not hold the bucket origin, and the `help:` line names the value to add. | Otherwise the first image fails in the browser. A GCS store with no key signs through the IAM `signBlob` call, and the start check finds a missing permission there. |
| D35 | The `object_store` requirement becomes `0.14.2`. | `Signer::signed_url_opts` and `SignedUrlOptions` are in 0.14.2 (§9.2). The current requirement `0.14` allows an earlier release that this design did not check. |

### 9.1 Named stores

**Declaration.** A named binding is a provider module and a name:

```text
brocanth add storage-s3@public      # adds the binding; storage-s3 enters [modules] once
brocanth remove storage-s3@public   # removes the binding, its config section, and the
                                    # module when no other binding uses it
brocanth add storage-s3             # unchanged: swaps the default (D8)
```

`brocanth.toml` keeps `[bindings]` for the default and adds one table:

```toml
[bindings]
ObjectStore = { provider = "storage-fs", why = "storage requires ObjectStore" }

[named_bindings]
ObjectStore = { public = "storage-s3", archive = "storage-gcs" }
```

The resolver rule: the provider in `[bindings]` is the default. Each other
provider of `ObjectStore` in `[modules] enabled` must appear in
`[named_bindings]`, or it is the two-provider conflict of today.
`Project` gains `named::<K>()`, a list of name and provider. A project
without `[named_bindings]` resolves as before.

Rejected: the module added twice, as two instances. A module is a crate
feature, a set of files and Slot lines. Instances would need every Slot
and every file path to know an instance name, for one Kind.

**Config.**

```toml
[storage]
root = "var/uploads"          # the default store, unchanged
secret = "…"
token_ttl = 120

[storage.public]
bucket = "assets"
region = "eu-west-1"
secret_access_key_file = "/run/secrets/assets-key"
```

A named disk store defaults to `root = "var/uploads-<name>"`. A named
cloud store has no default bucket (D24). `remove` takes the section out of
`config/<env>.toml` when it holds the generated values, and stops on an
edited section, as it does for a module today.

**Generated code.** `src/plugins/storage/mod.rs` stays byte for byte the
same for a one-store project. With names, the generator adds
`src/plugins/storage/stores.rs` and the lines in `mod.rs` that open it:

```rust
pub struct NamedStores {
    pub public: NamedStore,   // one field per name
}
pub struct NamedStore {
    pub store: Arc<dyn Storage>,
    pub tokens: FileTokens,
}
```

A handler reads `ctx.provided::<NamedStores, _>()?.public.store`.

**Library.** `FileTokenKey::from_secret_for(secret, name)` (D26). A
`StoreName` type holds the name rule (D22). `FileRoutes` does not change:
`storage-routes` builds one `FileRoutes` per store, each with its own
store, tokens, path and guard.

**Who sees it.**

- `brocanth explain`: `ObjectStore <- storage-fs (default)` and
  `ObjectStore[public] <- storage-s3`.
- `check --deploy`: `deploy/storage-local` runs per store and names it.
- `image`: `src()` and `srcset()` stay for the default store. A named
  store gets `src_in(ctx, "public", …)` in phase 4.
- Backoffice: the Files tab is still blocked on an index (§6). When it
  arrives, it shows one store per tab, so RBAC can grant each one.

### 9.2 What `object_store` 0.14.2 signs

Read in `object_store-0.14.2`, the version in `Cargo.lock`:

| Fact | Evidence |
|---|---|
| `SignedUrlOptions` holds `extra_query` (signed query pairs) and `signed_headers`. | `src/signer.rs:47-58` |
| `Signer::signed_url_opts` exists. The default answers `NotSupported` for options that are not empty, so a backend never drops an override in silence. | `src/signer.rs:115-130` |
| S3 implements it. Only names that start with `x-amz-` are refused. | `src/aws/mod.rs:189-203`, `src/util.rs:356-390` |
| S3 appends the extra pairs before it computes the canonical query, so they are signed. | `src/aws/credential.rs:327` |
| The crate's own S3 test signs a GET with `response-content-disposition` and checks that the server echoes it. | `src/aws/mod.rs:1077-1117` |
| GCS implements it (V4). Only `x-goog-` names are refused. The TTL is at most 7 days. | `src/gcp/mod.rs:350-372` |
| GCS appends the extra pairs before signing. | `src/gcp/credential.rs:791` |
| GCS with no private key signs through the IAM `signBlob` call: one network call per URL. | `src/gcp/credential.rs:813` |
| The signing date is private (`date: Option<…>`, no public setter), so two URLs for one object are never equal. | `src/aws/credential.rs:132`, `src/gcp/credential.rs:751` |
| `Attribute::ContentDisposition` is sent on put for S3 and GCS. | `src/attributes.rs:29`, `src/aws/client.rs:380`, `src/gcp/client.rs:200` |

So the overrides need no signer of our own and no upstream change.
`response-content-disposition` and `response-content-type` are response
overrides on S3 and on the GCS XML API. The §4 note "no per-request
response headers on a signed URL" is not true of 0.14.2.

`CloudBucket` adds its prefix itself (`full()`) and holds
`Arc<dyn Signer>`. So `Bucket::signed_url` gains a `SignedGet` argument
(disposition and type) and calls `signed_url_opts`. `FsBucket` still
answers `None`.

Fallbacks, if a later release removes the API: for S3, sign the URL
ourselves (SigV4 query signing is about 150 lines, and the credential
provider of `object_store` is public); for GCS, sign only with a
service-account key and proxy otherwise. Neither is needed now.

### 9.3 The redirect

```text
GET /files/<address>?token=…
  token checked as today -> meta read -> rule D29 -> signed URL
  307 Location: https://<bucket origin>/…&response-content-disposition=…&X-Amz-Signature=…
      Cache-Control: private, max-age=<min(token time left, signed_ttl / 2)>
      Referrer-Policy: no-referrer
```

- **Settings.** In `[storage_routes]` (or `[storage_routes.<name>]`):
  `downloads` and `signed_ttl`. The generator writes them only for a
  cloud backend, so the disk and Postgres goldens do not change.
- **Range.** The bucket answers `Range` itself. Today the route reads the
  whole object into memory; a redirect removes that cost for large files.
- **Caching.** The browser caches the bucket's answer under the signed
  URL. Each redirect mints a new URL (§9.2), so a second page view
  downloads the object again. An in-process cache of signed URLs, for
  `signed_ttl / 2` per store, address and disposition, gives the same URL
  back and also saves the GCS `signBlob` call (open question 5).
- **Public routes.** `public = true` with `redirect` still signs. A
  public bucket URL with no signature is a later item.
- **Disk and Postgres.** They proxy, and `redirect` on them fails the
  start (D33).
- **`check --deploy`.** New warnings: `redirect` with an `http://`
  endpoint (the bearer URL travels in clear text), `signed_ttl` above
  3600 s, and an endpoint on the application's own registrable domain
  (§9.4).

### 9.4 What the bucket cannot send, and why the rule covers it

A signed URL carries response overrides for a few headers only. S3 has
`response-content-type`, `-content-disposition`, `-cache-control`,
`-content-language`, `-content-encoding` and `-expires`. The GCS XML API
has `response-content-type` and `response-content-disposition`. Neither
can send `X-Content-Type-Options: nosniff` or `Content-Security-Policy`.

| Header | Proxy | Redirect |
|---|---|---|
| `Content-Type` | sniffed image, else `octet-stream` | the same, by override |
| `Content-Disposition` | `attachment` for a non-image | the same, by override, and stored on the object (D31) |
| `X-Content-Type-Options: nosniff` | yes | lost |
| `Content-Security-Policy: sandbox` | yes, for a non-image | lost |
| `Cache-Control` | `private, max-age=<token ttl>` | S3 by override; GCS as the bucket stores it |

What this means for a hostile upload:

- **HTML, SVG, XML, PDF with script.** None of them is a safe image, so
  the URL forces `attachment` and `octet-stream`. A browser downloads an
  attachment and does not render it. `nosniff` matters only for a body
  the browser renders, and `sandbox` matters only when the attachment
  header is lost. A proxy or a client can lose a header. Here the header
  comes from the signed query: a URL without it has no valid signature.
- **A polyglot that starts with real image bytes.** The type is the
  sniffed image type, so the browser renders it as an image. A browser
  does not sniff an `image/*` response into HTML. This is the answer the
  proxy gives today, without `nosniff`.
- **The origin.** The body comes from the bucket's origin, not the
  application's. Script there, if it ever ran, has no access to the
  application's cookies or pages. The residual risk is a custom endpoint
  on a sibling of the application's domain (`files.example.com`): a page
  rendered there could set cookies for the parent domain. The rule stops
  the render, and `check --deploy` warns about such an endpoint.
- **SVG stays an attachment.** It is not a safe image today, and the
  redirect does not change that.

The bucket must be private. A public bucket serves every object with its
stored headers to anyone who has the address. D31 makes that a download
for new objects, but the address is then the only secret. `check
--deploy` cannot see a bucket policy, so the AGENTS section says it.

### 9.5 Tests

| Suite | Adds | Proven by |
|---|---|---|
| `Bucket` conformance | signed GET with overrides: status 200, both headers echoed | MinIO |
| | the same URL with `response-content-disposition` removed: 403 | MinIO |
| | a TTL of 1 s, read after 2 s: 403 | MinIO |
| | a `HEAD` on a GET URL: 403 (the reason for D30) | MinIO |
| | the URL holds both overrides and `X-Goog-SignedHeaders` | fake-gcs-server, by reading the query |
| | `FsBucket`: `None` | always |
| `Storage` conformance | `signed_url` of a non-image holds `attachment` and `octet-stream`; of a PNG, the PNG type | disk (`None`), MinIO, fake-gcs-server |
| | a cloud `put` of a non-image stores `Content-Disposition: attachment` | MinIO, fake-gcs-server (`head`) |
| routes (`brocanth-test`) | a PNG redirects inline; HTML and SVG redirect as attachments; `HEAD` and `?thumb=` proxy; the cache and referrer headers; `redirect` on a disk store refused at start | always, with a signing test double |
| named stores | a token of one store is 403 on another; the default token key matches a fixed vector; overlap refused; variants in their own store; a delete in one store leaves the other | always (two disk stores) |
| CLI | resolver tests for `@name`; every current golden unchanged; new shapes `storage-named` (disk default, `storage-s3@public`) and `storage-s3-redirect` | always; a generated run of the two new shapes |

What the emulators cannot prove:

- fake-gcs-server does not check V4 signatures or expiry, and it does not
  echo the response overrides (checked on the pinned image, 2026-11-27).
  The GCS test checks the URL and the bytes, and asserts the headers only
  when the emulator sends them. A real bucket proves the rest, once by
  hand, before the owner accepts phase 2.
- fake-gcs-server drops `Content-Disposition` from an XML-API put, though
  `object_store` sends it. So D31 is proven on MinIO only; the GCS run
  skips that row.
- The `signBlob` path needs real IAM.
- MinIO proves SigV4 and the overrides. It does not prove AWS
  virtual-hosted URLs per region, or a custom endpoint.
- No emulator proves what a browser does with CSP and a redirect. The
  route test checks the headers; a browser test is out of scope.

### 9.6 Phases

| Phase | Work | Hours |
|---|---|---|
| 1 | Library: `SignedGet`, `Bucket::signed_url` over `signed_url_opts`, rule D29 in `Storage::signed_url`, `Content-Disposition` on put (D31), `object_store` 0.14.2, conformance rows. **Done 2026-11-27.** | 8-10 |
| 2 | Redirect: `downloads` and `signed_ttl` in `FileRoutes` and in the routes generator (cloud only), D30, D32, D33, start check D34, `check --deploy` rules, golden `storage-s3-redirect`. **Done 2026-11-27.** | 10-12 |
| 3 | Resolver and CLI: `named` Kinds, `[named_bindings]`, `@name` in `add`, `remove`, `create`, `explain`, `check --deploy` per store | 14-18 |
| 4 | Generator and library for names: `[storage.<name>]`, `NamedStores`, token keys per store, overlap check, routes and guards per store, `image::src_in`, golden `storage-named` | 14-18 |
| 5 | Docs: this section into §0, ADR 14, `docs/modules/storage*.md`, AGENTS sections | 3 |
| | Total | 49-61 |

Phases 1 and 2 do not depend on phases 3 and 4. They ship first.

**How phases 1 and 2 were built.** The decisions above hold. These are
the smaller choices the code made, each with its reason:

- `Storage` gains `can_sign` (sync) and `signing_origin` (async).
  `FileRoutes::new` reads `can_sign` for D33, because `configure` cannot
  await. `signing_origin` signs one URL for an address nothing is stored
  at, with the attachment overrides, and answers its origin for D34.
  `Storage::signed_url` reads the object's metadata first (one `head`), so
  a missing object is `NotFound` and not a URL to a 404.
- The signed-URL cache lives in `FileRoutes`, keyed by address. An address
  has one disposition, because the first upload owns the metadata, so
  the address is the whole key. A cached URL is served for at most
  `signed_ttl / 2`, and the `307` is cached for at most the same, so a
  browser never follows a dead URL. At 4096 entries the expired ones go.
- The start check (D34) runs as a readiness check of `storage_routes`,
  whatever `[storage] probe` says. It reads the policy that
  `SecurityHeaders` now publishes on the services
  (`SecurityHeadersConfig::enforced_csp`: none when the policy is off or
  report-only). After `img-src` (or `default-src`) the source path is not
  compared, as CSP 3 says for a redirect. Once it passed, it signs no
  more: `/readyz` runs it on each probe, and GCS with no key would call
  `signBlob` each time.
- `check --deploy` reads the site's own address from `[meta] base_url`,
  `[sitemap] base_url` or `[feed] site_url`; with none set, it cannot
  compare and says nothing. The registrable domain is the last two labels,
  or three under a country code with `co`, `com`, `net`, `org`, `ac`,
  `gov` or `edu`. No public suffix list: a miss is one warning too many,
  never one too few for a shared `example.com`.
- The generator writes `downloads` and `signed_ttl` only with
  `storage-s3` or `storage-gcs`, so every earlier golden is unchanged. The
  shape `storage-s3-redirect` writes `downloads = "proxy"` (answer 2) and
  the start check.

### 9.7 Questions answered by the owner

The owner accepted each recommended answer below (2026-11-27).

1. **Where a named store's settings live.** Recommended: `[storage.<name>]`
   and `--storage.<name>.bucket`. Trade-off: a future `[storage]` key can
   collide with a name that a project already uses; the CLI then refuses
   to regenerate and asks for a rename. `[storage.stores.<name>]` has no
   collision and costs a longer flag.
2. **Redirect by default for a cloud store.** Recommended: no, opt-in.
   Trade-off: until a project opts in, every download goes through the
   application, and the whole object (64 MiB at most) sits in memory.
3. **Redirect a non-image, or proxy it.** Recommended: redirect it as a
   forced attachment. Trade-off: it loses `nosniff` and `sandbox`, and
   relies on the signed `Content-Disposition`. The careful choice is a
   third value, `redirect-images`, which proxies every non-image and
   gives up the large-file gain for PDFs and videos.
4. **CSP `img-src` for the bucket origin.** Recommended: refuse the start
   and name the value to add. Trade-off: one manual step. The alternative
   is that `storage` writes into the security headers config: a write
   from one module into the section of another.
5. **An in-process cache of signed URLs.** Recommended: yes, for
   `signed_ttl / 2`. Trade-off: a little memory and one more part. In
   return, the browser cache works across page views, and GCS without a
   key makes one `signBlob` call per object per window, not per view.
6. **One secret for all stores.** Recommended: yes, with the name in the
   domain separator (D26). Trade-off: a rotation invalidates the tokens
   of every store at once. A secret per store adds a setting per store,
   and one more secret to forget.
7. **`storage-db` as a named store.** Recommended: at most one `storage-db`
   binding, default or named. Trade-off: no two Postgres stores. A table
   prefix per binding would lift that, with a migration per name.
8. **`Content-Disposition` stored on new cloud objects.** Recommended: yes,
   for new puts, with no backfill. Trade-off: objects stored before keep
   no stored disposition. The signed override still covers them on the
   redirect, but not a URL that application code builds by hand.
9. **The path of a named store's routes.** Recommended: `path/<name>` by
   default, and `[storage_routes.<name>] path` to move it. Trade-off: the
   name is then part of a public URL, so a rename breaks links already
   sent.
