# `auth-lockout`

Locks an account for minutes after too many wrong passwords.

The wizard calls it: *Lock an account for a while after wrong passwords*.

| | |
|---|---|
| Categories | **Enhancer** |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | [`auth`](auth.md), off until you tick it |
| Needs | none |
| After | [`auth`](auth.md) |
| Adapts to | none |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-auth` (through `auth`) |
| Without it | no lockout; only a rate limit slows a guessing run |

## What it writes

Files:

- `src/plugins/auth_lockout.rs`: `lockout(&auth)` builds a
  `brocanth_auth::Lockout` with the policy in the module file, and a test
  checks that policy.

Slots:

- src/plugins/mod.rs (the module list): `pub mod auth_lockout;`
- AGENTS.md: `## Account lockout`

With [`auth-screens`](auth-screens.md), the sign-in page goes through
`lockout(&auth)`, and the reset page lifts the lock. Without it, call
`lockout(&auth)?.login(auth.accounts(), email, password)` from your own
sign-in handler.

## Module file

`src/modules/auth_lockout.rs`:

| Option | Default | What it does |
|---|---|---|
| `max_failures` | `10` | Wrong passwords that lock an account, 3 or more. |
| `window_secs` | `900` | The seconds in which those wrong passwords count, 60 to 86400. |
| `lock_for_secs` | `900` | How long a lock lasts, in seconds, 60 to 86400. |

## What the runtime does

`Lockout::login` wraps `Accounts::login` and answers an `Attempt`:

- `Admitted`: the password matched and the account is not locked. The
  count goes back to 0.
- `Refused`: a wrong password, an unknown address, or a locked account.
- `JustLocked { account, until }`: a wrong password, and this one locked
  the account. Mail the owner.

The store keeps one row per account in `brocanth_auth_failed_logins`:
the count, the start of its window, and the end of the lock.
`PgStore::migrate` creates it. `MemoryStore` and `PgStore` both implement
`LockoutStore`, and `lockout_store_conformance!` tests another store.

## Decisions

- **A locked account answers like a wrong password.** Even the right
  password gets `Refused`. A page that says "locked" tells a stranger the
  address has an account. The owner learns of the lock by mail.
- **The lock ends by itself.** A lock that only an administrator lifts
  lets anybody who knows an address keep its owner out. A lock for minutes
  makes a guessing run slow and costs the owner minutes.
- **A password reset lifts the lock.** It proves the mailbox, and the mail
  of a lock sends the owner there.
- **An unknown address does the same store work.** Its failures count
  against a fixed sentinel id, so the time of an answer does not tell a
  known address from an unknown one.
- **Per account, not per address.** A guessing run from many addresses
  still locks the account. Limit each address with
  [`middleware-rate-limit`](middleware-rate-limit.md) beside this.
- **Options, not settings.** The policy is the same in every environment
  of one application. The generated test checks it with
  `LockoutPolicy::new`, so a bad value fails `cargo test`, not a deploy.

## Read more

- Source: [`crates/brocanth-auth/src/lockout.rs`](../../crates/brocanth-auth/src/lockout.rs),
  [`crates/brocanth-auth/src/scaffold/lockout.rs`](../../crates/brocanth-auth/src/scaffold/lockout.rs)
- Golden: [`crates/brocanth-auth/tests/golden/auth-lockout.txt`](../../crates/brocanth-auth/tests/golden/auth-lockout.txt)
