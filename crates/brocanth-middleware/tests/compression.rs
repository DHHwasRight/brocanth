//! Compression and conditional GET through the whole pipeline: what a
//! browser sends, what it gets back, and what it decodes.

use std::io::Read as _;
use std::task::{Context, Poll};

use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_body::{Body, BodyStream, Chunk};
use brocanth::plugin_context::{Base, NoContext};
use brocanth::plugin_hooks::{
    BodyShape, HookFamilies, HookRegistrar, HookSource, OnResponse, Request, RequestState,
    ResponseHead, ResponseHook,
};
use brocanth::plugin_order::Order;
use brocanth::plugin_routes::{Handler, RouteCx};
use brocanth::routing::{Router, Verb};
use brocanth_middleware::{Compression, CompressionConfig, ConditionalGet, ConditionalGetConfig};
use brocanth_test::{TestClient, TestRequest, TestResponse};

/// A page long enough to compress, and repetitive enough to shrink.
fn html() -> String {
    let rows: String = (0..200)
        .map(|i| format!("<li class=\"row\">Row number {i}</li>"))
        .collect();
    format!("<!DOCTYPE html><html><body><ul>{rows}</ul></body></html>")
}

fn respond(headers: &'static [(&'static str, &'static str)], body: Vec<u8>) -> Handler<Base> {
    Box::new(move |_cx: &mut RouteCx<'_, Base>| {
        let mut response = http::Response::builder();
        for (name, value) in headers {
            response = response.header(*name, *value);
        }
        Ok(response.body(Body::from(body.clone())).expect("a response"))
    })
}

/// Emits its chunks and ends, as an SSE endpoint does in a test.
struct Events(Vec<Vec<u8>>);

impl BodyStream for Events {
    fn poll_chunk(&mut self, _cx: &mut Context<'_>) -> Chunk {
        if self.0.is_empty() {
            Poll::Ready(None)
        } else {
            Poll::Ready(Some(Ok(self.0.remove(0))))
        }
    }
}

fn events() -> Handler<Base> {
    Box::new(|_cx: &mut RouteCx<'_, Base>| {
        let chunk = format!("data: {}\n\n", "x".repeat(4096)).into_bytes();
        Ok(http::Response::builder()
            .header("content-type", "text/event-stream")
            .body(Body::stream(Events(vec![chunk.clone(), chunk])))
            .expect("a response"))
    })
}

const HTML: &[(&str, &str)] = &[("content-type", "text/html; charset=utf-8")];

fn table() -> Router<Handler<Base>> {
    let page = html().into_bytes();
    let mut table = Router::builder();
    let routes: Vec<(&str, Handler<Base>)> = vec![
        ("/", respond(HTML, page.clone())),
        ("/small", respond(HTML, b"<p>hi</p>".to_vec())),
        (
            "/png",
            respond(&[("content-type", "image/png")], page.clone()),
        ),
        (
            "/done",
            respond(
                &[("content-type", "text/html"), ("content-encoding", "gzip")],
                page.clone(),
            ),
        ),
        (
            "/raw",
            respond(
                &[
                    ("content-type", "text/html"),
                    ("cache-control", "public, no-transform"),
                ],
                page.clone(),
            ),
        ),
        (
            "/tagged",
            respond(
                &[("content-type", "text/html"), ("etag", "\"v1\"")],
                page.clone(),
            ),
        ),
        (
            "/dated",
            respond(
                &[
                    ("content-type", "text/html"),
                    ("last-modified", "Sun, 06 Nov 1994 08:49:37 GMT"),
                ],
                page,
            ),
        ),
        ("/events", events()),
    ];
    for (path, handler) in routes {
        table.route(path, Verb::Get, handler).expect("a route");
    }
    table.build().expect("a table")
}

fn compressing(config: CompressionConfig) -> TestClient<Base, impl HookFamilies, Handler<Base>> {
    let app = AppBuilder::new()
        .plugin_hooking(Compression::new(config))
        .build()
        .expect("an application");
    TestClient::serving(app, table())
}

fn both() -> TestClient<Base, impl HookFamilies, Handler<Base>> {
    // Registered the check first, so the tie-break alone would run it last,
    // over the compressed bytes. The hooks' own order must put it before
    // compression on the way out.
    let app = AppBuilder::new()
        .plugin_hooking(ConditionalGet::new(ConditionalGetConfig::default()))
        .plugin_hooking(Compression::new(CompressionConfig::default()))
        .build()
        .expect("an application");
    TestClient::serving(app, table())
}

fn get(
    client: &TestClient<Base, impl HookFamilies, Handler<Base>>,
    path: &str,
    headers: &[(&str, &str)],
) -> TestResponse {
    let mut request = TestRequest::get(path);
    for (name, value) in headers {
        request = request.header(name, value);
    }
    client.send(request)
}

