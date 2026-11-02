# brocanth-static

Writes a brocanth application to a directory of files that any static host
serves: GitHub Pages, an S3 bucket, nginx. `Export` sends each GET path
through the application's own handler, hooks included, and writes each answer
to a file. It also writes one partial per layout depth for dp-swap, a
`404.html`, and the embedded assets, and it can prefix every root-relative URL
with a base path. Use it when a site has no request-time state. Do not use it
for pages that read a cookie, the query, a session or a database: the export
sends a bare GET, and nothing runs behind the files. The crate depends only on
`brocanth` and `brocanth-server`, and starts no runtime: `Export::run` is a
future that the caller drives.

## Getting started

```toml
[dependencies]
brocanth-static = { git = "https://github.com/DHHwasRight/brocanth.git" }
```

The crate has no features. In a generated project, the `static` module adds
it:

```sh
brocanth create my-site --with commands,static
brocanth add static        # in a project that has `commands`
```

The module writes `src/commands/export.rs` (a thin caller of this crate), a
`/dist` line in `.gitignore`, and a `mise run export` task. It needs the
`commands` module, and refuses to be written without it. It declares no config settings, so it has no
`--static.<key>` flag and no `$<APP>_STATIC__<KEY>` variable. The `export`
command takes two arguments:

| Argument | Default | Meaning |
|---|---|---|
| `--out` | `dist` | the directory to write; emptied first |
| `--base` | empty | the path the site is served under, such as `/my-site` |

Two constants in `src/commands/export.rs` are yours to edit: `PATHS`, the
concrete paths of routes with a parameter, and `PARTIALS`, the most layout
layers a page has.

`static` refuses every module that does not run outside the runtime (a
session, a database, a queue, the probes). `create` and `add` both apply that
rule. The crate does not: an application that calls it directly decides for
itself.

## Examples

### Export a site

```rust
use brocanth::routing::{Router, Verb};
use brocanth_server::{Request, Response};
use brocanth_static::Export;

async fn handle(request: Request) -> Response {
    match request.path.as_str() {
        "/" => Response::html(200, r#"<a href="/about">About</a>"#),
        "/about" => Response::html(200, "<h1>About</h1>"),
        "/users/7" => Response::html(200, "<h1>User 7</h1>"),
        _ => Response::html(404, "<h1>Not found</h1>"),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut routes = Router::builder();
    for pattern in ["/", "/about", "/users/{id}"] {
        routes.route(pattern, Verb::Get, ())?;
    }
    let router = routes.build()?;

    let out = std::env::temp_dir().join(format!("brocanth-static-readme-{}", std::process::id()));
    let report = Export::new(&out)
        .routes(&router)            // every GET route with no parameter
        .paths(["/users/7"])        // the routes with one, by concrete path
        .assets([("/css/app.css", &b"body{margin:0}"[..])])
        .run(handle)
        .await?;

    assert_eq!(report.pages, 3);
    assert!(report.not_found);
    assert_eq!(report.skipped, ["/users/{id}"]);
    assert!(out.join("index.html").is_file());
    assert!(out.join("about/index.html").is_file());
    assert!(out.join("users/7/index.html").is_file());
    assert!(out.join("404.html").is_file());
    assert!(out.join("css/app.css").is_file());
    println!("export: {report}");

    std::fs::remove_dir_all(&out)?;
    Ok(())
}
```

A path that answers anything but 200, or answers a stream, stops the export
with an `ExportError` that names the path and says what to do.

### A base path, and partials per depth

`base("/repo")` serves the site under `/repo/`, as a GitHub Pages project
site is. `partials(n)` writes `partials/<depth>/<route>/index.html` for each
depth `1..=n`, requested with `DP-Partial: <depth>`.

```rust
use brocanth_server::{Request, Response};
use brocanth_static::{Export, PARTIAL_HEADER};

async fn handle(request: Request) -> Response {
    // The page, without the layout layers the client already holds.
    let held: u32 = request.header(PARTIAL_HEADER).and_then(|v| v.parse().ok()).unwrap_or(0);
    let page = r#"<a href="/about">About</a>"#;
    match (request.path.as_str(), held) {
        ("/", 0) => Response::html(200, format!(r#"<main dp-layout="/">{page}</main>"#)),
        ("/", _) => Response::html(200, page),
        _ => Response::html(404, "<h1>Not found</h1>"),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::env::temp_dir().join(format!("brocanth-static-base-{}", std::process::id()));
    let report = Export::new(&out)
        .base("/repo")
        .partials(1)
        .paths(["/"])
        .run(handle)
        .await?;
    assert_eq!(report.partials, 1);

    let index = std::fs::read_to_string(out.join("index.html"))?;
    assert_eq!(index, r#"<main dp-layout="/repo/">"#.to_owned() + r#"<a href="/repo/about">About</a></main>"#);
    let partial = std::fs::read_to_string(out.join("partials/1/index.html"))?;
    assert_eq!(partial, r#"<a href="/repo/about">About</a>"#);

    std::fs::remove_dir_all(&out)?;
    Ok(())
}
```

The rewrite covers URL attributes (`brocanth_static::rebase::ATTRIBUTES`),
`url(...)` and `@import` in CSS. It never changes `<script>` contents, JSON
or JavaScript: a URL inside data is the application's to prefix.

### Where a path is written

```rust
use std::path::Path;

use brocanth_static::file_for;

assert_eq!(file_for("/"), Path::new("index.html"));
assert_eq!(file_for("/about"), Path::new("about/index.html"));
assert_eq!(file_for("/data/sales.json"), Path::new("data/sales.json"));
```

## Testing

```sh
cargo test -p brocanth-static
```

No environment variable is needed. The tests write under the system temp
directory. `tests/d3set.rs` mirrors what the d3set chart gallery
(github.com/DHHwasRight/d3set) needs: a base path, two layout layers and JSON
feeds. If that test must change, the gallery probably must change too: say so
in the commit.

To test an application's export, call `Export::run` with the same handler the
server uses, as above, and read the files it wrote. To test one page, use
`brocanth-test`'s `TestClient`, and send no cookie and no query, as the export
does.

## Read more

- `docs/static-export.md`: the full output table, partials per depth, the
  base path rewrite, and the rule that `static` excludes runtime modules.
- `docs/modules.md`, "Static output": `Manifest::static_output` and
  `Manifest::outside_runtime`.
- dp-swap (github.com/brocanth/htmx): the client that reads the partials
  through `partialDir`.
