# `debug-toolbar`

a panel on each HTML page in a debug build: timing, SQL statements, hooks, settings.

The wizard calls it: *A panel on each page in development: timing, SQL, hooks, settings*.

| | |
|---|---|
| Categories | **Middleware** |
| Wizard question | Operations |
| Default | off |
| Presets | none |
| Part of | [`debug`](debug.md), ticked with its parent |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-debug` (feature `config` with [`config`](config.md), `model` with [`model`](model.md)) |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/debug_toolbar.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | [`model`](model.md): the ORM's statements are counted per request and listed by shape |
| Contributes | `AgentsSection` `debug-toolbar`: what the development panel shows, and why it never ships |
| Collects | none |
| Exports | none |
| Hooks | `start` (Identify, on by default): start the clock and the statement count, first of all request hooks; `panel` (Identify, on by default): write the panel before `</body>` of each HTML answer |
| Options (build time) | none |
| Settings (run time) | `[debug_toolbar]`: `enabled`, `max_statements` |
<!-- end generated: capabilities -->

## What it does

In a debug build, each HTML answer (a 404 too) ends with a panel, before
`</body>`:

- **Request**: the verb, the path, the status, and the time from the first
  request hook to the response hooks.
- **SQL**: with [`model`](model.md), the statements the ORM issued for the
  request, grouped by shape with a count, the most repeated first. An N+1
  is one shape issued many times. Without `model`, the panel says how to
  get the count.
- **Hooks**: the hooks of each chain (request, response, error) in the
  order they run, and the hooks a module file turned off.
- **Settings**: the settings report (`--settings`): every layer, then the
  merged values, secrets masked.

A JSON answer and a `dp-partial` fragment are left alone.

## Decisions

- **Never in a release build.** The plugin installs its hooks only when
  `debug_assertions` is on (`brocanth_debug::ACTIVE`). In a release build
  it installs no hook and registers no probe, whatever `enabled` says, so
  no answer is rewritten. The manifest is `development_only` as well:
  `brocanth check --deploy` refuses the project with
  `deploy/development-only` until the module is removed.
- **A response hook, not a page or a layer.** A hook sees every answer,
  404 and 405 included, and inserts the panel by the same rule as the
  analytics beacon and the consent banner.
- **SQL from the ORM's probe.** SeaORM reports each statement to a
  callback, and `brocanth_model::Probe` groups them by shape. The toolbar
  registers the probe as a service in a debug build; the `model` plugin,
  which adapts to this module, installs it on its connection
  (`Orm::probed`). A query through `ctx.db()` (sqlx, without the ORM) is
  not counted. The probe sees the whole process: two requests at once
  count each other's statements. Right for one developer, wrong under
  load: one more reason the module never ships.
- **The chain, not a trace.** The panel lists the hooks of each chain in
  run order, from `Pipeline::hook_names` in the core. A request hook that
  answered stops the hooks after it; the panel does not say which one did.
- **Inline styles.** The panel carries a `style` attribute. A strict CSP
  from [`middleware-security-headers`](middleware-security-headers.md) may
  drop it; the panel then shows unstyled at the end of the page.

## What it writes

Files:

- `src/plugins/debug_toolbar/mod.rs`: tests that load `/` through the
  application and check the panel: its status line, its hooks, its
  settings, and its SQL count with `model`.
- `src/modules/debug_toolbar.rs`: the two hooks, each one a toggle.

With [`model`](model.md), `src/plugins/model/mod.rs` hands the ORM's
connection to the toolbar's probe.

Slots:

- src/plugins/mod.rs: `pub mod debug_toolbar;`
- src/plugins/mod.rs (the builder chain), the provider list, the hook record
- src/plugins/mod.rs (the context): `toolbar.observe($app.pipeline(), crate::plugins::settings_report())`
- config/<env>.toml: `[debug_toolbar]`
- AGENTS.md: `## Development panel`

## Settings

| Setting | Default | Meaning |
|---|---|---|
| `enabled` | `true` | Show the panel. A release build shows none, whatever this says. |
| `max_statements` | `50` | The most SQL statement shapes the panel lists. The count is always the full one. |

## Read more

- [`debug`](debug.md), [`model`](model.md)
- Source: [`crates/brocanth-debug/src/toolbar.rs`](../../crates/brocanth-debug/src/toolbar.rs), [`crates/brocanth-debug/src/scaffold.rs`](../../crates/brocanth-debug/src/scaffold.rs)