fn gunzip(bytes: &[u8]) -> String {
    let mut out = String::new();
    flate2::read::GzDecoder::new(bytes)
        .read_to_string(&mut out)
        .expect("a gzip body");
    out
}

fn unbrotli(bytes: &[u8]) -> String {
    let mut out = Vec::new();
    brotli::BrotliDecompress(&mut &bytes[..], &mut out).expect("a brotli body");
    String::from_utf8(out).expect("utf-8")
}

fn varies_on_encoding(response: &TestResponse) -> bool {
    response
        .header("vary")
        .is_some_and(|v| v.to_ascii_lowercase().contains("accept-encoding"))
}

#[test]
fn a_browser_gets_brotli_and_decodes_the_page() {
    let response = get(
        &compressing(CompressionConfig::default()),
        "/",
        &[("accept-encoding", "gzip, deflate, br, zstd")],
    );
    response.assert_ok();
    assert_eq!(response.header("content-encoding"), Some("br"));
    assert!(varies_on_encoding(&response));
    assert!(response.body().len() < html().len() / 4);
    assert_eq!(unbrotli(response.body()), html());
}

#[test]
fn a_client_that_takes_only_gzip_gets_gzip() {
    let response = get(
        &compressing(CompressionConfig::default()),
        "/",
        &[("accept-encoding", "gzip")],
    );
    assert_eq!(response.header("content-encoding"), Some("gzip"));
    assert_eq!(gunzip(response.body()), html());
}

#[test]
fn brotli_off_means_gzip_for_a_browser() {
    let config = CompressionConfig {
        brotli: false,
        ..Default::default()
    };
    let response = get(
        &compressing(config),
        "/",
        &[("accept-encoding", "gzip, br")],
    );
    assert_eq!(response.header("content-encoding"), Some("gzip"));
}

#[test]
fn no_accept_encoding_gets_the_page_as_it_is_and_still_varies() {
    let response = get(&compressing(CompressionConfig::default()), "/", &[]);
    assert_eq!(response.header("content-encoding"), None);
    assert_eq!(response.text(), html());
    assert!(
        varies_on_encoding(&response),
        "a shared cache must key on it"
    );
}

#[test]
fn what_must_not_be_compressed_is_not() {
    let client = compressing(CompressionConfig::default());
    let accept = [("accept-encoding", "gzip, br")];
    for path in ["/small", "/png", "/done", "/raw"] {
        let response = get(&client, path, &accept);
        let encoding = response.header("content-encoding");
        if path == "/done" {
            assert_eq!(encoding, Some("gzip"), "{path}: the handler's own");
            assert_eq!(response.text(), html(), "{path}: not compressed twice");
        } else {
            assert_eq!(encoding, None, "{path}");
        }
        assert!(!varies_on_encoding(&response), "{path}");
    }
}

#[test]
fn a_stream_is_never_compressed() {
    let response = get(
        &compressing(CompressionConfig::default()),
        "/events",
        &[("accept-encoding", "gzip, br")],
    );
    assert_eq!(response.header("content-encoding"), None);
    assert!(response.text().starts_with("data: xxx"));
    assert!(!varies_on_encoding(&response));
}

#[test]
fn a_strong_etag_becomes_weak_on_a_compressed_body() {
    let response = get(
        &compressing(CompressionConfig::default()),
        "/tagged",
        &[("accept-encoding", "gzip")],
    );
    assert_eq!(response.header("etag"), Some("W/\"v1\""));
}

#[test]
fn every_page_gets_a_weak_tag_that_survives_compression() {
    let client = both();
    let gzip = get(&client, "/", &[("accept-encoding", "gzip")]);
    let plain = get(&client, "/", &[]);
    let tag = gzip.header("etag").expect("a tag");
    assert!(tag.starts_with("W/\""), "{tag}");
    assert_eq!(
        plain.header("etag"),
        Some(tag),
        "one tag for both encodings"
    );
}

#[test]
fn a_matching_if_none_match_is_a_304_with_no_body_and_no_encoding() {
    let client = both();
    let first = get(&client, "/", &[("accept-encoding", "br")]);
    let tag = first.header("etag").expect("a tag").to_string();

    let again = get(
        &client,
        "/",
        &[("accept-encoding", "br"), ("if-none-match", &tag)],
    );
    assert_eq!(again.status(), 304);
    assert!(again.body().is_empty());
    assert_eq!(again.header("etag"), Some(tag.as_str()));
    assert_eq!(again.header("content-encoding"), None);
    assert_eq!(again.header("content-type"), None);

    // The tag from a brotli response is good for a gzip one too.
    let other = get(
        &client,
        "/",
        &[("accept-encoding", "gzip"), ("if-none-match", &tag)],
    );
    assert_eq!(other.status(), 304);
}

