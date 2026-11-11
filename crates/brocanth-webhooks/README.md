# brocanth-webhooks

Webhooks for brocanth, in both directions.

- **Incoming**: a request hook that answers `POST {path}/{source}`. It
  checks the signature (Standard Webhooks, Stripe, or plain HMAC such as
  GitHub's), refuses a delivery outside the replay window, drops a
  duplicate by its idempotency key, and hands a typed event to the handler
  you wrote. It runs before the CSRF check, so no path is exempted.
- **Outgoing** (feature `out`): endpoints with their own secrets, one
  delivery row per event per endpoint, and a `brocanth-jobs` job per
  delivery. The queue retries with backoff; the row is the delivery log.
  The body is signed with Standard Webhooks.
- **Admin** (feature `admin`): a tab that lists the endpoints and the
  deliveries, with a "Send again" button.

`brocanth create --with webhooks-in` or `--with webhooks-out` writes the
plugin into a project. See `docs/modules/webhooks-in.md` and
`docs/modules/webhooks-out.md`.

## Receive

```rust
use brocanth_webhooks::{Delivery, Handler, Incoming, Rejection, Scheme};

#[derive(serde::Deserialize)]
struct Paid {
    amount: i64,
}

struct Payments;

impl Handler for Payments {
    type Event = Paid;

    async fn handle(&self, event: Paid, delivery: &Delivery) -> Result<(), Rejection> {
        // `delivery.id` is the idempotency key; this runs once per key.
        let _ = (event.amount, &delivery.id);
        Ok(())
    }
}

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let secret = brocanth_webhooks::sign::new_secret();
let receiver = Incoming::new("/webhooks")
    .source("billing", Scheme::standard(&secret)?, Payments)?
    .build()?;

// What a sender does: sign `{id}.{timestamp}.{body}`.
let key = brocanth_webhooks::sign::decode_secret(&secret)?;
let body = br#"{"amount": 1200}"#;
let now = 1_700_000_000;
let signature = brocanth_webhooks::sign::sign_standard(&key, "msg_1", now, body);
let stamp = now.to_string();
let headers = |name: &str| match name {
    "webhook-id" => Some("msg_1"),
    "webhook-timestamp" => Some(stamp.as_str()),
    "webhook-signature" => Some(signature.as_str()),
    _ => None,
};

let runtime = tokio::runtime::Builder::new_current_thread().build()?;
let first = runtime.block_on(receiver.receive_at("billing", headers, body, now));
assert_eq!(first.status, 200);
let again = runtime.block_on(receiver.receive_at("billing", headers, body, now));
assert_eq!(again.message, "already received");
let late = runtime.block_on(receiver.receive_at("billing", headers, body, now + 3600));
assert_eq!(late.status, 401);
# Ok(())
# }
```

Register the hook in a plugin with
`registrar.request_async("receive", Order::new().before_if_present("csrf.check"), receiver)`.
The generated `src/plugins/webhooks_in/mod.rs` does it for you.

## Send

```rust,ignore
let hooks = ctx.provided::<brocanth_webhooks::Outgoing, _>().expect("dispatched");
let endpoint = hooks.add_endpoint("https://example.com/hooks", &["invoice.*"], "billing").await?;
// Show `endpoint.secret` once, to whoever registered it.
hooks.emit("invoice.paid", &serde_json::json!({ "id": 42 })).await?;
```

A row change becomes `<collection>.created`, `.updated` or `.deleted`
through `Outgoing::sink`, a `ChangeSink` you add to the model layer's sink.

## Decisions

- **One crate, two directions.** The outgoing side signs with the code the
  incoming side verifies, and a test proves they agree.
- **A hook, not a page, for incoming.** A page sits behind the CSRF check,
  and an exempt path turns the check off for every POST under it, signed or
  not. The hook answers only what it verifies.
- **The key is claimed before the handler, released on failure.** Two
  copies that arrive together run once; a failed handler runs again on the
  sender's retry.
- **The retries are the queue's.** A delivery is a `brocanth-jobs` job:
  backoff, the dead letter and the restart story are already written there.
- **No private endpoints by default.** A webhook URL is chosen by whoever
  registers it; without the check it makes the server call its own
  network. A name that resolves to a private address is an open item.
