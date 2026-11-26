# `notifications`

in-app notifications: an inbox per account at /notifications, unread count, mark read.

The wizard calls it: *In-app notifications, with mail and live push when present*.

| | |
|---|---|
| Categories | **Routes**, Enhancer |
| Wizard question | Behaviour |
| Default | off |
| Presets | none |
| Part of | nothing |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-notifications` (feature `config` with [`config`](config.md), `realtime` with [`realtime`](realtime.md)) |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/notifications.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | [`auth`](auth.md): a notification belongs to an account, and the inbox shows the signed-in one; [`middleware-csrf`](middleware-csrf.md): the inbox marks read with a form post: without the CSRF check any site marks a reader's notifications read |
| After | [`auth`](auth.md), [`middleware-csrf`](middleware-csrf.md), [`auth`](auth.md), [`mail`](mail.md), [`realtime`](realtime.md) |
| Provides | none |
| Requires | `PgPool` (a Postgres pool): the notifications are rows in the database the `db` module opens; providers: [`db`](db.md) |
| Uses | none |
| Adapts to | [`mail`](mail.md): `notify` with an address sends the notification by mail too, through the outbox; [`realtime`](realtime.md): `notify` publishes the new row as a change on `brocanth_notifications`, with its account; [`policy`](policy.md): the inbox page ANDs a policy rule over `NotificationColumn` into its `WHERE`; [`rls`](rls.md): with `policy`, a `#[db]` policy: a reader sees and marks read only their own rows; the inbox runs as the reader and `notify` as the anonymous request |
| Contributes | `ConfigCheck` `notifications`: `deploy/migrate` for `[notifications]`; `AgentsSection` `notifications`: how to notify an account, and who reads it |
| Collects | none |
| Exports | `db_policies` (`crate::plugins::notifications::db_policies`): with `policy` and `rls`, the path of a `fn(&Role) -> RulePolicies`: the database policies of `brocanth_notifications`, for `schema()` |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | `[notifications]`: `migrate`, `per_page` |
<!-- end generated: capabilities -->

## What it does

- **A table.** `brocanth_notifications` in the `[db]` database: the
  account (as text), a kind, a title, a body, a link, when it was stored,
  when it was read. The plugin creates it at the first readiness check
  (`migrate`), or you run `brocanth_notifications::SCHEMA` in a migration.
- **`notify`.** `crate::plugins::notifications::notify(ctx, &account,
  Notice::new(kind, title))` stores one row. With [`mail`](mail.md) it takes
  an address too and sends the notification through the outbox
  (`deliver_later`, queued with [`jobs`](jobs.md)). With
  [`realtime`](realtime.md) it publishes the row as a `created` change on
  `brocanth_notifications`, with its `account` column, so a subscription
  whose rule is `account = <the reader>` receives it.
- **A page.** `GET /notifications` lists the signed-in account's
  notifications, newest first, with the unread count, a "mark read" form
  per unread row and "mark all read". `?before=<id>` is the next page.
  `POST /notifications` with `id=<n>` or `all=1` marks them and redirects
  back. A visitor with no session gets `401`.

## Decisions

- **An inbox holds one account.** Every read and write goes through
  `store.inbox(&account)`, and the account is in the `WHERE` of each
  statement beside the id. A forged id from another account matches
  nothing. The account comes from the session (`account(ctx)`), never from
  a form or the query string.
- **A policy rule narrows, it cannot widen.** With
  [`policy`](policy.md), the page compiles a `Pred` over
  `NotificationColumn` and passes the condition to `list`, which ANDs it
  into the same `WHERE` as the account (`docs/module-capabilities.md`: no
  filter after a paginated load).