#[test]
fn a_stale_tag_gets_the_whole_page() {
    let response = get(&both(), "/", &[("if-none-match", "W/\"old\", \"older\"")]);
    response.assert_ok();
    assert_eq!(response.text(), html());
}

#[test]
fn a_handlers_own_tag_is_compared_weakly() {
    let response = get(&both(), "/tagged", &[("if-none-match", "W/\"v1\"")]);
    assert_eq!(response.status(), 304);
}

#[test]
fn last_modified_answers_if_modified_since() {
    let client = both();
    let same = get(
        &client,
        "/dated",
        &[("if-modified-since", "Sun, 06 Nov 1994 08:49:37 GMT")],
    );
    assert_eq!(same.status(), 304);
    let later = get(
        &client,
        "/dated",
        &[("if-modified-since", "Mon, 07 Nov 1994 08:49:37 GMT")],
    );
    assert_eq!(later.status(), 304);
    let earlier = get(
        &client,
        "/dated",
        &[("if-modified-since", "Sat, 05 Nov 1994 08:49:37 GMT")],
    );
    earlier.assert_ok();
    let unreadable = get(&client, "/dated", &[("if-modified-since", "yesterday")]);
    unreadable.assert_ok();
}

#[test]
fn if_none_match_wins_over_if_modified_since() {
    let response = get(
        &both(),
        "/dated",
        &[
            ("if-none-match", "\"stale\""),
            ("if-modified-since", "Mon, 07 Nov 1994 08:49:37 GMT"),
        ],
    );
    response.assert_ok();
}

#[test]
fn a_write_is_never_a_304() {
    let app = AppBuilder::new()
        .plugin_hooking(ConditionalGet::new(ConditionalGetConfig::default()))
        .build()
        .expect("an application");
    let mut table = Router::builder();
    table
        .route("/", Verb::Post, respond(HTML, html().into_bytes()))
        .expect("a route");
    let client = TestClient::serving(app, table.build().expect("a table"));
    let response = client.send(TestRequest::post("/").header("if-none-match", "*"));
    response.assert_ok();
    assert_eq!(response.header("etag"), None);
}

#[test]
fn no_store_gets_no_tag() {
    let app = AppBuilder::new()
        .plugin_hooking(ConditionalGet::new(ConditionalGetConfig::default()))
        .build()
        .expect("an application");
    let mut table = Router::builder();
    table
        .route(
            "/",
            Verb::Get,
            respond(
                &[("content-type", "text/html"), ("cache-control", "no-store")],
                html().into_bytes(),
            ),
        )
        .expect("a route");
    let client = TestClient::serving(app, table.build().expect("a table"));
    let response = client.send(TestRequest::get("/").header("if-none-match", "*"));
    response.assert_ok();
    assert_eq!(response.header("etag"), None);
}

/// Stands in for `gtag`: a hook that inserts markup into the HTML.
struct Inserter;

impl Plugin for Inserter {
    type Config = ();
    type Provides = NoContext;
    const NAME: &'static str = "gtag";
    const INSTALLS_HOOKS: bool = true;

    fn new(_: ()) -> Self {
        Self
    }
}

impl HookSource for Inserter {
    type Families = OnResponse;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        registrar.response("insert", Order::new(), Insert);
        Ok(())
    }
}

struct Insert;

impl ResponseHook for Insert {
    fn on_headers(
        &self,
        _request: &Request,
        _head: &mut ResponseHead,
        _body: BodyShape<'_>,
        _state: &mut RequestState,
    ) -> Result<(), PluginError> {
        Ok(())
    }

    fn on_body(
        &self,
        _request: &Request,
        _head: &mut ResponseHead,
        body: &mut Vec<u8>,
        _state: &mut RequestState,
    ) -> Result<(), PluginError> {
        body.extend_from_slice(b"<!--inserted-->");
        Ok(())
    }
}

#[test]
fn a_hook_that_inserts_markup_runs_before_compression() {
    // Registered first, so the tie-break alone would run it last, over
    // compressed bytes. Compression's own order puts it after `gtag`.
    let app = AppBuilder::new()
        .plugin_hooking(Inserter::new(()))
        .plugin_hooking(Compression::new(CompressionConfig::default()))
        .build()
        .expect("an application");
    let client = TestClient::serving(app, table());
    let response = get(&client, "/", &[("accept-encoding", "gzip")]);
    assert_eq!(
        gunzip(response.body()),
        format!("{}<!--inserted-->", html())
    );
}
