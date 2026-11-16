# `beacon`

A few lines of script that report each page view to an analytics server; no cookie.

The wizard calls it: *Report page views of these pages to a brocanth server with `analytics`*.

| | |
|---|---|
| Categories | **Middleware** |
| Wizard question | Operations |
| Default | off |
| Presets | none |
| Part of | nothing |
| Conflicts | [`analytics`](analytics.md): both would count one page: the hook on the server, and the beacon in the browser |
| Outside the runtime | yes |
| Crates | `brocanth-analytics` (no default features; feature `config` with `config`) |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/beacon.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | none |
| Contributes | `AgentsSection` `beacon`: how the beacon is sent, and that it sets no cookie |
| Collects | none |
| Exports | none |
| Hooks | `insert` (Identify, required): put the snippet into each HTML page, while `collector` is set |
| Options (build time) | none |
| Settings (run time) | `[beacon]`: `collector`, `respect_gpc` |
<!-- end generated: capabilities -->

**Why its own module and not a child of `analytics`:** the static site and
the server that counts are two projects. A static site cannot hold
`analytics`, which needs a running server and a database, and a child
cannot be chosen without its parent.

Hooks: `beacon.insert`, required: it is the module. It installs only while
`collector` is set.

## What it writes

Files: none.

Slots:

- src/plugins/mod.rs (the builder chain): `plugin_hooking_from_config::<brocanth_analytics::Beacon>("beacon")`
- src/plugins/mod.rs (the provider list): `beacon brocanth_analytics::Beacon`
- src/plugins/mod.rs (the hook record): `brocanth_analytics::Beacon`
- config/<env>.toml: `[beacon]`
- AGENTS.md: `## The beacon`

## What the plugin does

- A response hook puts a `<script>` before the last `</body>` of every HTML
  page (at its end when it has none; never in a dp-swap partial), once
  `collector` is set. `brocanth export` runs pages through the
  hooks, so every exported file carries it.
- In the browser, the script sends the path, the query and
  `document.referrer` to `collector`: `navigator.sendBeacon`, or a 1×1
  image without it. It wraps `history.pushState`, so a dp-swap navigation
  sends again. It sets no cookie and stores nothing.
- With `respect_gpc`, a browser with Global Privacy Control or Do Not Track
  sends nothing.
- The server counts the beacon only when its `[analytics]
  collector_origins` lists this site's origin. Its views are a site of
  their own on the server's dashboard, named by this site's host. See
  [`docs/analytics.md`](../analytics.md), decision 7.

## Settings

Section `[beacon]`. Environment variable: `$<APP>_BEACON__<KEY>`. Flag: `--beacon.<key>`. Rename or hide a flag in `src/modules/beacon.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `collector` | text, optional | commented out | `--beacon.collector` | The analytics server's collector URL. Unset, no page carries the snippet. |
| `respect_gpc` | bool | `false` | `--beacon.respect_gpc` | Send nothing from a browser with Global Privacy Control or Do Not Track on. |

## Read more

- [`docs/analytics.md`](../analytics.md)
- [brocanth-analytics](../../crates/brocanth-analytics/README.md)
- Source: [`crates/brocanth-analytics/src/scaffold.rs`](../../crates/brocanth-analytics/src/scaffold.rs)
