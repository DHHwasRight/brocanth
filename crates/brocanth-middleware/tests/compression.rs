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

// ---------------------------------------------------------------------
// One route table per test, for the edges the shared table does not have.
// ---------------------------------------------------------------------

fn respond_with(
    status: u16,
    headers: &'static [(&'static str, &'static str)],
    body: Vec<u8>,
) -> Handler<Base> {
    Box::new(move |_cx: &mut RouteCx<'_, Base>| {
        let mut response = http::Response::builder().status(status);
        for (name, value) in headers {
            response = response.header(*name, *value);
        }
        Ok(response.body(Body::from(body.clone())).expect("a response"))
    })
}

fn routes(list: Vec<(&str, Handler<Base>)>) -> Router<Handler<Base>> {
    let mut table = Router::builder();
    for (path, handler) in list {
        table.route(path, Verb::Get, handler).expect("a route");
    }
    table.build().expect("a table")
}

fn checking(
    config: ConditionalGetConfig,
    list: Vec<(&str, Handler<Base>)>,
) -> TestClient<Base, impl HookFamilies, Handler<Base>> {
    let app = AppBuilder::new()
        .plugin_hooking(ConditionalGet::new(config))
        .build()
        .expect("an application");
    TestClient::serving(app, routes(list))
}

fn compressing_routes(
    config: CompressionConfig,
    list: Vec<(&str, Handler<Base>)>,
) -> TestClient<Base, impl HookFamilies, Handler<Base>> {
    let app = AppBuilder::new()
        .plugin_hooking(Compression::new(config))
        .build()
        .expect("an application");
    TestClient::serving(app, routes(list))
}

