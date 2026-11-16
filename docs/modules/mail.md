# `mail`

send mail through SMTP, or to the log or a folder in development

The wizard calls it: *Send mail*.

| | |
|---|---|
| Categories | **Enhancer** |
| Wizard question | Behaviour |
| Default | off |
| Presets | none |
| Part of | nothing |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-mail` (features `smtp`, `serde`) |
| Without it | no mail; a password reset or a verification link cannot leave the server |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/mail.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | none |
| Requires | none |
| Uses | `JobQueue` (a job queue): `deliver_later` puts the message on the `mail` queue and a worker sends it; `PgPool` (a Postgres pool): a queued message waits in a Postgres table; the test that starts the application is left out |
| Adapts to | none |
| Contributes | `AgentsSection` `mail`: how a page sends mail, and never through `Smtp` itself |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | `[mail]`: `transport`, `from`, `from_name`, `folder`, `smtp_host`, `smtp_port`, `smtp_security`, `smtp_username`, `smtp_password` (secret) |
<!-- end generated: capabilities -->

## What it writes

Files:

- `src/plugins/mail/mod.rs`: the plugin, and the `Outbox` a page sends
  through. With `jobs`, also the `DeliverMail` job (its payload is the
  id of a pending message) and a worker for the `mail` queue. Its tests
  start the plugin with the default settings and with each transport and
  security, so a default that cannot start fails
  `cargo test`, not a deploy. Without `jobs`, a test also builds the whole
  application.

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs: `pub mod mail;`
- src/plugins/mod.rs (the provider list): `mail crate::plugins::mail::MailPlugin`
- config/<env>.toml: `[mail]`
- .gitignore: `/tmp/mail`
- AGENTS.md: `## Mail`

A page sends like this:

```rust
let outbox = ctx.provided::<crate::plugins::mail::Outbox, _>().expect("dispatched");
let message = outbox
    .message() // `From` is `[mail] from`
    .to(brocanth_mail::Address::parse("person@example.test")?)
    .subject("Welcome")
    .text("Hello")
    .build()?;
outbox.deliver_later(message).await?;
```

`deliver_later` with `jobs` keeps the message in a pending store, puts
its id on the `mail` queue and returns. Without `jobs` it sends at once.
The page is the same in both shapes.

## Settings

Section `[mail]`. Environment variable: `$<APP>_MAIL__<KEY>`. Flag: `--mail.<key>`. Rename or hide a flag in `src/modules/mail.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `transport` | text | `"log"` | `--mail.transport` | Where mail goes: `smtp`, `log` or `folder`. |
| `from` | text | `"no-reply@example.com"` | `--mail.from` | The address every message comes from. |
| `from_name` | text | the project title | `--mail.from_name` | The name shown beside that address. |
| `folder` | text | `"tmp/mail"` | `--mail.folder` | Where `folder` writes its .eml files. |
| `smtp_host` | text | `"localhost"` | `--mail.smtp_host` | The SMTP relay. |
| `smtp_port` | integer | `587` | `--mail.smtp_port` | The relay's port. |
| `smtp_security` | text | `"starttls"` | `--mail.smtp_security` | `tls`, `starttls`, or `plain` for a local catcher only. |
| `smtp_username` | text | `""` | `--mail.smtp_username` | The relay's user name. Empty for none. |
| `smtp_password` | text, secret | `""` | none | The relay's password. Empty for none. |
| `worker` | bool | `true` (only with `jobs`) | `--mail.worker` | Run the worker that sends queued mail in this process. |

## Decisions

- **The default transport is `log`, and only `smtp` delivers.** A
  development file copied to production then sends nothing to real
  people, and the start warns. A mail that reached a customer from a
  laptop cannot be called back.
- **The SMTP password is a secret.** It has no flag, because every user on
  the host can read a process's arguments.
- **With `jobs`, mail has a queue and a worker of its own.** The jobs
  module owns its job registry in its own file, and a module cannot write
  into another module's file. So the jobs plugin publishes its `Queue` in
  the services, and the mail plugin starts a second worker on the `mail`
  queue. A slow relay then holds up no other job.
- **A queued job carries an id, not the message.** A message can hold a
  secret: the password reset link carries its token. A job payload waits
  in the jobs table while the job waits and retries, the jobs tab shows
  it, and a backup keeps it. So `deliver_later` writes the message to a
  `Pending` store (`brocanth_mail::pending`) and enqueues its random id.
  Each attempt reads the row again, so a retry still sends. The worker
  deletes the row after the send and after the last attempt, and every
  row expires after one hour (`DEFAULT_TTL`), so a lost job leaves no
  link behind for longer. The five attempts of the default backoff take
  less than three minutes. With a database the store is the
  `brocanth_mail_pending` table (`PgPending`, created on first use), so a
  queued message survives a restart as its job does. With a queue in
  memory the store is in memory too (`MemoryPending`).
  Rejected: rendering from a template name in the worker. The message is
  free text that a page builds, and the reset token exists only as a hash
  after the request; the worker cannot build the link again. Also
  rejected: keeping the message in the payload and deleting it after the
  last attempt. The link would still wait in the jobs table for every
  retry, with no bound.
- **`deliver_later` exists without `jobs`, and sends at once.** Adding
  `jobs` later moves the send off the request with no change to a page.

## Read more

- [brocanth-mail](../../crates/brocanth-mail/README.md)
- Source: [`crates/brocanth-mail/src/scaffold.rs`](../../crates/brocanth-mail/src/scaffold.rs)
