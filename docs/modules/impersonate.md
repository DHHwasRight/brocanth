# `impersonate`

Staff act as a customer, with a banner on every page, the operator on the
session and an audit entry.

The wizard calls it: *Staff act as a customer, with a banner and an audit
entry*.

| | |
|---|---|
| Categories | **Routes**, Middleware, Enhancer |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | nothing |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-auth`, feature `admin` with `admin` |
| Without it | no impersonation; support sees a customer's page only by asking for a screenshot |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/impersonate.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | [`superuser`](superuser.md): only staff impersonate, and `superuser` reads the signed-in member of staff; [`auth-screens`](auth-screens.md): the operator types the password again at /account/elevate, and the pages share its helpers |
| After | [`superuser`](superuser.md), [`auth-screens`](auth-screens.md), [`superuser`](superuser.md), [`auth-screens`](auth-screens.md), [`audit`](audit.md) |
| Provides | none |
| Requires | none |
| Uses | `PgPool` (a Postgres pool): accounts are in Postgres, so the generated end-to-end test is left out |
| Adapts to | [`audit`](audit.md): each start and stop is an entry in the change history, with the operator and the reason |
| Contributes | `AgentsSection` `impersonate`: who may act as somebody, and what an impersonated session may not do; `AdminPanel` `impersonate`, on: an Impersonate tab that lists the accounts, each with a form that starts acting as it |
| Collects | none |
| Exports | none |
| Hooks | `read` (Identify, required): read the session once, and note an impersonation for the banner; `banner` (Identify, on by default): put the impersonation banner, with its stop button, after <body> on every HTML page |
| Options (build time) | none |
| Settings (run time) | none |
<!-- end generated: capabilities -->

## What it writes

Files:

- `src/pages/impersonate/index.rs`: `/impersonate`. GET draws the form
  (address and reason); POST starts acting as the account, from that form
  or from the admin tab (with the account's `id`).
- `src/pages/impersonate/stop.rs`: POST `/impersonate/stop`, from the
  banner's button. Deletes the impersonated session and gives the
  operator's own back.
- `src/pages/impersonate/screens.rsx`: the start form.
- `src/plugins/impersonate/mod.rs`: the hooking plugin (`read`, `banner`)
  and `record`, which notes each start and stop. With the store in memory,
  also tests that walk an impersonation through the application.

Slots:

- src/plugins/mod.rs: the hooking registration, the provider list, the
  hook record, and `hooks_off_from("impersonate", ..)`
- `AdminPanels`, when the admin collects tabs, behind `integrate.admin`:
  `brocanth_auth::admin::ImpersonatePanel`, RBAC resource `impersonate`
- the route table: `/impersonate`, `/impersonate/stop`
- AGENTS.md: `## Impersonation`

With [`audit`](audit.md), `record` writes an entry to the change history:
the operator is the actor, the subject's account is the row, the reason is
the label. Without it, `record` writes a log line.

## Settings

None, and no secret. Who may act is a role (`impersonate.edit`). How long
an impersonated session lives is the crate's
`SessionConfig::impersonation_timeout` (thirty minutes), and it never
outlives the operator's own session.

## Hooks

- `read` (Identify, required): with a session cookie, reads the session
  once and notes an impersonation in the request state.
- `banner` (Identify, on): puts an `<aside aria-label="Impersonation">`
  after `<body>` on every HTML page of an impersonated session: whose
  account, the reason, and a stop button with the CSRF token. Turn it off
  in `src/modules/impersonate.rs` only if your layout draws its own.

## Decisions

- **Only what the role grants.** The permission is `impersonate.edit`
  (`Staff::IMPERSONATE`), a level on the `impersonate` resource, which is
  also the admin tab's RBAC key. A superuser holds it. A superuser gives
  `impersonate.owner` to a support lead, who gives `impersonate.edit` to
  support staff in the Roles tab, and nothing more. It was
  `accounts.impersonate`, which no role screen could hand out.
- **No member of staff is a subject.** `Staff::impersonate` refuses every
  staff subject for every operator, superusers included. That is stricter
  than "no superuser by a non-superuser": acting as any member of staff
  would hand the operator rights nobody gave them.
- **The operator types the password first.** The crate wants an elevated
  operator session. The start page sends the operator to
  `/account/elevate?then=/impersonate`.
- **The credentials stay the customer's.** The crate refuses an
  impersonated session the password, the second factors, the recovery
  codes, elevation, a nested impersonation and provider links, each by a
  type. An impersonated session is never staff, so it cannot open the
  admin or start another impersonation.
- **The banner is a response hook.** The layout belongs to the project, so
  no module edits it. A hook reaches every HTML page, whatever renders it.
- **The admin tab lists; a page starts.** A panel answers with a body or a
  redirect and cannot set a cookie, so the tab's forms post to
  `/impersonate`. The admin stamps the CSRF token into them.
- **Stopping deletes the session.** Nothing clears
  `session.impersonation`, so the field stays trustworthy in an audit.
- **No generated test with Postgres.** With `db`, accounts are in
  Postgres.

## Read more

- Runtime: [`crates/brocanth-auth/src/impersonation.rs`](../../crates/brocanth-auth/src/impersonation.rs),
  [`crates/brocanth-auth/src/staff.rs`](../../crates/brocanth-auth/src/staff.rs),
  [`crates/brocanth-auth/src/admin/impersonate.rs`](../../crates/brocanth-auth/src/admin/impersonate.rs)
- Source: [`crates/brocanth-auth/src/scaffold/impersonate.rs`](../../crates/brocanth-auth/src/scaffold/impersonate.rs)
- Golden: [`crates/brocanth-auth/tests/golden/impersonate.txt`](../../crates/brocanth-auth/tests/golden/impersonate.txt)
