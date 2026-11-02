# brocanth-mail

Mail for brocanth: one `Mailer` trait, a `Memory` backend that records what it
was given, an `Smtp` backend behind the `smtp` feature, and headers that
cannot carry a newline. An `Address` and a `Subject` refuse CR and LF, so a
header injection is not a value the types can hold. Every body is base64. The
envelope comes from the recipients, never from the headers. Use this crate to
send the mail that `brocanth-auth` produces but does not send: a password
reset, a registration notice, a one-time code. It is a seam, not a mail
service. It has no queue, no templates and no `Bcc`.

## Getting started

```toml
[dependencies]
brocanth-mail = { git = "https://github.com/DHHwasRight/brocanth", branch = "main" }
```

| Feature | Default | Adds |
|---|---|---|
| `smtp` | off | `Smtp`, `SmtpConfig`, `Security` (`lettre`, with rustls and a pool) |
| `serde` | off | `Serialize` and `Deserialize` on `Message`, `Address` and `Subject`, so a message can be a job payload. Deserialising runs every check again. |

This crate has no `scaffold` feature and no module key: `brocanth create
--with mail` and `brocanth add mail` do not exist. Register `MailPlugin` by
hand in the application (see below). The plugin reads no environment and no
config section, so the application builds the `Mailer` and the sender in its
own `main`.

An SMTP password is a secret. Read it from the environment or from a config
file, never from a command-line flag: every user on the host can read a
process's arguments. `SmtpConfig`'s `Debug` prints whether credentials are
set, never their value.

## Examples

### Build a message

```rust
use brocanth_mail::{Address, Message};

# fn main() -> Result<(), brocanth_mail::MailError> {
let message = Message::builder(Address::named("Example", "no-reply@example.test")?)
    .to(Address::parse("person@example.test")?)
    .subject("Reset your password")
    .text("Open https://example.test/reset?t=abc")
    .html("<a href=\"https://example.test/reset?t=abc\">Reset</a>")
    .build()?;

let wire = String::from_utf8(message.render()).unwrap();
assert!(wire.contains("Subject: Reset your password\r\n"));
assert!(!wire.contains("Bcc:"));

// A newline in a subject is refused, not stripped.
assert!(Message::builder(Address::parse("a@example.test")?)
    .to(Address::parse("b@example.test")?)
    .subject("Hello\r\nBcc: harvest@attacker.test")
    .build()
    .is_err());
assert!(Address::parse("victim@example.test\r\nBcc: x@attacker.test").is_err());
# Ok(())
# }
```

### Send, and check what was sent

`Mail` is the service: a backend and a default sender. `Memory` keeps every
message, so a test can ask what was sent and to whom.

```rust
use std::sync::Arc;
use brocanth_mail::{Address, Mail, Memory};

# fn block_on<F: std::future::Future>(f: F) -> F::Output {
#     // `Memory::send` never waits, so one poll finishes it.
#     let mut f = std::pin::pin!(f);
#     match f.as_mut().poll(&mut std::task::Context::from_waker(std::task::Waker::noop())) {
#         std::task::Poll::Ready(v) => v,
#         std::task::Poll::Pending => unreachable!(),
#     }
# }
# fn main() -> Result<(), brocanth_mail::MailError> {
let outbox = Arc::new(Memory::new());
let mail = Mail::new(outbox.clone(), Address::parse("no-reply@example.test")?);

let message = mail
    .message() // `From` is the default sender
    .to(Address::parse("person@example.test")?)
    .subject("Your code")
    .text("123456")
    .build()?;
# block_on(async {
mail.send(&message).await?;
# Ok::<_, brocanth_mail::MailError>(())
# })?;

assert_eq!(outbox.len(), 1);
let sent = outbox.to("person@example.test");
assert_eq!(sent[0].message.subject().as_str(), "Your code");
assert!(sent[0].headers().contains("From: no-reply@example.test"));
# Ok(())
# }
```

### Register the plugin

The plugin puts `Mail` in the per-request context (`let mail: &Mail =
cx.extract();`) and in the application's services, so another plugin finds it
with `lc.require_service::<Mail>()?`.

```rust
use std::sync::Arc;
use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth_mail::{Address, MailConfig, MailPlugin, Memory};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let app = AppBuilder::new()
    .plugin(MailPlugin::new(MailConfig {
        mailer: Arc::new(Memory::new()),
        default_from: Address::named("Example", "no-reply@example.test")?,
    }))
    .build()?;
# let _ = app;
# Ok(())
# }
```

### Send through a relay

With the `smtp` feature. `Smtp::connect` does not connect: the pool opens a
connection on the first send. It refuses a relay name that it cannot use.

```rust
# #[cfg(feature = "smtp")]
# fn smtp() -> Result<(), brocanth_mail::MailError> {
use brocanth_mail::{Security, Smtp, SmtpConfig};

// The password comes from the environment, never from a flag.
let password = std::env::var("SMTP_PASSWORD").unwrap_or_default();
let config = SmtpConfig::new("smtp.example.test")
    .with_security(Security::StartTls)
    .with_port(587)
    .with_credentials("app", password);
assert!(!format!("{config:?}").contains("SMTP_PASSWORD"));
let mailer = Smtp::connect(&config)?;
# let _ = mailer;
# Ok(())
# }
# fn main() {}
```

In development, point it at a local catcher:
`SmtpConfig::new("localhost").with_security(Security::PlaintextForLocalCatcherOnly).with_port(1025)`.

## Testing

```sh
cargo test -p brocanth-mail
cargo test -p brocanth-mail --features smtp,serde
```

No server and no environment variable is needed. No test talks to a real
relay.

### Testing an application that uses this crate

Register `MailPlugin` with an `Arc<Memory>` and keep a clone of the `Arc`.
Drive the screen with `brocanth-test`'s `TestClient`, then assert on the
outbox: `outbox.len()`, `outbox.to("person@example.test")`, `outbox.last()`.
`Sent::headers()` returns the header block, for an injection test. Call
`outbox.clear()` between cases that share one fixture.

## More

- The crate documentation (`cargo doc -p brocanth-mail --open`): each security
  decision with its reason, and how to wire a `brocanth-auth` password reset
  to a message.
- The `brocanth-auth` crate documentation: the `ResetOutcome`,
  `RegistrationNotice`, `OtpDelivery` and `LoginAlert` values it returns for
  the application to send.
