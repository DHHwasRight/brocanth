# `auth-mfa`

Who must use a second factor, forced TOTP set-up, recovery codes, a QR
code and login alerts.

The wizard calls it: *Second factors: who must use one, recovery codes,
alerts*.

| | |
|---|---|
| Categories | **Routes**, Middleware, Enhancer |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | [`auth`](auth.md), off until you tick it |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-auth` (through `auth`), `qrcode` (SVG only) |
| Without it | no second-factor policy; an account uses TOTP only when its owner turns it on |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/auth_mfa.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | [`auth-screens`](auth-screens.md): the second factor is asked for on its code page, and its sign-in asks this module's policy |
| After | [`auth-screens`](auth-screens.md), [`auth-screens`](auth-screens.md) |
| Provides | none |
| Requires | none |
| Uses | `PgPool` (a Postgres pool): accounts are in Postgres, so the generated end-to-end test is left out |
| Adapts to | none |
| Contributes | `AgentsSection` `auth-mfa`: who must give a second factor, and the rules of the factor pages |
| Collects | none |
| Exports | none |
| Hooks | `client` (Identify, required): write the client address to the request state, for login alerts |
| Options (build time) | none |
| Settings (run time) | `[auth_mfa]`: `require`, `alerts` |
<!-- end generated: capabilities -->

## What it writes

Files:

- `src/pages/account/mfa/index.rs`: `/account/mfa`, the factors of the
  signed-in account: TOTP on or off, the recovery codes left, and the
  buttons below.
- `src/pages/account/mfa/recovery.rs`: POST `/account/mfa/recovery`, a
  new set of recovery codes, shown once.
- `src/pages/account/mfa/off.rs`: POST `/account/mfa/off`, TOTP off with
  its recovery codes. Refused when the policy holds the account to a
  second factor.
- `src/pages/account/mfa/setup.rs`: `/account/mfa/setup`, TOTP for a
  sign-in the policy does not admit without one.
- `src/pages/account/mfa/screens.rsx`: the markup.
- `src/plugins/auth_mfa/mod.rs`: the plugin, its `client` hook, `policy`,
  `qr_svg` and `alert`. With the store in memory, also tests that walk the
  pages through the application.

Slots:

- src/plugins/mod.rs: the hooking registration from `[auth_mfa]`, the
  provider list and the hook record
- config/<env>.toml: `[auth_mfa]`
- the route table: the four routes above
- AGENTS.md: `## Second factors`

And `auth-screens` adapts:

- `admit` asks `auth_mfa::policy`; a sign-in that must enrol goes to
  `/account/mfa/setup` with its proof sealed; a session that opens calls
  `auth_mfa::alert`.
- `/account/totp` draws the QR code.

## Settings

Section `[auth_mfa]`. Environment variable: `$<APP>_AUTH_MFA__<KEY>`.
Flag: `--auth_mfa.<key>`. No secret: the key that seals TOTP secrets is
`[auth] totp_key_env`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `require` | text | `"enrolled"` | `--auth_mfa.require` | Who must give a second factor: enrolled, staff or everyone. |
| `alerts` | bool | `true` | `--auth_mfa.alerts` | Mail the owner when a sign-in comes from a new network. |

Another word in `require` stops the start with a `help:` line.

## Hooks

`client` (Identify, required): writes the client address to the request
state. It runs after `proxy-headers.read` when that hook is there, and
takes the address a trusted proxy reported; else the socket's peer.

## Decisions

- **The pages `auth-screens` has stay there.** A project with
  `auth-screens` alone keeps two-step sign-in at `/account/code` and
  `/account/totp`. Moving them would change every such project. This
  module adapts them and adds what is missing.
- **The policy is a setting, read at each sign-in.** `staff` reads the
  superuser flag and the roles at each sign-in, so a role given today asks
  for a factor at the next one. A setting, because staging and production
  may differ.
- **Forced enrolment opens no session.** The password's proof waits sealed
  in the `auth_pending` cookie, as for a pending code.
  `AccountHolder::enrolling` reads the holder from that proof: a credential
  checked minutes ago, which counts as fresh. No half-session exists that
  a page could forget to check.
- **A required factor stays on.** `/account/mfa/off` refuses an account
  the policy holds to a second factor, and the page hides the button.
- **A changed factor needs a fresh elevation.** The crate asks for it.
  `/account/elevate` now takes the TOTP code beside the password, because
  an account with TOTP cannot elevate with a password alone, and it goes
  back to the page in `then`, a path on this site only.
- **Login alerts here, not in `auth-otp`.** Every sign-in goes through
  `admit`, which this module already adapts. A network is the /24 or /48
  of the address, digested with the session key (`brocanth_auth::alerts`).
  No address, no judgement, no mail: "unknown" read as "new" alerts on
  every sign-in. The first sign-in ever sends nothing.
- **The client address comes from a hook.** A page's `Ctx` has no peer
  address, so the `client` hook writes one to the request state.
- **The QR code is SVG from `qrcode`, drawn on the server.** No image
  crate, no script, and no third-party image service that would see the
  secret.
- **No generated test with Postgres.** With `db`, accounts are in
  Postgres.

## Read more

- Runtime: [`crates/brocanth-auth/src/mfa.rs`](../../crates/brocanth-auth/src/mfa.rs),
  [`crates/brocanth-auth/src/alerts.rs`](../../crates/brocanth-auth/src/alerts.rs)
- Source: [`crates/brocanth-auth/src/scaffold/mfa.rs`](../../crates/brocanth-auth/src/scaffold/mfa.rs)
- Golden: [`crates/brocanth-auth/tests/golden/auth-mfa.txt`](../../crates/brocanth-auth/tests/golden/auth-mfa.txt)
