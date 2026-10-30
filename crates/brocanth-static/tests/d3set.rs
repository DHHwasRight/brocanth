//! What the d3set chart gallery (github.com/DHHwasRight/d3set, `rust/gallery`)
//! asks of this crate, with a handler that answers the way its pages do.
//!
//! The gallery is a brocanth application with two layout layers: the shell
//! (`dp-layout="/"`) and a section (`dp-layout="/charts"`). It exports to
//! GitHub Pages under `/d3set`, with dp-swap's `partialDir: "partials"`,
//! and serves recorded feeds as JSON (`/data/sales.json`) that the charts
//! poll. If this test has to change, the gallery's `export.rs` probably
//! has to change too.

use std::path::{Path, PathBuf};

use brocanth::routing::{Router, Verb};
use brocanth_server::{Request, Response};
use brocanth_static::{Export, ExportError, Report};

/// The gallery's routes: two without a parameter, two with one.
fn router() -> Router<()> {
    let mut builder = Router::builder();
    for pattern in ["/", "/guide", "/charts/{kind}", "/data/{file}"] {
        builder.route(pattern, Verb::Get, ()).unwrap();
    }
    builder.route("/subscribe", Verb::Post, ()).unwrap();
    builder.build().unwrap()
}

/// A page the way `html!` writes it: the layers the client does not hold.
fn page(path: &str, holding: u32) -> String {
    let section = path.starts_with("/charts/");
    let body = format!(r#"<h1>{path}</h1><a href="/charts/line">Line</a><img src="/favicon.svg">"#);
    let section_layer =
        format!(r#"<div dp-layout="/charts"><nav><a href="/charts/bar">Bar</a></nav>{body}</div>"#);
    let inner = if section { section_layer } else { body };
    match holding {
        0 => format!(
            r#"<!DOCTYPE html><html><head><link rel="stylesheet" href="/css/app.css"></head><body><main id="app" dp-layout="/">{inner}</main><script type="application/json">{{"url":"/d3set/data/sales.json"}}</script><script src="/dpswap.min.js"></script></body></html>"#
        ),
        // Depth 1 keeps the shell; a page with no section answers the
        // same for every depth past its own layers.
        1 => inner,
        _ if section => format!(r#"<h1>{path}</h1><a href="/charts/line">Line</a>"#),
        _ => inner,
    }
}

async fn handle(request: Request) -> Response {
    let holding = request
        .header("dp-partial")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    match request.path.as_str() {
        "/data/sales.json" => Response::json(
            200,
            r#"{"interval":2000,"frames":[[{"url":"/not/rewritten"}]]}"#,
        ),
        "/" | "/guide" | "/charts/bar" | "/charts/line" => {
            Response::html(200, page(&request.path, holding))
        }
        _ => Response::html(404, r#"<h1>Not found</h1><a href="/">Home</a>"#),
    }
}

fn out(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("brocanth-static-{name}-{}", std::process::id()))
}

fn read(root: &Path, file: &str) -> String {
    std::fs::read_to_string(root.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"))
}

async fn export(name: &str) -> (PathBuf, Report) {
    let root = out(name);
    let report = Export::new(&root)
        .base("/d3set/")
        .partials(2)
        .routes(&router())
        .paths(["/charts/bar", "/charts/line", "/data/sales.json", "/"])
        .assets([
            ("/css/app.css", &b"@font-face{src:url(/fonts/a.woff2)}"[..]),
            ("/js/app.js", &b"import('/js/chunk.js')"[..]),
        ])
        .run(handle)
        .await
        .unwrap();
    (root, report)
}

#[tokio::test]
async fn the_gallery_exports_documents_partials_files_and_assets() {
    let (root, report) = export("all").await;
    assert_eq!(
        report,
        Report {
            pages: 4,
            partials: 8,
            files: 1,
            assets: 2,
            not_found: true,
            skipped: vec!["/charts/{kind}".into(), "/data/{file}".into()],
        }
    );
    for file in [
        "index.html",
        "guide/index.html",
        "charts/bar/index.html",
        "partials/1/index.html",
        "partials/2/index.html",
        "partials/1/charts/bar/index.html",
        "partials/2/charts/line/index.html",
        "data/sales.json",
        "404.html",
        "css/app.css",
        "js/app.js",
    ] {
        assert!(root.join(file).is_file(), "{file} missing");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn each_depth_leaves_out_its_layers() {
    let (root, _) = export("depth").await;
    let document = read(&root, "charts/bar/index.html");
    let one = read(&root, "partials/1/charts/bar/index.html");
    let two = read(&root, "partials/2/charts/bar/index.html");
    assert!(document.contains(r#"<main id="app""#));
    assert!(!one.contains(r#"<main id="app""#) && one.contains("dp-layout"));
    assert!(!two.contains("dp-layout") && two.contains("<h1>/charts/bar</h1>"));
    // A page with one layer answers depth 2 like depth 1, and both exist.
    assert_eq!(
        read(&root, "partials/1/guide/index.html"),
        read(&root, "partials/2/guide/index.html")
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn html_and_css_carry_the_base_and_data_does_not() {
    let (root, _) = export("base").await;
    let document = read(&root, "charts/bar/index.html");
    for moved in [
        r#"href="/d3set/css/app.css""#,
        r#"dp-layout="/d3set/""#,
        r#"dp-layout="/d3set/charts""#,
        r#"href="/d3set/charts/line""#,
        r#"src="/d3set/favicon.svg""#,
        r#"src="/d3set/dpswap.min.js""#,
    ] {
        assert!(document.contains(moved), "{moved} not in {document}");
    }
    // The payload was prefixed by the application, and stays as it was.
    assert!(document.contains(r#"{"url":"/d3set/data/sales.json"}"#));
    assert!(read(&root, "partials/1/charts/bar/index.html").contains(r#"href="/d3set/charts/bar""#));
    assert!(read(&root, "404.html").contains(r#"href="/d3set/""#));
    assert!(read(&root, "data/sales.json").contains(r#""url":"/not/rewritten""#));
    assert_eq!(
        read(&root, "css/app.css"),
        "@font-face{src:url(/d3set/fonts/a.woff2)}"
    );
    assert_eq!(read(&root, "js/app.js"), "import('/js/chunk.js')");
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn a_page_that_does_not_answer_stops_the_export() {
    let error = Export::new(out("missing"))
        .paths(["/charts/area"])
        .run(handle)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        ExportError::Status {
            status: 404,
            depth: None,
            ..
        }
    ));
    assert!(error.to_string().contains("help:"));
    let _ = std::fs::remove_dir_all(out("missing"));
}

#[tokio::test]
async fn a_partial_that_does_not_answer_names_its_depth() {
    let error = Export::new(out("partial"))
        .partials(1)
        .paths(["/"])
        .run(|request: Request| async move {
            if request.header("dp-partial").is_some() {
                Response::html(500, "no")
            } else {
                Response::html(200, "<p>ok</p>")
            }
        })
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        ExportError::Status {
            depth: Some(1),
            status: 500,
            ..
        }
    ));
    let _ = std::fs::remove_dir_all(out("partial"));
}

#[tokio::test]
async fn a_path_with_a_query_is_refused() {
    let error = Export::new(out("query"))
        .paths(["/?js=off"])
        .run(handle)
        .await
        .unwrap_err();
    assert!(matches!(error, ExportError::Path(_)));
}

#[tokio::test]
async fn no_base_writes_the_pages_as_they_are() {
    let root = out("nobase");
    Export::new(&root)
        .paths(["/charts/bar"])
        .run(handle)
        .await
        .unwrap();
    assert_eq!(read(&root, "charts/bar/index.html"), page("/charts/bar", 0));
    assert!(!root.join("partials").exists());
    std::fs::remove_dir_all(root).unwrap();
}