/// Bytes that do not compress: a xorshift sequence.
fn noise(len: usize) -> Vec<u8> {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

// --- Conditional GET -------------------------------------------------

fn page_tag() -> String {
    let client = checking(
        ConditionalGetConfig::default(),
        vec![("/", respond(HTML, html().into_bytes()))],
    );
    get(&client, "/", &[])
        .header("etag")
        .expect("a tag")
        .to_string()
}

#[test]
fn a_star_matches_any_current_page() {
    let client = checking(
        ConditionalGetConfig::default(),
        vec![("/", respond(HTML, html().into_bytes()))],
    );
    let response = get(&client, "/", &[("if-none-match", "*")]);
    assert_eq!(response.status(), 304);
    assert!(response.body().is_empty());
}

#[test]
fn a_strong_tag_in_the_request_matches_the_weak_page_tag() {
    // A client may send the tag back without `W/`; weak comparison
    // ignores the prefix on both sides.
    let tag = page_tag();
    let strong = tag.trim_start_matches("W/");
    let client = checking(
        ConditionalGetConfig::default(),
        vec![("/", respond(HTML, html().into_bytes()))],
    );
    assert_eq!(
        get(&client, "/", &[("if-none-match", strong)]).status(),
        304
    );
}

#[test]
fn a_tag_anywhere_in_a_list_or_in_a_second_header_matches() {
    let tag = page_tag();
    let client = checking(
        ConditionalGetConfig::default(),
        vec![("/", respond(HTML, html().into_bytes()))],
    );
    let list = format!("\"a\", W/\"b,c\" ,{tag}");
    assert_eq!(get(&client, "/", &[("if-none-match", &list)]).status(), 304);
    let two_lines = get(
        &client,
        "/",
        &[("if-none-match", "\"stale\""), ("if-none-match", &tag)],
    );
    assert_eq!(two_lines.status(), 304, "every header line is read");
}

#[test]
fn a_malformed_if_none_match_is_a_full_page() {
    let client = checking(
        ConditionalGetConfig::default(),
        vec![("/", respond(HTML, html().into_bytes()))],
    );
    let tag = page_tag();
    let unquoted = tag.trim_start_matches("W/").trim_matches('"').to_string();
    for value in [unquoted.as_str(), "\"open", "W/", ",,,", "**"] {
        let response = get(&client, "/", &[("if-none-match", value)]);
        response.assert_ok();
        assert_eq!(response.text(), html(), "{value}");
    }
}

#[test]
fn head_gets_the_same_tag_and_a_304() {
    let client = checking(
        ConditionalGetConfig::default(),
        vec![("/", respond(HTML, html().into_bytes()))],
    );
    let tag = page_tag();
    let head = client.send(TestRequest::new(Verb::Head, "/"));
    head.assert_ok();
    assert_eq!(
        head.header("etag"),
        Some(tag.as_str()),
        "HEAD and GET agree"
    );
    let again = client.send(TestRequest::new(Verb::Head, "/").header("if-none-match", &tag));
    assert_eq!(again.status(), 304);
}

#[test]
fn a_status_other_than_200_is_left_alone() {
    let client = checking(
        ConditionalGetConfig::default(),
        vec![
            ("/missing", respond_with(404, HTML, html().into_bytes())),
            ("/gone", respond_with(410, HTML, html().into_bytes())),
            ("/created", respond_with(201, HTML, html().into_bytes())),
            (
                "/moved",
                respond_with(302, &[("location", "/")], Vec::new()),
            ),
        ],
    );
    for path in ["/missing", "/gone", "/created", "/moved"] {
        let response = get(&client, path, &[("if-none-match", "*")]);
        assert_ne!(response.status(), 304, "{path}");
        assert_eq!(response.header("etag"), None, "{path}");
    }
}

#[test]
fn every_write_verb_is_left_alone() {
    let app = AppBuilder::new()
        .plugin_hooking(ConditionalGet::new(ConditionalGetConfig::default()))
        .build()
        .expect("an application");
    let mut table = Router::builder();
    for verb in [Verb::Put, Verb::Patch, Verb::Delete] {
        table
            .route("/", verb, respond(HTML, html().into_bytes()))
            .expect("a route");
    }
    let client = TestClient::serving(app, table.build().expect("a table"));
    for request in [
        TestRequest::put("/"),
        TestRequest::patch("/"),
        TestRequest::delete("/"),
    ] {
        let verb = request.verb();
        let response = client.send(request.header("if-none-match", "*"));
        response.assert_ok();
        assert_eq!(response.header("etag"), None, "{verb}");
    }
}

#[test]
fn a_stream_gets_no_tag_and_no_304() {
    let client = checking(ConditionalGetConfig::default(), vec![("/", events())]);
    let response = get(&client, "/", &[("if-none-match", "*")]);
    response.assert_ok();
    assert_eq!(response.header("etag"), None);
    assert!(response.text().starts_with("data: "));
}

#[test]
fn a_304_keeps_the_cache_headers_and_drops_the_body_headers() {
    const HEADERS: &[(&str, &str)] = &[
        ("content-type", "text/html"),
        ("content-language", "fr"),
        ("cache-control", "private, max-age=60"),
        ("vary", "Cookie"),
        ("last-modified", "Sun, 06 Nov 1994 08:49:37 GMT"),
    ];
    let client = checking(
        ConditionalGetConfig::default(),
        vec![("/", respond(HEADERS, html().into_bytes()))],
    );
    let response = get(&client, "/", &[("if-none-match", "*")]);
    assert_eq!(response.status(), 304);
    assert_eq!(
        response.header("cache-control"),
        Some("private, max-age=60")
    );
    assert_eq!(response.header("vary"), Some("Cookie"));
    assert_eq!(
        response.header("last-modified"),
        Some("Sun, 06 Nov 1994 08:49:37 GMT")
    );
    assert!(response.header("etag").is_some());
    assert_eq!(response.header("content-type"), None);
    assert_eq!(response.header("content-language"), None);
    assert_eq!(response.header("content-length").unwrap_or("0"), "0");
}

#[test]
fn a_no_store_directive_among_others_still_counts() {
    const HEADERS: &[(&str, &str)] = &[
        ("content-type", "text/html"),
        ("cache-control", "private, No-Store, max-age=0"),
    ];
    let client = checking(
        ConditionalGetConfig::default(),
        vec![("/", respond(HEADERS, html().into_bytes()))],
    );
    let response = get(&client, "/", &[("if-none-match", "*")]);
    response.assert_ok();
    assert_eq!(response.header("etag"), None);
}

#[test]
fn etag_off_sets_no_tag_but_a_handlers_tag_still_answers() {
    let config = ConditionalGetConfig {
        etag: false,
        ..Default::default()
    };
    let client = checking(
        config,
        vec![
            ("/", respond(HTML, html().into_bytes())),
            (
                "/tagged",
                respond(
                    &[("content-type", "text/html"), ("etag", "\"v1\"")],
                    html().into_bytes(),
                ),
            ),
        ],
    );
    let plain = get(&client, "/", &[("if-none-match", "*")]);
    plain.assert_ok();
    assert_eq!(plain.header("etag"), None);
    let tagged = get(&client, "/tagged", &[("if-none-match", "\"v1\"")]);
    assert_eq!(tagged.status(), 304);
}

#[test]
fn last_modified_off_ignores_if_modified_since() {
    const DATED: &[(&str, &str)] = &[
        ("content-type", "text/html"),
        ("last-modified", "Sun, 06 Nov 1994 08:49:37 GMT"),
    ];
    let config = ConditionalGetConfig {
        last_modified: false,
        ..Default::default()
    };
    let client = checking(config, vec![("/", respond(DATED, html().into_bytes()))]);
    let response = get(
        &client,
        "/",
        &[("if-modified-since", "Mon, 07 Nov 1994 08:49:37 GMT")],
    );
    response.assert_ok();
    assert_eq!(response.text(), html());
}

#[test]
fn if_modified_since_without_last_modified_is_a_full_page() {
    let client = checking(
        ConditionalGetConfig::default(),
        vec![("/", respond(HTML, html().into_bytes()))],
    );
    let response = get(
        &client,
        "/",
        &[("if-modified-since", "Fri, 01 Jan 2100 00:00:00 GMT")],
    );
    response.assert_ok();
}

#[test]
fn the_tag_changes_with_the_body() {
    let client = checking(
        ConditionalGetConfig::default(),
        vec![
            ("/a", respond(HTML, b"<p>a</p>".to_vec())),
            ("/b", respond(HTML, b"<p>b</p>".to_vec())),
        ],
    );
    let a = get(&client, "/a", &[]).header("etag").map(str::to_string);
    let b = get(&client, "/b", &[]).header("etag").map(str::to_string);
    assert!(a.is_some() && b.is_some());
    assert_ne!(a, b);
    // The tag of /a does not answer /b.
    let cross = get(&client, "/b", &[("if-none-match", a.as_deref().unwrap())]);
    cross.assert_ok();
}

// --- Compression -----------------------------------------------------

#[test]
fn q_zero_on_both_codings_gets_identity_and_still_varies() {
    let client = compressing(CompressionConfig::default());
    for accept in ["br;q=0, gzip;q=0", "*;q=0", "identity", "gzip;q=0.000"] {
        let response = get(&client, "/", &[("accept-encoding", accept)]);
        assert_eq!(response.header("content-encoding"), None, "{accept}");
        assert_eq!(response.text(), html(), "{accept}");
        assert!(varies_on_encoding(&response), "{accept}");
    }
}

#[test]
fn an_unknown_encoding_gets_identity() {
    let client = compressing(CompressionConfig::default());
    for accept in ["zstd", "deflate, compress", "x-unknown;q=1", "gzipx", "b r"] {
        let response = get(&client, "/", &[("accept-encoding", accept)]);
        assert_eq!(response.header("content-encoding"), None, "{accept}");
        assert_eq!(response.text(), html(), "{accept}");
    }
}

#[test]
fn accept_encoding_on_two_header_lines_is_read_whole() {
    let response = get(
        &compressing(CompressionConfig::default()),
        "/",
        &[("accept-encoding", "deflate"), ("accept-encoding", "gzip")],
    );
    assert_eq!(response.header("content-encoding"), Some("gzip"));
}

#[test]
fn min_bytes_is_the_threshold() {
    let page = html();
    let at = CompressionConfig {
        min_bytes: page.len(),
        ..Default::default()
    };
    let response = get(&compressing(at), "/", &[("accept-encoding", "gzip")]);
    assert_eq!(response.header("content-encoding"), Some("gzip"), "at");
    let above = CompressionConfig {
        min_bytes: page.len() + 1,
        ..Default::default()
    };
    let response = get(&compressing(above), "/", &[("accept-encoding", "gzip")]);
    assert_eq!(response.header("content-encoding"), None, "under");
    assert!(
        !varies_on_encoding(&response),
        "never compressed, so no Vary"
    );
}

#[test]
fn a_body_that_does_not_shrink_goes_out_as_it_is() {
    let body = noise(4096);
    let client = compressing_routes(
        CompressionConfig::default(),
        vec![("/", respond(HTML, body.clone()))],
    );
    for accept in ["gzip", "br"] {
        let response = get(&client, "/", &[("accept-encoding", accept)]);
        assert_eq!(response.header("content-encoding"), None, "{accept}");
        assert_eq!(response.body(), &body[..], "{accept}");
        assert!(varies_on_encoding(&response), "{accept}");
    }
}

#[test]
fn types_replaces_the_list() {
    let config = CompressionConfig {
        types: vec!["application/json".to_string()],
        ..Default::default()
    };
    let json = format!("[{}]", vec!["{\"a\":1}"; 300].join(","));
    let client = compressing_routes(
        config,
        vec![
            ("/", respond(HTML, html().into_bytes())),
            (
                "/json",
                respond(
                    &[("content-type", "Application/JSON; charset=utf-8")],
                    json.clone().into_bytes(),
                ),
            ),
        ],
    );
    let page = get(&client, "/", &[("accept-encoding", "gzip")]);
    assert_eq!(
        page.header("content-encoding"),
        None,
        "text/html is off the list"
    );
    let data = get(&client, "/json", &[("accept-encoding", "gzip")]);
    assert_eq!(data.header("content-encoding"), Some("gzip"));
    assert_eq!(gunzip(data.body()), json);
}

#[test]
fn no_content_type_or_a_buffered_event_stream_is_left_alone() {
    let client = compressing_routes(
        CompressionConfig::default(),
        vec![
            ("/untyped", respond(&[], html().into_bytes())),
            (
                "/sse",
                respond(
                    &[("content-type", "text/event-stream")],
                    html().into_bytes(),
                ),
            ),
        ],
    );
    for path in ["/untyped", "/sse"] {
        let response = get(&client, path, &[("accept-encoding", "gzip, br")]);
        assert_eq!(response.header("content-encoding"), None, "{path}");
        assert!(!varies_on_encoding(&response), "{path}");
    }
}

#[test]
fn a_status_without_a_whole_body_is_left_alone_and_an_error_page_is_compressed() {
    let client = compressing_routes(
        CompressionConfig::default(),
        vec![
            ("/partial", respond_with(206, HTML, html().into_bytes())),
            ("/reset", respond_with(205, HTML, Vec::new())),
            ("/none", respond_with(204, HTML, Vec::new())),
            ("/missing", respond_with(404, HTML, html().into_bytes())),
        ],
    );
    let accept = [("accept-encoding", "gzip")];
    let partial = get(&client, "/partial", &accept);
    assert_eq!(partial.header("content-encoding"), None);
    assert_eq!(partial.text(), html());
    for path in ["/reset", "/none"] {
        assert_eq!(get(&client, path, &accept).header("content-encoding"), None);
    }
    let missing = get(&client, "/missing", &accept);
    assert_eq!(missing.status(), 404);
    assert_eq!(missing.header("content-encoding"), Some("gzip"));
    assert_eq!(gunzip(missing.body()), html());
}

#[test]
fn a_weak_tag_stays_as_it_is() {
    let client = compressing_routes(
        CompressionConfig::default(),
        vec![(
            "/",
            respond(
                &[("content-type", "text/html"), ("etag", "W/\"v1\"")],
                html().into_bytes(),
            ),
        )],
    );
    let response = get(&client, "/", &[("accept-encoding", "gzip")]);
    assert_eq!(response.header("content-encoding"), Some("gzip"));
    assert_eq!(response.header("etag"), Some("W/\"v1\""));
}

#[test]
fn an_identity_body_keeps_its_strong_tag() {
    let response = get(&compressing(CompressionConfig::default()), "/tagged", &[]);
    assert_eq!(response.header("content-encoding"), None);
    assert_eq!(response.header("etag"), Some("\"v1\""));
}

#[test]
fn the_vary_of_the_handler_is_kept() {
    let client = compressing_routes(
        CompressionConfig::default(),
        vec![(
            "/",
            respond(
                &[("content-type", "text/html"), ("vary", "Cookie")],
                html().into_bytes(),
            ),
        )],
    );
    let response = get(&client, "/", &[("accept-encoding", "gzip")]);
    let vary = response
        .header("vary")
        .expect("a vary")
        .to_ascii_lowercase();
    assert!(
        vary.contains("cookie") && vary.contains("accept-encoding"),
        "{vary}"
    );
}

/// Words in an order that does not repeat, so a slow level finds matches
/// a fast one skips.
fn prose() -> Vec<u8> {
    const WORDS: [&str; 16] = [
        "the", "page", "cache", "renders", "a", "row", "and", "client", "server", "sends",
        "header", "body", "of", "tag", "brotli", "gzip",
    ];
    let words: Vec<&str> = noise(20_000)
        .iter()
        .map(|b| WORDS[usize::from(*b) % WORDS.len()])
        .collect();
    format!("<p>{}</p>", words.join(" ")).into_bytes()
}

#[test]
fn gzip_level_and_brotli_quality_reach_the_encoder() {
    let size = |config: CompressionConfig, accept: &str| {
        let client = compressing_routes(config, vec![("/", respond(HTML, prose()))]);
        let response = get(&client, "/", &[("accept-encoding", accept)]);
        assert!(response.header("content-encoding").is_some(), "{accept}");
        response.body().len()
    };
    let fast = size(
        CompressionConfig {
            gzip_level: 1,
            ..Default::default()
        },
        "gzip",
    );
    let small = size(
        CompressionConfig {
            gzip_level: 9,
            ..Default::default()
        },
        "gzip",
    );
    assert!(small < fast, "level 9 ({small}) under level 1 ({fast})");
    let fast = size(
        CompressionConfig {
            brotli_quality: 0,
            ..Default::default()
        },
        "br",
    );
    let small = size(
        CompressionConfig {
            brotli_quality: 11,
            ..Default::default()
        },
        "br",
    );
    assert!(
        small < fast,
        "quality 11 ({small}) under quality 0 ({fast})"
    );
}

#[test]
fn gzip_off_means_identity_for_a_gzip_only_client() {
    let config = CompressionConfig {
        gzip: false,
        ..Default::default()
    };
    let response = get(&compressing(config), "/", &[("accept-encoding", "gzip")]);
    assert_eq!(response.header("content-encoding"), None);
    assert!(varies_on_encoding(&response));
}

#[test]
fn a_bad_setting_stops_the_start_with_a_help_line() {
    let result = AppBuilder::new()
        .plugin_hooking(Compression::new(CompressionConfig {
            gzip_level: 10,
            ..Default::default()
        }))
        .build();
    let Err(error) = result else {
        panic!("a gzip level of 10 must stop the start");
    };
    let message = error.to_string();
    assert!(message.contains("gzip_level"), "{message}");
    assert!(message.contains("help:"), "{message}");
}

/// An application hook that rewrites the body, named in `runs_after`.
struct Footer;

impl Plugin for Footer {
    type Config = ();
    type Provides = NoContext;
    const NAME: &'static str = "footer";
    const INSTALLS_HOOKS: bool = true;

    fn new(_: ()) -> Self {
        Self
    }
}

impl HookSource for Footer {
    type Families = OnResponse;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        registrar.response("append", Order::new(), Insert);
        Ok(())
    }
}

#[test]
fn runs_after_puts_an_application_hook_before_compression() {
    let config = CompressionConfig {
        runs_after: vec!["footer".to_string()],
        ..Default::default()
    };
    let app = AppBuilder::new()
        .plugin_hooking(Footer::new(()))
        .plugin_hooking(Compression::new(config))
        .build()
        .expect("an application");
    let client = TestClient::serving(app, table());
    let response = get(&client, "/", &[("accept-encoding", "gzip")]);
    assert_eq!(
        gunzip(response.body()),
        format!("{}<!--inserted-->", html())
    );
}

#[test]
fn a_304_through_both_hooks_is_not_compressed_and_does_not_vary_wrongly() {
    let client = both();
    let tag = get(&client, "/", &[])
        .header("etag")
        .expect("a tag")
        .to_string();
    let response = get(
        &client,
        "/",
        &[("accept-encoding", "gzip, br"), ("if-none-match", &tag)],
    );
    assert_eq!(response.status(), 304);
    assert!(response.body().is_empty());
    assert_eq!(response.header("content-encoding"), None);
}
