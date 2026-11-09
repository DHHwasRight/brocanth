# `webhooks`

Signed webhooks in both directions; pick a direction below.

The wizard calls it: *Webhooks: receive signed deliveries, send signed events*.

| | |
|---|---|
| Categories | **Enhancer** |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | nothing |
| Needs | none |
| After | none |
| Adapts to | none |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-webhooks` |
| Without it | no webhooks; a payment provider or a partner has no verified way in or out |

The parent brings the crate and one rule. The two children are the two
directions. A project picks either or both:

- [`webhooks-in`](webhooks-in.md): receive. Ticked with the parent.
- [`webhooks-out`](webhooks-out.md): send. Off until you tick it, because it
  needs `db` and `jobs`.

**Why one crate for both:** the outgoing half signs with Standard Webhooks,
and the incoming half verifies Standard Webhooks. One signing module serves
both, and a test sends a delivery from one half to the other.

## What it writes

Slots:

- AGENTS.md: `## Webhooks`. A webhook secret is a password: it goes in the
  config file or the environment, never in code or a flag.

## Read more

- [brocanth-webhooks](../../crates/brocanth-webhooks/README.md)
- Source: [`crates/brocanth-webhooks/src/scaffold.rs`](../../crates/brocanth-webhooks/src/scaffold.rs)
