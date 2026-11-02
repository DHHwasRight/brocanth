# brocanth-queue-conformance

The `brocanth-queue` contract as an executable suite. Nothing here names
Postgres, a connection, a transaction or SQL. Each case declares the
`Capabilities` it needs. When the backend does not declare them, the case
is skipped, not failed. So a backend that supports less is still held to
everything it claims. The suite runs twice: once against the backend, and
once through `Adversary`, a wrapper that uses every freedom the contract
gives. The two results must match. They stop matching when a case assumes
something only a database gives.

Use it in the test suite of every `Backend` and every `Outbox` store. Do
not use it to test an application: it tests storage, not jobs.

## Getting started

```toml
[dev-dependencies]
brocanth-queue-conformance = { git = "https://github.com/DHHwasRight/brocanth.git" }
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

Inside this workspace: `brocanth-queue-conformance.workspace = true`.

The crate has no features and no `scaffold` module. It depends on
`brocanth-queue` and `tokio`.

## Run the suite against a new backend

1. Implement `brocanth_queue::Backend` for your type. Declare in
   `capabilities()` only what the backend really does. The blanket impl
   gives you `DynBackend`.
2. Add a test file to your backend crate, for example
   `tests/conformance.rs`, and call `assert_conformant`. It runs both
   passes. It panics if a case fails or if the plain and hostile passes
   disagree.
3. If the backend also has an outbox store, call `assert_outbox_conformant`
   with a `write` closure. The closure must commit its rows before it
   returns, as the caller's transaction would, and the store must be empty
   when the run starts.
4. Use a multi-thread runtime: `#[tokio::test(flavor = "multi_thread")]`.
   Some cases wait for a lease to expire.
5. For a backend with a server, skip the test when no server is set, as
   `brocanth-queue-postgres/tests/postgres.rs` does. Each run names its
   queues with a prefix unique to the run (`Ctx::queue`), so a second run
   against the same server does not meet the rows of the first.

This is that test file, with `Memory` and `MemoryOutbox` in the place of
your types:

```rust
use std::sync::Arc;

use brocanth_queue::DynBackend;
use brocanth_queue_memory::{Memory, MemoryOutbox};

# async fn the_backend_conforms() {
// #[tokio::test(flavor = "multi_thread")]
// async fn the_backend_conforms() {
let backend: Arc<dyn DynBackend> = Arc::new(Memory::new());
brocanth_queue_conformance::assert_conformant(backend).await;
// }
# }

# async fn the_outbox_conforms() {
// #[tokio::test(flavor = "multi_thread")]
// async fn the_outbox_conforms() {
let store = Arc::new(MemoryOutbox::new());
let writer = Arc::clone(&store);
brocanth_queue_conformance::assert_outbox_conformant("memory", store, |jobs| {
    for job in jobs {
        writer.write(job);
    }
    async { Ok(()) }
})
.await;
// }
# }
# #[tokio::main]
# async fn main() {
#     the_backend_conforms().await;
#     the_outbox_conforms().await;
# }
```

## Examples

### Read the report

`run` returns a `Report`. It prints one line per case, and lists the
failures and the skips.

```rust
use std::sync::Arc;

use brocanth_queue::DynBackend;
use brocanth_queue_conformance::run;
use brocanth_queue_memory::Memory;

# #[tokio::main]
# async fn main() {
let backend: Arc<dyn DynBackend> = Arc::new(Memory::sqs_shaped());
let report = run(backend).await;
println!("{report}");

assert!(report.failures().is_empty(), "{report}");
assert_eq!(
    report.passed() + report.skipped().len(),
    report.outcomes.len()
);
# }
```

### Compare the plain and the hostile pass yourself

`run_both` returns the two reports. `Report::shape` drops the detail, so
two runs that agree on the contract compare equal.

```rust
# use std::sync::Arc;
# use brocanth_queue::DynBackend;
# use brocanth_queue_memory::Memory;
# #[tokio::main]
# async fn main() {
let backend: Arc<dyn DynBackend> = Arc::new(Memory::new());
let (plain, hostile) = brocanth_queue_conformance::run_both(backend).await;
assert_eq!(plain.shape(), hostile.shape());
# }
```

### Run every capability gate at least once

A case that every backend skips proves nothing. The reference backend runs
the suite under two capability sets, so each case runs at least once. A new
gate must keep this true (see `tests/memory.rs`).

## Testing

```sh
cargo test -p brocanth-queue-conformance
```

No server and no environment variable. The suite runs against
`brocanth-queue-memory`, unbounded and SQS-shaped, and the outbox suite
runs against `MemoryOutbox`. The Postgres run of the same suite is in
`brocanth-queue-postgres` and needs `BROCANTH_QUEUE_TEST_DATABASE_URL` (see
that crate's README). To print every backend's report:

```sh
cargo run -p brocanth-queue-postgres --example report
```

## Links

- `brocanth-queue` README: the contract the suite encodes.
- [`../brocanth-queue/docs/DESIGN.md`](../brocanth-queue/docs/DESIGN.md):
  what SQS and JetStream cannot do, which is what the capability gates
  read.
- [`../brocanth-queue/docs/DECISIONS.md`](../brocanth-queue/docs/DECISIONS.md),
  Part 4 and "Verified": the outbox cases and the last recorded results.
