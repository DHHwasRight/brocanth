# Where accounts live

`brocanth-auth` keeps no state of its own. It calls a store through five
traits in `src/storage.rs`:

| Trait | For |
|---|---|
| `AuthStore` | Accounts, provider links, sessions, single-use tokens. Every application needs it. |
| `AccountDirectory` | Listing accounts and their sessions, for the backoffice. |
| `MfaStore` | TOTP secrets and recovery codes. |
| `OtpStore` | One-time-code challenges. |
| `SignInLocationStore` | The networks an account signed in from, for login alerts. |

Two stores ship. Both implement all five traits.

| Store | Feature | Survives a restart |
|---|---|---|
| `MemoryStore` | none | No. It is a `Mutex<HashMap>`. |
| `PgStore` | `postgres` (off by default) | Yes. |

## Use `PgStore`

```toml
brocanth-auth = { workspace = true, features = ["postgres"] }
```

```rust
let store = PgStore::connect(&database_url).await?; // or PgStore::from_pool(pool)
store.migrate().await?;
let store = Arc::new(store);
```

`migrate` is idempotent and takes an advisory lock, so every instance can
run it at boot. If your schema lives in reviewed migration files, copy
`PgStore::schema_sql()` into one and do not call `migrate`. The tables are
named `brocanth_auth_*`.

`from_pool` takes the `sqlx` 0.9 pool that the models, the queue and the
cache already use. Use one pool, not four.

Run `store.sweep(now, session_config.idle_timeout)` from a recurring job. It
deletes dead sessions, expired tokens and expired one-time-code challenges.
Correctness does not depend on it: every service checks expiry when it reads
a row. The sweep only keeps the tables small.

## What a row holds

- A single-use token: the selector and the SHA-256 of the verifier. The
  secret half never reaches the store.
- A recovery code or a login-alert network: a digest.
- A one-time code: an argon2id hash.
- A TOTP secret: the secret. TOTP is symmetric, so the server needs it.
- A session: its id, as `MemoryStore` keeps it. The trait gives the id back
  (`Sessions::end_impersonation` seals a new cookie for the operator's
  session), so a digest cannot replace it. A cookie is the id plus an HMAC
  under the `SigningKey`, and the key is not in the database.

The email is unique on `lower(email)`. Accounts without an email do not
collide.

## The five atomic operations

`src/storage.rs` names five operations that must be atomic. `PgStore` does
each in one statement or one transaction:

| Operation | SQL |
|---|---|
| `consume_token` | `DELETE ... RETURNING` |
| `create_account` (unique email) | a unique index |
| `claim_otp_attempt` | `UPDATE ... RETURNING`, then a conditional `DELETE`, in one transaction |
| `claim_totp_step` | `UPDATE ... WHERE last_step < $2`, and the row count |
| `consume_recovery_code` | `DELETE`, and the row count |

## The conformance suite

The `conformance` feature turns on `brocanth_auth::conformance`. It has one
macro per trait. Each macro writes one `#[tokio::test]` per case. The cases
include races: 16 callers at once on one token, one TOTP step, one recovery
code, one OTP challenge, one email and one new network. Exactly the right
number must win.

`tests/memory_store.rs` and `tests/postgres_store.rs` run the same cases. To
test your own adapter:

```rust
async fn fresh() -> Option<Arc<MyStore>> { Some(Arc::new(MyStore::new())) }

brocanth_auth::auth_store_conformance!(fresh);
brocanth_auth::mfa_store_conformance!(fresh);
```

`fresh` can return a `conformance::Fixture` to clean up after each case. The
Postgres tests use one to drop the schema that each case ran in.

Run the Postgres cases with a database:

```sh
BROCANTH_AUTH_TEST_POSTGRES=postgres://localhost/brocanth_auth_test \
  cargo test -p brocanth-auth --all-features
```

`BROCANTH_TEST_DATABASE_URL` also works. Set
`BROCANTH_AUTH_REQUIRE_POSTGRES=1` to fail, not skip, when no database is
set.

## A generated project

The `auth` module that `brocanth create` writes still says
`type Store = MemoryStore`. To move it to `PgStore`:

1. Turn on the `postgres` feature of `brocanth-auth` in the manifest.
2. Change the alias to `type Store = PgStore`.
3. Move the build from `configure` to `before_start`. Opening a pool is
   async and `Plugin::new` is not. The generated `cache` module shows the
   pattern: take the runtime handle and block on a scoped thread.
4. Read the database URL from an environment variable named in `Settings`,
   or take the pool that the model layer opened.
5. Call `migrate()` before `Auth::build`, or copy `PgStore::schema_sql()`
   into a migration.
6. Schedule `sweep` as a recurring job if the project has the `jobs` module.