- **The database holds the rule too, with [`policy`](policy.md) and
  [`rls`](rls.md).** The module writes `NotificationPolicy`, a `#[db]`
  `policy!` over `brocanth_notifications`, and `db_policies(&role)`,
  which `schema()` calls on its own `.policies(…)` line (exported as
  `db_policies`, so the line has one home). `rls make` then writes
  `SELECT` and `UPDATE` policies that admit a row only when its `account`
  is `brocanth.account`, and forces the table. The policy has a `db { … }`
  clause: `cargo test` runs the agreement test on `[db] url`.
  - **A reader reads and marks only their own rows.** The page runs the
    inbox as `reader(&account)`: the role `app_user` with `account` set
    from the session. A bug that asks for another account's inbox gets no
    row from the database, and an `UPDATE` of another account's row
    touches nothing. `WITH CHECK` refuses to move a row to another
    account.
  - **Any request writes (`create: always`).** A notification tells
    somebody else about an event, so the writer is not the reader. A
    `create` rule on the account would refuse the common case. `notify`
    writes as the anonymous request (`app_user`, no attribute). The
    alternative, a system role with a policy of its own, needs a second
    role, a grant and a policy outside `from_rules`; an `INSERT` cannot
    read a row, so `always` opens nothing a reader can see.
  - **No `RETURNING`.** Postgres checks a returned row against the
    `SELECT` policy, and the writer may not read another account's row.
    `notify` reads the id with `lastval()` in the same transaction, so
    `app_user` needs `USAGE` on `brocanth_notifications_id_seq`
    (`.sequence(…)`).
  - **No admin arm, no `delete`, no write mask.** An inbox is personal.
    A notification is marked read, not removed. A mask needs `read_at` in
    the entity, a `timestamptz`, which needs `brocanth-policy`'s `chrono`
    feature; the reader's `UPDATE` reaches only their own rows anyway.
  - **`account` is a text attribute.** `Visitor` gets `account:
    Option<String>`, the account as `brocanth-auth` stores it. The
    example's `account_id` is an `i64`, and a policy that compares the
    text column with it fails in Postgres.
  - **A store takes an identity.** `NotificationStore::as_identity` runs
    each call in one transaction that assumes it first. Without one, a
    call runs as the login role, which on the forced table sees no row.

  Without `rls`, the module writes no database policy: the account in
  each `WHERE` is the only check.
- **Keyset pages.** A new notification while a reader pages does not
  shift the next page, as an offset would.
- **A link is a path or http(s).** `notify` refuses anything else: the
  page writes it into an `href`.
- **The account is text.** Whatever id `brocanth-auth` uses is stored as
  its text, so the table needs no foreign key into a table it does not
  own.

## What it writes

Files:

- `src/plugins/notifications/mod.rs`: the plugin, `account(ctx)`,
  `store(ctx)` and `notify`. With `policy` and `rls`, also
  `NotificationPolicy` (`#[db]`), `db_policies(&role)` and
  `reader(&account)`. Its tests check that a visitor with no session
  has no inbox and that a mark-read post without the CSRF token is refused.
- `src/pages/notifications.rs`: the inbox and the mark-read post.

Slots:

- src/plugins/mod.rs: `pub mod notifications;`
- src/plugins/mod.rs (the builder chain), the provider list
- src/plugins/rls/mod.rs (with `policy` and `rls`): `schema()` calls
  `crate::plugins::notifications::db_policies(&app_user())`, through the
  `db_policies` export
- src/pages/mod.rs (the route table test): `/notifications`
- config/<env>.toml: `[notifications]`
- `check --deploy`: `deploy/migrate` for `[notifications]`
- AGENTS.md: `## Notifications`

## Settings

| Setting | Default | Meaning |
|---|---|---|
| `migrate` | `true` | Create the table on the first readiness check. |
| `per_page` | `20` | Notifications per page on `/notifications`. |
| `mail` | `true` (only with `mail`) | Send a notification by mail too when `notify` is given an address. |

## Read more

- [`auth`](auth.md), [`mail`](mail.md), [`realtime`](realtime.md), [`policy`](policy.md), [`rls`](rls.md)
- Source: [`crates/brocanth-notifications/src/store.rs`](../../crates/brocanth-notifications/src/store.rs), [`crates/brocanth-notifications/src/scaffold.rs`](../../crates/brocanth-notifications/src/scaffold.rs)
