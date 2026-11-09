# brocanth-flags

Feature flags for brocanth. A flag is on for everyone, for a percentage of
signed-in users, for a list of users, or for a list of groups. The rules
are rows in your own Postgres. A page reads them from a copy in memory, so
a flag costs a map lookup and never waits on the database.

```rust
use std::sync::Arc;

use brocanth_flags::{Actor, Flag, Flags, MemoryStore, Rule};

// The flags the code reads, declared once. A page asks with the constant,
// so a misspelt flag does not compile.
const NEW_CHECKOUT: Flag = Flag::new("new_checkout", "The checkout flow from the redesign.");

# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
let flags = Flags::with_store(Arc::new(MemoryStore::new()));
let ada = Actor::new("ada").in_group("staff");

// Off until somebody turns it on.
assert!(!flags.enabled(NEW_CHECKOUT, &ada));

// On for the staff group: in the store, and in this process's copy at once.
flags
    .set(
        NEW_CHECKOUT.key(),
        Rule { groups: vec!["staff".into()], ..Rule::default() },
    )
    .await
    .unwrap();
assert!(flags.enabled(NEW_CHECKOUT, &ada));
assert!(!flags.enabled(NEW_CHECKOUT, &Actor::new("bob")));

// A quarter of signed-in users. The same person stays in or out.
flags.set(NEW_CHECKOUT.key(), Rule::percent(25)).await.unwrap();
let bob = Actor::new("bob");
assert_eq!(flags.enabled(NEW_CHECKOUT, &bob), flags.enabled(NEW_CHECKOUT, &bob));
# });
```

## In an application

`brocanth create --with flags` registers `FlagsPlugin` after the `db`
plugin and writes `src/plugins/flags/mod.rs`, which holds the list of
flags and the function a page calls:

```rust,ignore
if crate::plugins::flags::enabled(ctx, crate::plugins::flags::NEW_CHECKOUT) {
    // the new flow
}
```

The plugin creates the `brocanth_flags` table on the first readiness
check, reads it before the server listens, and reads it again every
`refresh_secs`. With `admin`, the tab at `/admin/flags` turns each flag.

## Features

| Feature | Default | Adds |
|---|---|---|
| `runtime` | on | the rules, the copy, the plugin, the memory store |
| `postgres` | on | `PgStore`, over `brocanth-db` |
| `config` | off | `Deserialize` on `FlagsConfig`, for `[flags]` |
| `admin` | off | `admin::FlagsPanel` |
| `scaffold` | off | the `flags` module for `brocanth create` |
