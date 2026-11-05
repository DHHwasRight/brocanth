# `analytics`

Page views counted on the server: no cookie, no script, no third party; rows in your Postgres.

The wizard calls it: *Count page views on the server: no cookie, no script, no third party*.

| | |
|---|---|
| Categories | **Middleware** |
| Wizard question | Operations |
| Default | off |
| Presets | none |
| Part of | nothing |
| Needs | [`db`](db.md): the views are rows in the database the `db` module opens |
| After | [`db`](db.md) |
| Adapts to | [`config`](config.md) |
| Conflicts | [`beacon`](beacon.md): both would count one page: the hook on the server, and the beacon in the browser; [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-analytics` (feature `config` with `config`, `admin` with `admin`) |
| Without it | no page-view counts; nothing is measured, so nothing needs consent |

With [`admin`](admin.md), the admin module adds a tab at `/admin/analytics`
(`admin` adapts to `analytics`).

## What it writes

Files: none.

Slots:

- src/plugins/mod.rs (the builder chain): `plugin_hooking_from_config::<brocanth_analytics::Analytics>("analytics")`
- src/plugins/mod.rs (the provider list): `analytics brocanth_analytics::Analytics`
- src/plugins/mod.rs (the hook record): `brocanth_analytics::Analytics`
- config/<env>.toml: `[analytics]`
- AGENTS.md: `## Analytics`

## What the plugin does

- A request hook, after dispatch. A GET answered 2xx with `text/html` is one
  page view, unless its path is excluded, its user agent reads as a bot, or
  it is a prefetch. A dp-swap partial counts too: it replaces a full load.
- It stores no address, no user agent, no time of day and no cookie. The
  visitor is 8 bytes of `SHA-256(salt of the day ‖ SHA-256(site ‖ address ‖
  user agent))`. The salt is random, shared through the database, and
  deleted once the day is over.
- The hook only queues. A task writes the queue every `flush_interval`
  seconds. A failed write keeps its batch for the next try and never
  reaches a request.
- With `collector_origins`, a second hook answers `collector_path` for a
  static site's [`beacon`](beacon.md): it counts a beacon from a listed
  origin with the same rules, and always answers 204 or a 1×1 GIF. A beacon
  row's site is its origin's host, so each static site is a site of its
  own on the dashboard.
- `before_start` takes the `brocanth_db::Db` the `db` plugin put in the
  services. With `migrate`, the first readiness check creates the tables.

## Settings

Section `[analytics]`. Environment variable: `$<APP>_ANALYTICS__<KEY>`. Flag: `--analytics.<key>`. Rename a flag in `src/flags.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `enabled` | bool | `false` (the development file; the plugin's default is `true`) | `--analytics.enabled` | Count page views. Off installs no hook. |
| `site` | text | the project name | `--analytics.site` | The site name on the rows this server counts, and part of the visitor hash. A beacon row takes its origin's host. |
| `exclude` | list | `["/admin"]` | none (list) | Paths never counted, each with everything under it. |
| `query_allow` | list | the five `utm_*` | none (list) | Query parameters kept on the stored path. Every other one is dropped. |
| `retention_days` | integer | `760` | `--analytics.retention_days` | Days a row is kept. 0 keeps rows forever. |
| `flush_interval` | integer | `10` | `--analytics.flush_interval` | Seconds between two writes of the buffer. |
| `respect_gpc` | bool | `false` | `--analytics.respect_gpc` | Skip a request that sends Sec-GPC: 1 or DNT: 1. |
| `ip_header` | text, optional | commented out | `--analytics.ip_header` | A header the edge sets to the client address. Unset, the socket's. |
| `country_header` | text, optional | commented out | `--analytics.country_header` | A header the edge sets to a two-letter country code. Unset, no country. |
| `migrate` | bool | `true` | `--analytics.migrate` | Create the two tables on the first readiness check. |
| `collector_path` | text | `"/_a/v"` | `--analytics.collector_path` | The path that counts page views a static site sends with the `beacon` module. |
| `collector_origins` | list | `[]` | none (list) | The static sites allowed to send them: "https://docs.example.com". Empty, no collector. |

## Read more

- [`docs/analytics.md`](../analytics.md): the design and the privacy reasoning
- [brocanth-analytics](../../crates/brocanth-analytics/README.md)
- Source: [`crates/brocanth-analytics/src/scaffold.rs`](../../crates/brocanth-analytics/src/scaffold.rs)
