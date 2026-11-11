# brocanth-audit

Change history for brocanth: who changed which row, when, and what
changed. The history is one table in your own Postgres. Entries older
than `retention_days` are purged once a day.

Two writers feed it:

- the model layer: every `RowChange` it publishes, through a model hook
  or with the trail as the `ChangeSink`;
- the admin (feature `admin`): every create, edit, delete and bulk
  action, with the values before and after.

```rust
use std::sync::Arc;

use brocanth_audit::{Kind, MemoryStore, Trail};
use brocanth_events::{ChangeSink, RowChange};

# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
// Keep 365 days.
let trail = Trail::with_store(Arc::new(MemoryStore::new()), 365);

// A write of the model layer, made by a known person. `by` is a
// `ChangeSink`: hand it to `insert_published` and its friends.
let fields = ["title"];
trail.by("ada").publish(&RowChange::updated("post", 7_i64, fields.as_slice()));

// A write never waits on the history: it is queued, and a read or the
// plugin's task writes the queue.
let history = trail.history("post", "7").await.unwrap();
assert_eq!(history.len(), 1);
assert_eq!(history[0].kind, Kind::Updated);
assert_eq!(history[0].actor.as_deref(), Some("ada"));
assert_eq!(history[0].fields[0].name, "title");
# });
```

## In an application

`brocanth create --with audit` registers `AuditPlugin` after the `db`
plugin and writes `src/plugins/audit/mod.rs`, which holds the function
that names who is asking and a sink for a page:

```rust,ignore
post.insert_published(ctx.orm(), &crate::plugins::audit::sink(ctx)).await?;
```

With `admin`, a History tab at `/admin/history` and a History link on
every record.

## Features

| Feature | Default | Adds |
|---|---|---|
| `runtime` | on | the entries, the trail, the plugin, the memory store |
| `postgres` | on | `PgStore`, over `brocanth-db` |
| `config` | off | `Deserialize` on `AuditConfig`, for `[audit]` |
| `admin` | off | the History tab, the journal and the record link |
| `scaffold` | off | the `audit` module for `brocanth create` |
