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
| Needs | none |
| After | [`config`](config.md), [`jobs`](jobs.md) |
| Adapts to | [`config`](config.md), [`jobs`](jobs.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-mail` (features `smtp`, `serde`) |
| Without it | no mail; a password reset or a verification link cannot leave the server |

## What it writes

Files:

- `src/plugins/mail/mod.rs`: the plugin, and the `Outbox` a page sends
  through. With `jobs`, also the `DeliverMail` job and a worker for the
  `mail` queue.

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

`deliver_later` with `jobs` puts the message on the `mail` queue and
returns. Without `jobs` it sends at once. The page is the same in both
shapes.

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
- **`deliver_later` exists without `jobs`, and sends at once.** Adding
  `jobs` later moves the send off the request with no change to a page.

## Read more

- [brocanth-mail](../../crates/brocanth-mail/README.md)
- Source: [`crates/brocanth-mail/src/scaffold.rs`](../../crates/brocanth-mail/src/scaffold.rs)
