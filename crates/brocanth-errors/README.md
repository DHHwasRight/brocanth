# brocanth-errors

HTML error pages and error reporters for a brocanth project. A missing page
answers an HTML 404 inside the site's layouts, a failed page answers an HTML
500, a debug build shows the cause, and a release build never does. Another
module hears about every 5xx through the `Reporter` trait. Use it in a
project that serves pages to people. Do not use it for an API that answers
only JSON: a client that does not accept HTML keeps the plain answer anyway.
With `runtime` (the default) the crate links the core and `http`, which every
application links already.

## Getting started

```sh
brocanth create blog --with rsx,errors
brocanth add errors
```

The module writes `src/pages/not-found.rsx` and `src/pages/error.rsx` (with
`rsx`), `src/plugins/errors/mod.rs`, and the `errors` word in
`pages!("src/pages", errors)`. See `docs/modules/errors.md`.

By hand:

```toml
[dependencies]
brocanth-errors = { git = "https://github.com/DHHwasRight/brocanth.git" }
```

## The pages

`not-found.rsx` answers a 404 for every page in its directory and below it,
and for a path under the directory that no route matched. `error.rsx`
answers every other failure of a page in its directory and below it. The
nearest file wins, and the layouts above it wrap it. A component declares
`caught: &Caught` to read the status and, for a 4xx, the message:

```rust
use brocanth_errors::Caught;

let missing = Caught::new(404, "no user 7", "/users/7");
assert_eq!(missing.reason, "Not Found");
assert_eq!(missing.message.as_deref(), Some("no user 7"));

// A 5xx carries no message: the cause is for the log.
let broken = Caught::new(500, "connection refused", "/users/7");
assert_eq!(broken.message, None);
```

A project with no error page of its own gets `plain_page`. A debug build
shows `development_page` for a 5xx, outside the layouts, because the layout
can be what failed:

```rust
use brocanth_errors::{development_page, plain_page, Caught};

let caught = Caught::new(500, "invalid digit found in string", "/users/x");
assert!(!plain_page(&caught).contains("invalid digit"));

let dev = development_page(&caught, "invalid digit found in string", "GET", [("accept", "text/html")]);
assert!(dev.contains("invalid digit found in string"));
```

A client that does not read HTML, and an error that names its own
`content-type`, keep the plain answer:

```rust
use brocanth_errors::accepts_html;

assert!(accepts_html(Some("text/html,*/*;q=0.8")));
assert!(!accepts_html(Some("application/json")));
```

## Reporters

The `Errors` plugin publishes `Reporters` in the services, at `register`.
A plugin adds its reporter in `after_config`:

```rust
use brocanth_errors::{Report, Reporter, Reporters};

struct Stderr;

impl Reporter for Stderr {
    fn report(&self, report: &Report<'_>) {
        eprintln!("{} {} {}: {}", report.status, report.method, report.path, report.message);
    }
}

let reporters = Reporters::new(true);
reporters.add(Stderr);
assert_eq!(reporters.len(), 1);
```

A reporter hears every 5xx: a page's error (`Origin::Page`) and a hook's
error (`Origin::Hook`). It runs on the request's thread, so it must not
wait: give the report to a channel or a job and return.

## Settings

Section `[errors]`, type `ErrorsConfig`:

| Key | Default | What it does |
|---|---|---|
| `details` | `true` | A debug build shows the cause of a 5xx. A release build never does. |

## Features

| Feature | Adds |
|---|---|
| `runtime` (default) | the plugin, the pages, the reporter contract |
| `config` | `ErrorsConfig` deserialises from `[errors]` |
| `scaffold` | the `errors` module, for the CLI |

## Testing

```sh
cargo test -p brocanth-errors
```

No environment variable is needed.

## Read more

- `docs/modules/errors.md`: what the module writes.
- `brocanth-pages`: the `errors` word, and how a page reaches its error page.
