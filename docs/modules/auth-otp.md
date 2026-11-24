# `auth-otp`

Sign in with an eight-digit code mailed to the address, under
`/account/otp`.

The wizard calls it: *Sign in with a code sent by mail*.

| | |
|---|---|
| Categories | **Routes**, Enhancer |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | [`auth`](auth.md), off until you tick it |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-auth` (through `auth`) |
| Without it | no sign-in by mailed code; a person signs in with a password |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/auth_otp.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | [`auth-screens`](auth-screens.md): the code pages share its helpers, its markup rules and the sign-in that opens the session; [`mail`](mail.md): the code leaves by mail, and only the mailbox ever sees it |
| After | [`auth-screens`](auth-screens.md), [`mail`](mail.md), [`auth-screens`](auth-screens.md) |
| Provides | none |
| Requires | none |
| Uses | `PgPool` (a Postgres pool): accounts are in Postgres, so the generated end-to-end test is left out |
| Adapts to | none |
| Contributes | `AgentsSection` `auth-otp`: how a mailed code signs somebody in, and what keeps it safe |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | `[auth_otp]`: `digits`, `ttl_secs`, `max_attempts` |
<!-- end generated: capabilities -->

## What it writes

Files:

- `src/pages/account/otp/index.rs`: `/account/otp`. GET draws the form;
  POST asks `Otp::request` for a code, mails it, and puts the challenge
  in the `auth_otp` cookie.
- `src/pages/account/otp/code.rs`: `/account/otp/code`. POST checks the
  code with `Otp::authenticate`, then opens the session through
  `auth_screens::admit`.
- `src/pages/account/otp/screens.rsx`: the markup of both pages.
- `src/plugins/auth_otp/mod.rs`: the plugin that holds `[auth_otp]` and
  checks it at start, `Codes::otp`, and the challenge cookie. With the
  store in memory, it also holds tests that walk both pages through the
  application.

Slots:

- src/plugins/mod.rs (the module list): `pub mod auth_otp;`
- config/<env>.toml: `[auth_otp]`
- the route table: `/account/otp`, `/account/otp/code`
- AGENTS.md: `## Sign-in by mailed code`

And `auth-screens` adapts: its sign-in form links to `/account/otp`, and
`admit` turns on `FactorPolicy::accepting_mailbox_as_primary`.

## Settings

Section `[auth_otp]`. Environment variable: `$<APP>_AUTH_OTP__<KEY>`.
Flag: `--auth_otp.<key>`. No secret.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `digits` | integer | `8` | `--auth_otp.digits` | Digits in a code, 6 to 10. |
| `ttl_secs` | integer | `600` | `--auth_otp.ttl_secs` | How long a code works, in seconds, 60 to 3600. |
| `max_attempts` | integer | `5` | `--auth_otp.max_attempts` | Wrong guesses a code survives, 1 to 10. |

A value outside its range stops the start with a `help:` line.

## Decisions

- **A code, not a link.** A mail scanner opens every link in a mail, and
  a link signs in whichever browser opens it. A code is typed into the
  browser that asked for it.
- **The mailbox stands in for the password.** The crate counts a mailed
  code as a second factor, and `FactorPolicy::admit` refuses it alone.
  `FactorPolicy::accepting_mailbox_as_primary` is the explicit switch.
  Reason: a password reset by mail already lets the mailbox in, so a code
  by mail opens no new door. The mailed code never counts as the second
  factor too: an account with TOTP gives its TOTP code after it.
- **The same answer for every address.** An unknown or malformed address
  takes the same path, pays the same argon2id hash, gets the same cookie
  and the same page. Only the mail says which case it was.
- **One answer for a wrong, spent, expired or unknown code.** The crate
  answers one error for all; the page draws one sentence.
- **Settings, not options.** The values may differ between staging and
  production, so they are settings, checked at start with the crate's own
  ranges. The defaults are the crate's arithmetic: 5 guesses against 10^8.
- **A rate limit is recommended, not built in.** Each request costs an
  argon2id hash and sends a mail. `middleware-rate-limit` has an example
  rule for the sign-in paths; add `/account/otp` to it.
- **No generated test with Postgres.** With `db`, accounts are in
  Postgres, and a test of the pages would need a database.

## Read more

- Runtime: [`crates/brocanth-auth/src/otp.rs`](../../crates/brocanth-auth/src/otp.rs)
- Source: [`crates/brocanth-auth/src/scaffold/otp.rs`](../../crates/brocanth-auth/src/scaffold/otp.rs)
- Golden: [`crates/brocanth-auth/tests/golden/auth-otp.txt`](../../crates/brocanth-auth/tests/golden/auth-otp.txt)
