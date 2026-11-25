# brocanth-backoffice

The backoffice as a service of its own. The design is in
`docs/backoffice.md` §7.

**The tab data contract.** The admin's model screens reach their rows
through `TabData`: one `Call` of owned serde types, with the caller's
`Claim` (subject, superuser, grants) on each. `InProcess` is the data side
over any `AdminData`. `Contracted` is the admin's side, an `AdminData` over
any `TabData`. `Json` puts a JSON codec between the two, so a test proves
that every call crosses a wire.

```rust
AdminConfig::new()
    .data_through(|data| Contracted::new(InProcess::new(data)).shared())
```

The data side gives `Principal::new(&claim)` to the source's
`for_request`, so a source with `as_identity` runs each call as the staff
member's database identity. `InProcess::enforcing_grants()` refuses a call
whose claim does not hold `<tab>.<level>`; a server that takes calls from
another process must turn it on.

**The wire** (feature `wire`, on by default). `WireServer` answers the
contract at `POST /_backoffice/wire` in the application; `WireClient` is a
`TabData` over HTTP for the backoffice binary. Each request is signed
with HMAC-SHA256 over a shared service secret, with a time window and a
nonce; the server always checks grants and holds transactions between
requests with a timeout and a cap. `docs/backoffice.md` §7.5.

**The second binary.** The `backoffice` module (feature `scaffold`) writes
`src/bin/backoffice.rs`, which runs the application itself and serves only
the admin's mount, at `[backoffice] addr`. `Gate` keeps each binary to its
own paths. `[backoffice] embedded = false` takes the admin out of the
application binary.

```text
cargo run --bin backoffice -- --backoffice.addr=127.0.0.1:3001
cargo run -- --backoffice.embedded=false
```

Deferred: the wire transport, data traits for the panels, sign-in pages in
the second binary, an image for it.
