# brocanth-analytics

First-party page-view counting for a brocanth application. A request
hook sees each finished response. When it was a page view, the hook
reduces the request to a few coarse fields and queues them. A task
writes the queue to your own Postgres every few seconds.

There is no cookie, no script in the page, and no third party. No IP
address and no user agent is stored. A visitor is a hash that changes
every day, and the salt that made it is deleted when the day ends.

Use it to answer "how many people read this page, and where did they
come from". Do not use it to follow one person: it cannot, by design.
The hook runs in the server, so a static site served by another host is
counted through a collector on this server, fed by the `beacon` module's
snippet in the exported pages (below).

## Getting started

In a generated project:

```text
brocanth create my-site --with db,analytics
brocanth add analytics        # in a project that has `db`
```

`analytics` needs `db`: the views go in two tables of that database. With
`admin`, the numbers are a tab at `/admin/analytics`.

By hand:

```toml
[dependencies]
brocanth-analytics = { git = "https://github.com/DHHwasRight/brocanth.git", features = ["config", "admin"] }
```

Features: `runtime` (the plugin, the hook, the collector, the beacon and
the stores) and `postgres` are on by default; `config` and `admin` are off.
The `brocanth` CLI takes the crate with `default-features = false,
features = ["scaffold"]`, so the generator links none of the runtime.

The plugin is registered after the `db` plugin, so it finds the pool in
the application's services:

```rust,no_run
# use brocanth::plugin_app::AppBuilder;
# use brocanth::plugin_api::Plugin;
use brocanth_analytics::{Analytics, AnalyticsConfig};

let app = AppBuilder::new()
    // .plugin(…the db plugin…)
    .plugin_hooking(Analytics::new(AnalyticsConfig {
        site: "my-site".to_string(),
        ..AnalyticsConfig::default()
    }))
    .build()
    .expect("an application");
# let _ = app;
```

## Settings

`[analytics]` in `config/<env>.toml`, `$<APP>_ANALYTICS__<KEY>`, or
`--analytics.<key>`:

| Key | Default | Meaning |
|---|---|---|
| `enabled` | `true` (`false` in the generated development file) | Count page views. `false` installs no hook. |
| `site` | the project name | The site on every row, and part of the hash. |
| `exclude` | `["/admin"]` | Paths never counted, with everything under them. File only. |
| `query_allow` | the five `utm_*` | Query parameters kept on the path. File only. |
| `retention_days` | `760` | Days a row is kept (25 months). `0` keeps rows forever. |
| `flush_interval` | `10` | Seconds between two writes. |
| `respect_gpc` | `false` | Skip a request with `Sec-GPC: 1` or `DNT: 1`. |
| `ip_header` | unset | A header the edge sets to the client address. |
| `country_header` | unset | A header the edge sets to a country code, such as `cf-ipcountry`. |
| `migrate` | `true` | Create the two tables on the first readiness check. |
| `collector_path` | `/_a/v` | The path that counts what a static site's beacon sends. |
| `collector_origins` | `[]` | Origins allowed to send beacons. Empty, no collector. File only. |

## Static sites: the beacon

A static host never runs the hook. A static site is counted by a brocanth
server with `analytics`:

1. On the server, list the site: `collector_origins = ["https://docs.example.com"]`
   under `[analytics]`. That installs a collector at `/_a/v`.
2. On the static site, choose the `beacon` module and set
   `collector = "https://app.example.com/_a/v"` under `[beacon]`. Every
   exported page then carries a short script that reports the view with
   `navigator.sendBeacon`, or a 1×1 image. No cookie, no storage.

The collector counts a beacon only from a listed origin, ignores payloads
over 2 KiB, and always answers 204 (or the image), never a 5xx.

```rust
use brocanth_analytics::beacon::insert;

let mut page = b"<html><body><h1>Docs</h1></body></html>".to_vec();
insert(&mut page, "<script>/* the beacon */</script>");
assert_eq!(page, b"<html><body><h1>Docs</h1><script>/* the beacon */</script></body></html>");
```

## Examples

What a request is reduced to:

```rust
use brocanth_analytics::classify::{clean_path, clean_query, referrer_origin, ua_class};

assert_eq!(clean_path("/unsubscribe/jane@example.com"), "/unsubscribe/_");

let allow = vec!["utm_source".to_string()];
assert_eq!(clean_query("token=s3cret&utm_source=news", &allow), "utm_source=news");

assert_eq!(
    referrer_origin("https://news.example/item?id=42", Some("my-site.example")).as_deref(),
    Some("https://news.example"),
);

let class = ua_class("Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1");
assert_eq!((class.browser, class.os, class.device), ("Safari", "iOS", "mobile"));
```

Counting through the whole pipeline, with a store in memory:

```rust
use std::sync::Arc;

use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_body::Body;
use brocanth::plugin_context::Base;
use brocanth::plugin_routes::{Handler, RouteCx};
use brocanth::routing::{Router, Verb};
use brocanth_analytics::{Analytics, AnalyticsConfig, MemoryStore};
use brocanth_test::{TestClient, TestRequest};

let page: Handler<Base> = Box::new(|_cx: &mut RouteCx<'_, Base>| {
    Ok(http::Response::builder()
        .header("content-type", "text/html; charset=utf-8")
        .body(Body::from(b"<h1>Hello</h1>".to_vec()))
        .unwrap())
});
let mut table = Router::builder();
table.route("/", Verb::Get, page).unwrap();

let store = Arc::new(MemoryStore::new());
let analytics = Analytics::with_store(AnalyticsConfig::default(), store.clone());
let recorder = analytics.recorder();
let app = AppBuilder::new().plugin_hooking(analytics).build().unwrap();
let client = TestClient::serving(app, table.build().unwrap());

client
    .send(TestRequest::get("/").header("user-agent", "Mozilla/5.0 (X11; Linux x86_64) Firefox/131.0"))
    .assert_ok();

let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
assert_eq!(runtime.block_on(recorder.flush()).unwrap(), 1);
assert_eq!(store.views()[0].path, "/");
assert_eq!(store.views()[0].browser, "Firefox");
```

## Testing

```text
cargo test -p brocanth-analytics
```

The Postgres tests are skipped unless `BROCANTH_TEST_DATABASE_URL` is set.
Each one creates its own schema and drops it.

In an application, build the plugin with `Analytics::with_store` and a
`MemoryStore`, send requests with `brocanth-test`'s `TestClient`, call
`recorder.flush()`, then assert on `store.views()`. `Analytics::with_clock`
moves the day.

## More

- `docs/analytics.md`: the design, the privacy reasoning and its sources,
  and what this crate does not guarantee.
- `docs/modules/analytics.md`: the module page.
