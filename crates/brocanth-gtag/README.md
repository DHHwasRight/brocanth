# brocanth-gtag

Google Analytics 4 behind a consent banner. A response hook puts a banner
and a small consent script into every HTML page. In `basic` mode, the
default, Google's tag loads only after the visitor accepts. In `advanced`
mode, it loads on the first page with every consent denied and sends
cookieless pings before any choice. Global Privacy Control and Do Not
Track count as a refusal. Before a page view leaves, the URL loses its
fragment, every query parameter that is not on an allow-list, and anything
that holds an `@`. A generated project keeps the tag off its admin; by
hand, call `.exclude_path("/admin")` on the configuration.

Use it for a site that wants visit counts from Google Analytics and must
ask first, as the GDPR and the ePrivacy rules require in the EU. Do not use
it when you need no third party at all: the `analytics` module counts
visits on your own server, with no script, no cookie and no banner (see
[`docs/analytics.md`](../../docs/analytics.md)). The table in
[`docs/gtag.md`](../../docs/gtag.md) compares the two.

This crate does not make a site compliant on its own. The site owner is the
controller. They still need a privacy policy, a data processing agreement
with Google, and a decision about transfers to the US. See
[`docs/gtag.md`](../../docs/gtag.md).

## Getting started

In a generated project:

```text
brocanth add gtag
```

Or by hand:

```toml
[dependencies]
brocanth-gtag = { git = "https://github.com/DHHwasRight/brocanth.git" }
```

The module is off until a deployment turns it on, so a developer's visits
are not counted:

| Setting | Default | Flag | Environment |
|---|---|---|---|
| `enabled` | `false` | `--gtag.enabled` | `$<APP>_GTAG__ENABLED` |
| `measurement_id` | empty | `--gtag.measurement_id` | `$<APP>_GTAG__MEASUREMENT_ID` |
| `consent_mode` | `basic` | `--gtag.consent_mode` | `$<APP>_GTAG__CONSENT_MODE` |
| `respect_gpc` | `true` | `--gtag.respect_gpc` | `$<APP>_GTAG__RESPECT_GPC` |
| `consent_days` | `182` | `--gtag.consent_days` | `$<APP>_GTAG__CONSENT_DAYS` |
| `send_title` | `false` | `--gtag.send_title` | `$<APP>_GTAG__SEND_TITLE` |
| `allow_params` | the five `utm_*` | file only | by index, with `config` |
| `exclude` | `[]` (a generated project adds the admin's path) | file only | by index, with `config` |
| `cookie_domain` | none | `--gtag.cookie_domain` | `$<APP>_GTAG__COOKIE_DOMAIN` |
| `consent_log` | none | `--gtag.consent_log` | `$<APP>_GTAG__CONSENT_LOG` |
| `consent_log_retention_days` | `395` | `--gtag.consent_log_retention_days` | `$<APP>_GTAG__CONSENT_LOG_RETENTION_DAYS` |
| `banner.text`, `banner.accept`, `banner.refuse`, `banner.manage`, `banner.policy_url` | English | `--gtag.banner.*` | `$<APP>_GTAG__BANNER__*` |

With the `config` module, the settings live under `[gtag]` in
`config/<env>.toml`. Turn the module on in `config/production.toml` only.

Features:

| Feature | Default | What it adds |
|---|---|---|
| `runtime` | on | the `Gtag` plugin and its hooks, with the core and `http` |
| `config` | off | `Deserialize` on the configuration types, for a `[gtag]` section |
| `scaffold` | off | the `brocanth create` module; the CLI takes it with `default-features = false` |

## Consent stays in the browser, unless you log it

The choice lives in the visitor's `localStorage`. GDPR Article 7(1) asks
the controller to be able to demonstrate consent, and a browser-only
choice demonstrates nothing. On a site with a server, set `consent_log` to
a file: each choice is then appended as one JSON line, with no address and
no user agent. A static site has no server to receive the report, so it
cannot keep such a record.

The script reports to `GET /__consent`. It answers only a request from the
site's own pages, and it rotates the file past 16 MiB, but anyone can send
it a request: give `/__consent` a rule in the rate limiter. The file is per
instance, and a container loses it when it stops, unless the path is on a
volume that outlives it.

The log keeps a line for `consent_log_retention_days`, 395 by default
(about 13 months). The writer drops older lines at the start and once a
day, from the file and from `<file>.1`. Decision: the log rotates by age
as well as by size. A consent record must outlive the choice it proves
(`consent_days`, six months by default) and its renewal, but a record
kept with no end is personal data with no purpose. Thirteen months is a
common retention for consent records. Set a longer value when your
regulator or your counsel asks for one.

```rust
use brocanth_gtag::{parse_choice, GtagConfig};

let mut config = GtagConfig::default().measuring("G-ABC123");
config.consent_log = Some("log/consent.jsonl".into());
config.cookie_domain = Some("example.co.uk".into());
assert!(config.check().is_ok());

// What the script sends after a click, read strictly.
let choice = parse_choice("c=granted&id=0123456789abcdef&v=1a2b").expect("a choice");
assert_eq!(choice.choice, "granted");
assert!(parse_choice("c=granted&id=0123456789abcdef&v=1a2b&ip=1.2.3.4").is_none());
```

## Examples

Register the plugin and serve a page. Off by default: the page is
unchanged.

```rust
use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth_gtag::{Gtag, GtagConfig};

let config = GtagConfig::default()
    .measuring("G-ABC123")
    // The default excludes nothing: keep the tag off the admin.
    .exclude_path("/admin");
let app = AppBuilder::new()
    .plugin_hooking(Gtag::new(config))
    .build()
    .expect("an application");
# let _ = app;
```

The markup the hook inserts before `</body>`: a hidden banner, the
settings as JSON, and the consent script. No Google URL is in a `src`
attribute; the script adds one only after consent.

```rust
use brocanth_gtag::GtagConfig;

let markup = GtagConfig::default().measuring("G-ABC123").markup();
assert!(markup.contains(r#"id="brocanth-consent""#));
assert!(markup.contains(r#""id":"G-ABC123""#));
assert!(!markup.contains(r#"src="https://www.googletagmanager.com"#));
```

What a page view may carry. The browser script does the same; this
function is its specification.

```rust
use brocanth_gtag::{scrub, GtagConfig};

let allow = GtagConfig::default().allow_params;
assert_eq!(
    scrub("https://shop.example/u/ann@example.com?utm_source=news&token=s3cret#x", &allow),
    "https://shop.example/u/redacted?utm_source=news",
);
```

Allow the script and Google's origins in the Content-Security-Policy of
`brocanth-middleware`. The generated project does this for you when it
has `middleware-security-headers`.

```rust
use brocanth_middleware::SecurityHeadersConfig;

let headers = SecurityHeadersConfig::default()
    .page_script(brocanth_gtag::SCRIPT, brocanth_gtag::CSP);
let policy = headers.csp.render(&headers.frames).expect("a policy");
assert!(policy.contains("https://www.googletagmanager.com"));
```

## Reopening the banner

The hook adds a small "Cookie settings" button on every page. Any element
with `data-consent-manage` also reopens the banner, for a link in the site
footer:

```html
<a href="#" data-consent-manage>Cookie settings</a>
```

## Testing

```text
cargo test -p brocanth-gtag
```

The tests drive the hook with `brocanth-test`'s `TestClient` and check
what a browser receives: no Google script before consent, no markup under
Global Privacy Control or on an excluded path, and a policy that lets the
tag load.
`tests/consent_js.rs` runs the consent script under Node when `node` is on
the `PATH`, and skips otherwise.

To test a page of your own application, send a request and read the body:

```rust,ignore
let page = client.send(TestRequest::get("/").header("sec-gpc", "1"));
assert!(!page.text().contains("brocanth-consent"));
```

## More

- [`docs/gtag.md`](../../docs/gtag.md): the design, what it can
  and cannot guarantee, and the sources.
- [`docs/modules/gtag.md`](../../docs/modules/gtag.md): the
  module page.
- [`brocanth-test`](../brocanth-test/README.md): the testing guide.
