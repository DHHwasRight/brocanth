# `gtag`

Google Analytics 4 behind a consent banner; nothing leaves before the visitor accepts.

The wizard calls it: *Count visits with Google Analytics, after consent*.

| | |
|---|---|
| Categories | **Middleware** |
| Wizard question | Operations |
| Default | off |
| Presets | offered unticked in "A static site" |
| Part of | none |
| Conflicts | none |
| Outside the runtime | yes: the hook runs in the static export too |
| Crates | `brocanth-gtag` |
| Without it | no Google tag; nothing on a page talks to Google |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/gtag.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | none |
| Requires | none |
| Uses | `Mount` (a mounted path): a `Backoffice` mount, such as the admin, never gets the tag, at the path its module file mounts it |
| Adapts to | [`i18n`](i18n.md): the banner's words come from the catalogs, in the request's locale |
| Contributes | `AgentsSection` `gtag`: the consent banner, and that no tag loads without consent |
| Collects | none |
| Exports | none |
| Hooks | `consent-log` (Guard, required): record each consent choice, while `consent_log` names a file; `insert` (Identify, required): put the banner and the tag into each HTML page |
| Options (build time) | none |
| Settings (run time) | `[gtag]`: `enabled`, `measurement_id`, `consent_mode`, `respect_gpc`, `consent_days`, `send_title`, `allow_params`, `exclude`, `cookie_domain`, `consent_log`, `consent_log_retention_days`, `banner.text`, `banner.accept`, `banner.refuse`, `banner.manage`, `banner.policy_url`, `banner.policy` |
<!-- end generated: capabilities -->

With [`i18n`](i18n.md), the banner's words come from the catalogs, in the
request's locale. `integrate.i18n: false` in `src/modules/gtag.rs` keeps the
`banner.*` settings in every locale.

Hooks: `gtag.insert` and `gtag.consent-log`. Neither has a field in
`src/modules/gtag.rs`. `consent-log` exists only while the `consent_log`
setting names a file. **Reason:** the banner's script reports a choice
exactly when that setting is set, so a hook turned off under a set path
would leave the script reporting to nothing.

## What it writes

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs (the provider list): `gtag brocanth_gtag::Gtag`
- src/plugins/mod.rs (the hook record): `brocanth_gtag::Gtag`
- config/<env>.toml: `[gtag]`
- AGENTS.md: `## Google Analytics`

No template and no route. A response hook puts the banner, the settings
as JSON and the consent script before `</body>` of every HTML page, so
`rsx`, `jt`, a hand-written handler and `static`'s export all get it.

With [`middleware-security-headers`](middleware-security-headers.md), the
policy allows the consent script by its hash and adds Google's origins to
`script-src`, `img-src` and `connect-src`. That adaptation is declared on
the security headers' side.

With [`i18n`](i18n.md), the banner follows the request's locale. The
crate's `i18n` feature is on, and the hook reads `brocanth_i18n::Current`
from the request state. Its words are the `[gtag.banner]` keys of
`locales/*.toml`, which the `i18n` module writes. A key no catalog has
keeps the `banner.*` setting.

## Settings

Section `[gtag]`. Environment variable: `$<APP>_GTAG__<KEY>` (a dot in the key is `__`). Flag: `--gtag.<key>`. Rename or hide a flag in `src/modules/gtag.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `enabled` | bool | `false` | `--gtag.enabled` | Insert the consent banner and the Google tag into every HTML page. Off outside production, so a developer's visits are not counted. |
| `measurement_id` | text | `""` | `--gtag.measurement_id` | The GA4 measurement id, `G-` then capitals and digits. |
| `consent_mode` | text | `"basic"` | `--gtag.consent_mode` | `basic`: no Google script before consent. `advanced`: cookieless pings before it. |
| `respect_gpc` | bool | `true` | `--gtag.respect_gpc` | Treat Global Privacy Control and Do Not Track as a refusal. |
| `consent_days` | integer | `182` | `--gtag.consent_days` | How long a choice is kept before the banner asks again, in days. Six months is what the CNIL recommends. |
| `send_title` | bool | `false` | `--gtag.send_title` | Send the page title. Off: the scrubbed path stands in for it. |
| `allow_params` | list | the five `utm_*` | none (a list) | The query parameters that reach Google; every other one is removed. |
| `exclude` | list | `[]` | none (a list) | Paths that never get the tag, each with everything under it. With `admin`, the registration adds the admin's `mount_at`, read from `src/modules/admin.rs`. |
| `cookie_domain` | text | `""` | `--gtag.cookie_domain` | The domain Google's cookies live on, such as `example.co.uk`; none lets the tag pick it. Empty means none. |
| `consent_log` | text | `""` | `--gtag.consent_log` | A file each consent choice is appended to, such as `log/consent.jsonl`. Empty means none. A server only: a static export has no one to report a choice to. |
| `consent_log_retention_days` | integer | `395` | `--gtag.consent_log_retention_days` | How long a line stays in the consent log, in days. About 13 months. The server drops older lines at the start and once a day: a consent record must outlive the choice it proves, but not keep personal data with no purpose. |
| `banner.text` | text | English | `--gtag.banner.text` | The sentence the banner asks with. |
| `banner.accept` | text | `"Accept"` | `--gtag.banner.accept` | The button that accepts. |
| `banner.refuse` | text | `"Refuse"` | `--gtag.banner.refuse` | The button that refuses. |
| `banner.manage` | text | `"Cookie settings"` | `--gtag.banner.manage` | The button that opens the banner again. |
| `banner.policy_url` | text | `""` | `--gtag.banner.policy_url` | A link to the privacy policy, such as `/privacy`. Empty means none. |
| `banner.policy` | text | `"Privacy policy"` | `--gtag.banner.policy` | The text of that link. |

## Read more

- [`analytics`](analytics.md): first-party counts on your own server, with no script, no cookie and no banner.

- [Google Analytics](../gtag.md): the design, what it guarantees and what it does not, and the sources.
- [`brocanth-gtag`](../../crates/brocanth-gtag/README.md)
- Source: [`crates/brocanth-gtag/src/scaffold.rs`](../../crates/brocanth-gtag/src/scaffold.rs)
