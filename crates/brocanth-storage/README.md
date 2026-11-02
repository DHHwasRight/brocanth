# brocanth-storage

File storage for brocanth: one `Storage` trait, a local-disk backend, a
Postgres backend behind a feature, content-addressed names, signed download
tokens and derived image variants. An object's address is a 32-byte hash of
its content, so a path cannot contain `..` or a slash, and no filename from a
client reaches the filesystem. The stored media type comes from the bytes, not
from the client. Use this crate to keep uploads. It does not depend on
`brocanth`, has no routes and parses no multipart body: the upload route, the
download route and the policy that guards them belong to the application. It
has no S3 backend. An object store is a separate crate that implements the
same trait.

## Getting started

```toml
[dependencies]
brocanth-storage = { git = "https://github.com/DHHwasRight/brocanth", branch = "main" }
```

| Feature | Default | Adds |
|---|---|---|
| `variants` | off | `derive_variant` and `render`, with the png, jpeg, gif and webp decoders (`image`) |
| `postgres` | off | `Postgres`, objects in `bytea` columns (`sqlx`, `tokio`) |
| `scaffold` | off | the `storage` module for `brocanth create`; only the CLI turns it on |

With no feature, the build has no SQL driver, no async runtime and no image
codec. Leave `variants` off when the application stores documents: it points
four image decoders at bytes from strangers.

As a module:

```sh
brocanth create my-app --with config,storage   # a new project
brocanth add storage                           # an existing project
```

The module writes `src/plugins/storage/mod.rs`, which opens a `LocalDisk`,
keys it with the secret, and puts a `FileStore` (the store and a `FileTokens`)
on the context. It adds `/var/uploads` to `.gitignore`. It writes no upload
route and no download route.

Settings, section `[storage]`:

| Key | Kind | Default | Flag | Environment |
|---|---|---|---|---|
| `root` | text | `var/uploads` | `--storage.root` | `$<APP>_STORAGE__ROOT` |
| `secret` | text | a development-only string | none: secret | `$<APP>_STORAGE__SECRET` |
| `token_ttl` | integer | `120` | `--storage.token_ttl` | `$<APP>_STORAGE__TOKEN_TTL` |

`secret` signs file tokens and keys object addresses. It has no flag, because
every user on the host can read a process's arguments. Set it in the
environment or, with the `config` module, in `config/<env>.toml`. Replace the
default before the first real upload: while it is the default, anybody who
read the repository can mint a token for any object. See
`docs/module-flags.md`.

## Examples

### Store a file and read it back

```rust
use brocanth_storage::{HashKey, LocalDisk, ObjectInfo};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let dir = tempfile::tempdir()?;
let store = LocalDisk::open(dir.path())?
    .with_hash_key(HashKey::from_secret(b"deployment secret"));

let about = ObjectInfo::new()
    .with_filename("../../etc/passwd")
    .with_declared_type("text/html");
let key = store.put_blocking(b"\x89PNG\r\n\x1a\nnot really", &about)?;

let meta = store.meta_blocking(&key)?;
assert_eq!(meta.content_type.as_str(), "image/png"); // sniffed, not declared
assert_eq!(meta.filename.unwrap().as_str(), "etcpasswd"); // kept, defanged
assert_eq!(key.to_hex().len(), 64); // keep this on the record

// The same bytes have the same address.
assert_eq!(store.put_blocking(b"\x89PNG\r\n\x1a\nnot really", &ObjectInfo::new())?, key);
# Ok(())
# }
```

`HashKey` makes the address an HMAC. Plain SHA-256 also deduplicates, but then
anybody with a copy of a file can ask whether it was uploaded. The `key`
module documentation sets out the trade.

### Sign a download link

A `FileToken` says "the bearer may read this object". It carries no identity,
so mint one only after the policy allowed the request.

```rust
use brocanth_storage::{FileTokenKey, FileTokens, ObjectKey, StorageError};

let tokens = FileTokens::new(FileTokenKey::from_secret(b"deployment secret")).with_ttl(120);
let key = ObjectKey::of(b"some bytes");

let token = tokens.issue_at(&key, 1_000);
assert_eq!(tokens.verify_at(token.as_str(), 1_060).unwrap(), key);

// Expired, forged and malformed are one error.
assert!(matches!(tokens.verify_at(token.as_str(), 1_121), Err(StorageError::InvalidToken)));
assert!(matches!(tokens.verify_at("forged.token", 1_000), Err(StorageError::InvalidToken)));
```

`issue` and `verify` read the system clock. The `_at` forms take the time, for
a test.

### Write code against the trait

A handler holds `Arc<dyn Storage>`, so a change of backend is a change in one
file.

```rust
use std::sync::Arc;
use brocanth_storage::{ObjectInfo, ObjectKey, Storage, StorageError};

async fn upload(
    store: &Arc<dyn Storage>,
    bytes: &[u8],
    sent_name: &str,
) -> Result<ObjectKey, StorageError> {
    let about = ObjectInfo::new().with_filename(sent_name);
    store.put(bytes, &about).await
}

async fn download(store: &Arc<dyn Storage>, key: &ObjectKey) -> Result<(Vec<u8>, bool), StorageError> {
    let meta = store.meta(key).await?;
    // `false` means: serve it as an attachment, never inline.
    let inline = meta.content_type.is_safe_to_inline();
    Ok((store.get(key).await?, inline))
}
# fn main() { let _ = (upload, download); }
```

### A thumbnail specification

`VariantSpec` parses with `variants` off, so a build without codecs can still
serve a variant another process computed (`Storage::get_variant`).

```rust
use brocanth_storage::VariantSpec;

let spec: VariantSpec = "300x200t".parse().unwrap();
assert_eq!(spec.to_string(), "300x200t");
assert!("0x0".parse::<VariantSpec>().is_err());
```

## Testing

```sh
cargo test -p brocanth-storage
cargo test -p brocanth-storage --features variants,postgres
```

The disk tests need nothing. The Postgres backend tests need the `postgres`
feature and a database; without `BROCANTH_TEST_DATABASE_URL` they skip and
say so:

```sh
BROCANTH_TEST_DATABASE_URL=postgres://localhost/brocanth_test \
  cargo test -p brocanth-storage --features postgres
```

The suite creates its own tables in that database.

### Testing an application that uses this crate

Open a `LocalDisk` in a temporary directory (`tempfile::tempdir()`) for each
test. Use `FileTokens::issue_at` and `verify_at` with a fixed time to test
expiry without a sleep. For an upload or download screen, register the store
in the application and drive it with `brocanth-test`'s `TestClient`; assert on
the status and the `Content-Disposition` of the response.

## More

- The crate documentation (`cargo doc -p brocanth-storage --open`): each
  security decision with its reason, the hash trade in `key`, and the token
  format in `token`.
- `docs/module-flags.md`: how `[storage]` is set, and why `secret` has no
  flag.
- `PLAN.md` §4 (why no object store in v1) and §8.2 (a feature that is off
  means the code is absent).
