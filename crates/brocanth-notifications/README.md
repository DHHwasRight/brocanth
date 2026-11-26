# brocanth-notifications

In-app notifications: a row per notification in the application's
Postgres, an inbox per account, an unread count, mark one or all read.

```rust,ignore
let store: &NotificationStore = ctx.provided().expect("dispatched");
store.notify(&account, &Notice::new("comment", "Ada replied").url("/posts/7")).await?;

let inbox = store.inbox(&account);
let unread = inbox.unread().await?;
let page = inbox.list(None, 20, None).await?;
inbox.mark_read(page[0].id).await?;
```

Every read and every write goes through an `Inbox`, which holds one
account. No method takes a second account or lists across accounts, so a
page cannot show one person another person's notifications by mistake.
`list` also takes a policy `Condition` over `NotificationColumn`, ANDed
into the same `WHERE`.

Under row-level security, `store.as_identity(identity)` runs each call in
one transaction that assumes the identity first, so the database checks
the account too. With `policy` and `rls`, the generated project holds a
`#[db]` policy for the table (`tests/rls.rs` runs the same one on a forced
table).

A generated project gets it with `brocanth add notifications`
(`docs/modules/notifications.md`): a page at `/notifications`, mail with the
`mail` module, a live push with `realtime`.
