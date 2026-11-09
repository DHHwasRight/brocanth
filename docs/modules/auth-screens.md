# `auth-screens`

Sign-up, sign-in, sign-out, reset, verify and TOTP pages under `/account`.

The wizard calls it: *Sign-up, sign-in and reset pages*.

| | |
|---|---|
| Categories | **Routes**, Enhancer |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | [`auth`](auth.md), off until you tick it |
| Needs | [`rsx`](rsx.md), [`input`](input.md), [`middleware-csrf`](middleware-csrf.md), [`mail`](mail.md), [`flash`](flash.md) |
| After | [`auth`](auth.md), [`flash`](flash.md), [`mail`](mail.md), [`auth-lockout`](auth-lockout.md) |
| Adapts to | [`config`](config.md), [`jobs`](jobs.md), [`cache`](cache.md), [`auth-lockout`](auth-lockout.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-auth` (through `auth`) |
| Without it | no account pages; the auth services are there and nothing calls them |

## What it writes

Files:

- `src/pages/account/signup.rs`, `signin.rs`, `signout.rs`, `forgot.rs`,
  `code.rs`, `totp.rs`, `reset/[token].rs`, `verify/[token].rs`
- `src/pages/account/screens.rsx`: the markup of every screen
- `src/plugins/auth_screens/mod.rs`: the plugin and the helpers every page
  shares. With the store in memory, it also holds a test that walks
  sign-up and sign-in through the application.

Slots:

- src/plugins/mod.rs (the module list): `pub mod auth_screens;`
- config/<env>.toml: `[auth_screens]`
- the route table: the eight routes below
- AGENTS.md: `## Account screens`

| Route | GET | POST |
|---|---|---|
| `/account/signup` | the form | makes the account, mails a link to confirm the address |
| `/account/signin` | the form | checks the password, then opens a session or asks for a code |
| `/account/code` | the form | checks a TOTP or recovery code, then opens the session |
| `/account/signout` | a button | ends the session |
| `/account/forgot` | the form | mails a reset link if the address has an account |
| `/account/reset/{token}` | the form | sets the new password, ends every session |
| `/account/verify/{token}` | a button | confirms the address |
| `/account/totp` | a new key and its `otpauth://` link | confirms the key, shows the recovery codes once |

With [`auth-lockout`](auth-lockout.md), sign-in goes through the lockout:
it counts wrong passwords and mails the owner when an account locks. A
reset lifts the lock.

## Module file

`src/modules/auth_screens.rs`:

| Option | Default | What it does |
|---|---|---|
| `after_sign_in` | `"/"` | Where a person lands after signing in. |
| `pending_secs` | `300` | How long a sign-in waits for its second factor, in seconds. |

## Settings

Section `[auth_screens]`. Environment variable: `$<APP>_AUTH_SCREENS__<KEY>`. Flag: `--auth_screens.<key>`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `base_url` | text | `"http://localhost:3000"` | `--auth_screens.base_url` | The address links in mail start with: scheme, host, port. |
| `issuer` | text | the project title | `--auth_screens.issuer` | The name an authenticator app shows beside the code. |

## Decisions

- **Pages, not a mounted router.** Each screen is a file the project owns,
  in its layout, with its renderer. The admin mounts a router because its
  screens belong to the crate. These belong to the application.
- **Five requirements, no adaptation.** `rsx` draws the forms, `input`
  reads them, `middleware-csrf` checks each post, `mail` carries the
  links, `flash` says what happened after a redirect. Without one of them
  a screen is broken or unsafe.
- **A link in mail comes from `base_url`, never from the request.** A
  reset link built from the `Host` header goes where the attacker's
  request said, with the victim's token in it. The plugin refuses to
  start when `base_url` is not an absolute address.
- **A verification link asks for a click.** The GET draws a button and the
  POST spends the token. A mail scanner that follows each link must not
  confirm an address for a person who never saw the mail.
- **The same answer for a known and an unknown address.** Sign-up,
  sign-in and the reset request use the same words and status whatever
  the store holds. The crate makes the timing alike. Only the mail says
  which case it was, and only its owner reads it.
- **The half-finished sign-in is a sealed cookie for minutes.** The
  password passed and the code has not. `Sessions::seal_proof` signs the
  proof with the session key. `Path=/account` keeps the cookie off every
  other request. A page never holds the key.
- **No generated test with Postgres.** With `jobs` or `cache`, accounts are
  in Postgres, and a test of the pages would need a database.

## Read more

- Source: [`crates/brocanth-auth/src/scaffold/screens.rs`](../../crates/brocanth-auth/src/scaffold/screens.rs)
- Golden: [`crates/brocanth-auth/tests/golden/auth-screens.txt`](../../crates/brocanth-auth/tests/golden/auth-screens.txt)
