//! The upload and download routes through the whole pipeline, beside the
//! CSRF hook.

#![cfg(feature = "routes")]

use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{HookRegistrar, HookSource, OnAsyncRequest};
use brocanth::plugin_order::Order;
use brocanth::routing::Verb;
use brocanth_middleware::{Csrf, CsrfConfig};
use brocanth_storage::routes::{FileRoutes, RoutesConfig};
use brocanth_storage::{FileTokenKey, FileTokens, LocalDisk, ObjectInfo, ObjectKey, Storage};
use brocanth_test::{TestClient, TestRequest, TestResponse};

/// The one future at a time a test needs. The disk backend never waits on
/// a socket, so polling on this thread and parking is enough.
fn block_on<F: Future>(future: F) -> F::Output {
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        if let Poll::Ready(out) = future.as_mut().poll(&mut cx) {
            return out;
        }
        std::thread::park();
    }
}

/// The plugin a project writes, cut down: the two hooks around the CSRF
/// check.
struct Files {
    routes: FileRoutes,
}

impl Plugin for Files {
    type Config = FileRoutes;
    type Provides = NoContext;
    const NAME: &'static str = "storage_routes";
    const INSTALLS_HOOKS: bool = true;

    fn new(routes: FileRoutes) -> Self {
        Self { routes }
    }
}

impl HookSource for Files {
    type Families = OnAsyncRequest;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        registrar.request_async(
            "lift",
            Order::new().before_if_present("csrf.check"),
            self.routes.csrf_lift(),
        );
        registrar.request_async(
            "answer",
            Order::new().after_if_present("csrf.check"),
            self.routes.clone(),
        );
        Ok(())
    }
}

const CSRF: &str = "ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12";

struct Rig {
    /// The client, behind a closure: its type names the application's
    /// plugin list.
    send: Box<dyn Fn(TestRequest) -> TestResponse>,
    routes: FileRoutes,
    store: Arc<dyn Storage>,
    _dir: tempfile::TempDir,
}

fn rig(config: RoutesConfig, open: bool) -> Rig {
    rig_with(config, open, CsrfConfig::default(), &[])
}

/// The token service the rig signs with, so a test can mint its own.
fn tokens() -> FileTokens {
    FileTokens::new(FileTokenKey::from_secret(b"test secret"))
}

/// A rig with its own CSRF configuration and the `storage_routes` hooks
/// named in `off` turned off, as `src/modules/storage_routes.rs` does.
fn rig_with(config: RoutesConfig, open: bool, csrf: CsrfConfig, off: &[&'static str]) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn Storage> = Arc::new(LocalDisk::open(dir.path()).unwrap());
    let tokens = tokens();
    let routes = FileRoutes::new(
        store.clone(),
        tokens,
        config,
        move |_: &http::Request<Vec<u8>>, _: &brocanth::plugin_hooks::RequestState| open,
    )
    .unwrap();
    let app = AppBuilder::new()
        .plugin_hooking(Csrf::new(csrf))
        .plugin_hooking(Files::new(routes.clone()))
        .hooks_off("storage_routes", off.iter().copied())
        .build()
        .expect("an application");
    let client = TestClient::new(app);
    Rig {
        send: Box::new(move |request| block_on(client.send_async(request))),
        routes,
        store,
        _dir: dir,
    }
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbImage::from_pixel(width, height, image::Rgb([200, 30, 30]));
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

/// A plain HTML form: the CSRF token in a field, the file in another.
fn form(fields: &[(&str, &str)], file: (&str, &str, &[u8])) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(
            format!("--b\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
                .as_bytes(),
        );
    }
    let (filename, content_type, bytes) = file;
    body.extend_from_slice(
        format!(
            "--b\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n\
             Content-Type: {content_type}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n--b--\r\n");
    body
}

fn upload(rig: &Rig, body: Vec<u8>) -> TestResponse {
    (rig.send)(
        TestRequest::post("/files")
            .header("cookie", &format!("csrf_token={CSRF}"))
            .body("multipart/form-data; boundary=b", body),
    )
}

fn get(rig: &Rig, target: &str) -> TestResponse {
    (rig.send)(TestRequest::get(target))
}

#[test]
fn a_form_uploads_without_a_script_and_comes_back_to_its_page() {
    let rig = rig(RoutesConfig::default(), true);
    let image = png(40, 20);
    let sent = upload(
        &rig,
        form(
            &[("_csrf_token", CSRF), ("return_to", "/posts/new")],
            ("cat.png", "text/html", &image),
        ),
    );
    assert_eq!(sent.status(), 303, "{}", sent.text());
    let back = sent.location().unwrap().to_string();
    assert!(back.starts_with("/posts/new?file="), "{back}");
    let key = ObjectKey::of(&image);
    assert!(
        back.contains(&key.to_hex()),
        "the address is the content: {back}"
    );

    // The link it handed back opens the file, served as what the bytes are.
    let token = back.split("token=").nth(1).unwrap();
    let file = get(&rig, &format!("/files/{}?token={token}", key.to_hex()));
    assert_eq!(file.status(), 200);
    assert_eq!(file.header("content-type"), Some("image/png"));
    assert_eq!(file.body(), image.as_slice());
    assert_eq!(file.header("x-content-type-options"), Some("nosniff"));
    assert!(
        file.header("content-disposition").is_none(),
        "an image is inline"
    );

    // A copy the browser holds is not sent again.
    let etag = file.header("etag").unwrap().to_string();
    let again = (rig.send)(
        TestRequest::get(format!("/files/{}?token={token}", key.to_hex()))
            .header("if-none-match", &etag),
    );
    assert_eq!(again.status(), 304);
}

#[test]
fn an_upload_with_no_csrf_token_stops_at_the_check() {
    let rig = rig(RoutesConfig::default(), true);
    let sent = upload(&rig, form(&[], ("a.png", "image/png", &png(2, 2))));
    assert_eq!(sent.status(), 403);
    let forged = upload(
        &rig,
        form(
            &[("_csrf_token", &"cd".repeat(32))],
            ("a.png", "image/png", &png(2, 2)),
        ),
    );
    assert_eq!(forged.status(), 403);
    assert!(!block_on(rig.store.exists(&ObjectKey::of(&png(2, 2)))).unwrap());
}

#[test]
fn the_guard_decides_who_may_upload() {
    let rig = rig(RoutesConfig::default(), false);
    let sent = upload(
        &rig,
        form(&[("_csrf_token", CSRF)], ("a.png", "image/png", &png(2, 2))),
    );
    assert_eq!(sent.status(), 403);
    assert!(sent.text().contains("help:"), "{}", sent.text());
}

#[test]
fn a_file_past_the_limit_is_refused() {
    let config = RoutesConfig {
        max_bytes: 10,
        ..RoutesConfig::default()
    };
    let rig = rig(config, true);
    let sent = upload(
        &rig,
        form(&[("_csrf_token", CSRF)], ("a.bin", "x/y", &[7_u8; 64])),
    );
    assert_eq!(sent.status(), 413);
}

#[test]
fn a_download_needs_a_token_for_that_file() {
    let rig = rig(RoutesConfig::default(), true);
    let key = block_on(rig.store.put(b"secret notes", &ObjectInfo::new())).unwrap();
    let other = block_on(rig.store.put(b"other", &ObjectInfo::new())).unwrap();
    let hex = key.to_hex();

    assert_eq!(get(&rig, &format!("/files/{hex}")).status(), 403);
    assert_eq!(
        get(&rig, &format!("/files/{hex}?token=forged.token")).status(),
        403
    );
    // A token for another file opens only that one.
    let wrong = rig.routes.url(&other);
    let wrong_token = wrong.split("token=").nth(1).unwrap();
    assert_eq!(
        get(&rig, &format!("/files/{hex}?token={wrong_token}")).status(),
        403
    );

    // Not an image, so an attachment that runs nothing when opened.
    let file = get(&rig, &rig.routes.url(&key));
    assert_eq!(file.status(), 200);
    assert_eq!(file.body(), b"secret notes");
    assert_eq!(
        file.header("content-type"),
        Some("application/octet-stream")
    );
    assert!(file
        .header("content-disposition")
        .unwrap()
        .starts_with("attachment"));
    assert_eq!(file.header("content-security-policy"), Some("sandbox"));
    assert!(file.header("cache-control").unwrap().starts_with("private"));

    assert_eq!(get(&rig, "/files/not-an-address").status(), 404);
    let post = (rig.send)(
        TestRequest::post(format!("/files/{hex}"))
            .header("cookie", &format!("csrf_token={CSRF}"))
            .header("x-csrf-token", CSRF),
    );
    assert_eq!(post.status(), 405);
}

#[test]
fn public_routes_serve_without_a_token_and_cache_for_ever() {
    let config = RoutesConfig {
        public: true,
        ..RoutesConfig::default()
    };
    let rig = rig(config, true);
    let key = block_on(rig.store.put(b"public", &ObjectInfo::new())).unwrap();
    let file = get(&rig, &format!("/files/{}", key.to_hex()));
    assert_eq!(file.status(), 200);
    assert!(file.header("cache-control").unwrap().contains("immutable"));
}

#[test]
fn a_thumbnail_is_rendered_once_and_then_read_from_the_cache() {
    let rig = rig(RoutesConfig::default(), true);
    let key = block_on(rig.store.put(&png(300, 200), &ObjectInfo::new())).unwrap();
    let url = rig.routes.url(&key);
    let thumb = get(&rig, &format!("{url}&thumb=30x20"));
    assert_eq!(thumb.status(), 200, "{}", thumb.text());
    let small = image::load_from_memory(thumb.body()).unwrap();
    assert_eq!((small.width(), small.height()), (30, 20));
    let spec = "30x20".parse().unwrap();
    assert!(
        block_on(rig.store.get_variant(&key, spec))
            .unwrap()
            .is_some(),
        "cached"
    );

    assert_eq!(get(&rig, &format!("{url}&thumb=huge")).status(), 400);
    let text = block_on(rig.store.put(b"plain text", &ObjectInfo::new())).unwrap();
    let text_url = rig.routes.url(&text);
    assert_eq!(get(&rig, &format!("{text_url}&thumb=30x20")).status(), 415);
}

#[test]
fn thumbnails_turned_off_are_not_found() {
    let config = RoutesConfig {
        thumbnails: false,
        ..RoutesConfig::default()
    };
    let rig = rig(config, true);
    let key = block_on(rig.store.put(&png(30, 20), &ObjectInfo::new())).unwrap();
    let thumb = get(&rig, &format!("{}&thumb=10x10", rig.routes.url(&key)));
    assert_eq!(thumb.status(), 404);
}

#[test]
fn every_other_path_reaches_the_router() {
    let rig = rig(RoutesConfig::default(), true);
    // No route table, so the router answers 404 for both; the point is that
    // the hooks let them through rather than answering 403 or 405.
    assert_eq!(get(&rig, "/files").status(), 404);
    assert_eq!(get(&rig, "/filesystem").status(), 404);
    assert_eq!(get(&rig, "/files/a/b").status(), 404);
}

/// A multipart body with only text fields.
fn fields_only(fields: &[(&str, &str)]) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(
            format!("--b\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
                .as_bytes(),
        );
    }
    body.extend_from_slice(b"--b--\r\n");
    body
}

/// The token in a `Location` or a URL.
fn token_in(url: &str) -> &str {
    url.split("token=")
        .nth(1)
        .unwrap()
        .split(['&', '#'])
        .next()
        .unwrap()
}

#[test]
fn a_form_with_no_file_part_is_a_400_that_names_the_field() {
    let rig = rig(RoutesConfig::default(), true);
    let sent = upload(&rig, fields_only(&[("_csrf_token", CSRF), ("title", "x")]));
    assert_eq!(sent.status(), 400, "{}", sent.text());
    assert!(sent.text().contains("`file`"), "{}", sent.text());
}

#[test]
fn an_empty_file_is_a_400() {
    let rig = rig(RoutesConfig::default(), true);
    let sent = upload(
        &rig,
        form(&[("_csrf_token", CSRF)], ("a.txt", "text/plain", b"")),
    );
    assert_eq!(sent.status(), 400, "{}", sent.text());
}

#[test]
fn a_body_that_is_not_multipart_is_a_415_with_help() {
    let rig = rig(RoutesConfig::default(), true);
    let sent = (rig.send)(
        TestRequest::post("/files")
            .header("cookie", &format!("csrf_token={CSRF}"))
            .header("x-csrf-token", CSRF)
            .body("application/x-www-form-urlencoded", b"file=abc".to_vec()),
    );
    assert_eq!(sent.status(), 415);
    assert!(sent.text().contains("help:"), "{}", sent.text());

    let no_boundary = (rig.send)(
        TestRequest::post("/files")
            .header("cookie", &format!("csrf_token={CSRF}"))
            .header("x-csrf-token", CSRF)
            .body("multipart/form-data", b"--b--\r\n".to_vec()),
    );
    assert_eq!(no_boundary.status(), 400, "{}", no_boundary.text());
}

#[test]
fn a_file_of_exactly_max_bytes_is_taken_and_one_byte_more_is_not() {
    let config = RoutesConfig {
        max_bytes: 64,
        ..RoutesConfig::default()
    };
    let rig = rig(config, true);
    let fits = [1_u8; 64];
    let sent = upload(
        &rig,
        form(&[("_csrf_token", CSRF)], ("a.bin", "x/y", &fits)),
    );
    assert_eq!(sent.status(), 201, "{}", sent.text());
    assert_eq!(sent.text(), ObjectKey::of(&fits).to_hex());

    let over = [2_u8; 65];
    let sent = upload(
        &rig,
        form(&[("_csrf_token", CSRF)], ("a.bin", "x/y", &over)),
    );
    assert_eq!(sent.status(), 413, "{}", sent.text());
    assert!(!block_on(rig.store.exists(&ObjectKey::of(&over))).unwrap());
}

#[test]
fn a_refused_upload_stores_nothing() {
    let rig = rig(RoutesConfig::default(), false);
    let bytes = png(3, 3);
    let sent = upload(
        &rig,
        form(&[("_csrf_token", CSRF)], ("a.png", "image/png", &bytes)),
    );
    assert_eq!(sent.status(), 403);
    assert!(!block_on(rig.store.exists(&ObjectKey::of(&bytes))).unwrap());
}

#[test]
fn a_fetch_caller_uploads_with_the_header_and_gets_201_with_the_link() {
    let rig = rig(RoutesConfig::default(), true);
    let bytes = b"notes from a script".to_vec();
    let sent = (rig.send)(
        TestRequest::post("/files")
            .header("cookie", &format!("csrf_token={CSRF}"))
            .header("x-csrf-token", CSRF)
            .body(
                "multipart/form-data; boundary=b",
                form(&[], ("n.txt", "text/plain", &bytes)),
            ),
    );
    assert_eq!(sent.status(), 201, "{}", sent.text());
    let hex = ObjectKey::of(&bytes).to_hex();
    assert_eq!(sent.text(), hex);
    let location = sent.location().unwrap().to_string();
    assert!(
        location.starts_with(&format!("/files/{hex}?token=")),
        "{location}"
    );
    let file = get(&rig, &location);
    assert_eq!(file.status(), 200);
    assert_eq!(file.body(), bytes.as_slice());
}

#[test]
fn an_expired_token_is_refused_like_a_forged_one() {
    let rig = rig(RoutesConfig::default(), true);
    let key = block_on(rig.store.put(b"short lived", &ObjectInfo::new())).unwrap();
    let hex = key.to_hex();
    let now = brocanth_storage::token::now();

    let old = tokens().issue_at(&key, now - 10 * 60);
    let expired = get(&rig, &format!("/files/{hex}?token={}", old.as_str()));
    assert_eq!(expired.status(), 403);

    let fresh = tokens().issue_at(&key, now);
    assert_eq!(
        get(&rig, &format!("/files/{hex}?token={}", fresh.as_str())).status(),
        200
    );
    // Signed with another key: the token of another deployment.
    let foreign = FileTokens::new(FileTokenKey::from_secret(b"another secret")).issue(&key);
    let refused = get(&rig, &format!("/files/{hex}?token={}", foreign.as_str()));
    assert_eq!(refused.status(), 403);
    assert_eq!(refused.text(), expired.text(), "one answer for both");
}

#[test]
fn a_token_changed_by_one_character_is_refused() {
    let rig = rig(RoutesConfig::default(), true);
    let key = block_on(rig.store.put(b"tamper me", &ObjectInfo::new())).unwrap();
    let url = rig.routes.url(&key);
    let token = token_in(&url).to_string();
    for at in [0, token.len() / 2, token.len() - 1] {
        let mut bytes = token.clone().into_bytes();
        bytes[at] = if bytes[at] == b'A' { b'B' } else { b'A' };
        let tampered = String::from_utf8(bytes).unwrap();
        let answer = get(&rig, &format!("/files/{}?token={tampered}", key.to_hex()));
        assert_eq!(answer.status(), 403, "byte {at}");
    }
}

#[test]
fn a_public_route_still_refuses_a_bad_token() {
    let config = RoutesConfig {
        public: true,
        ..RoutesConfig::default()
    };
    let rig = rig(config, true);
    let key = block_on(rig.store.put(b"public", &ObjectInfo::new())).unwrap();
    let answer = get(&rig, &format!("/files/{}?token=forged.token", key.to_hex()));
    assert_eq!(answer.status(), 403);
}

#[test]
fn a_path_that_climbs_out_of_the_store_is_never_a_file() {
    let rig = rig(RoutesConfig::default(), true);
    let key = block_on(rig.store.put(b"inside", &ObjectInfo::new())).unwrap();
    let token = token_in(&rig.routes.url(&key)).to_string();
    for path in [
        "/files/..",
        "/files/../../etc/passwd",
        "/files/..%2f..%2fetc%2fpasswd",
        "/files/%2e%2e",
        "/files/%2e%2e%2f%2e%2e%2fetc%2fpasswd",
        "/files/..\\..\\etc\\passwd",
    ] {
        let answer = get(&rig, &format!("{path}?token={token}"));
        assert_eq!(answer.status(), 404, "{path}: {}", answer.text());
    }
    // An address with a path in it: two segments, so the router's.
    let answer = get(&rig, &format!("/files/{}/..?token={token}", key.to_hex()));
    assert_eq!(answer.status(), 404);
}

#[test]
fn a_return_to_off_this_site_is_never_followed() {
    let rig = rig(RoutesConfig::default(), true);
    for (n, to) in [
        "//evil.example/",
        "https://evil.example/",
        "/\\evil.example",
        "\\\\evil.example",
        "javascript:alert(1)",
        " //evil.example",
        "/ok\r\nset-cookie: a=b",
        "/ok\tx",
        "evil.example",
    ]
    .into_iter()
    .enumerate()
    {
        let bytes = format!("file {n}").into_bytes();
        let sent = upload(
            &rig,
            form(
                &[("_csrf_token", CSRF), ("return_to", to)],
                ("a.txt", "text/plain", &bytes),
            ),
        );
        assert_eq!(sent.status(), 201, "{to:?} was followed");
        let location = sent.location().unwrap();
        assert!(location.starts_with("/files/"), "{to:?}: {location}");
        assert!(sent.header("set-cookie").is_none_or(|c| !c.contains("a=b")));
    }
}

#[test]
fn a_return_to_keeps_its_query_and_its_fragment() {
    let rig = rig(RoutesConfig::default(), true);
    let bytes = b"with a query".to_vec();
    let hex = ObjectKey::of(&bytes).to_hex();
    let sent = upload(
        &rig,
        form(
            &[("_csrf_token", CSRF), ("return_to", "/posts/new?draft=1")],
            ("a.txt", "text/plain", &bytes),
        ),
    );
    assert_eq!(sent.status(), 303);
    let back = sent.location().unwrap();
    assert!(
        back.starts_with(&format!("/posts/new?draft=1&file={hex}&token=")),
        "{back}"
    );

    let sent = upload(
        &rig,
        form(
            &[("_csrf_token", CSRF), ("return_to", "/posts/new#photos")],
            ("a.txt", "text/plain", &bytes),
        ),
    );
    let back = sent.location().unwrap();
    assert!(
        back.starts_with(&format!("/posts/new?file={hex}&token=")),
        "the query is before the fragment: {back}"
    );
    assert!(back.ends_with("#photos"), "{back}");
    // The token in it still opens the file.
    let file = get(&rig, &format!("/files/{hex}?token={}", token_in(back)));
    assert_eq!(file.status(), 200);
}

#[test]
fn an_uploaded_page_is_served_as_an_attachment_that_runs_nothing() {
    let rig = rig(RoutesConfig::default(), true);
    let page = b"<html><script>alert(document.cookie)</script></html>".to_vec();
    let sent = upload(
        &rig,
        form(
            &[("_csrf_token", CSRF)],
            ("evil\".html; x=\"y", "text/html", &page),
        ),
    );
    assert_eq!(sent.status(), 201, "{}", sent.text());
    let file = get(&rig, sent.location().unwrap());
    assert_eq!(file.status(), 200);
    assert_ne!(file.header("content-type"), Some("text/html"));
    assert!(!file.header("content-type").unwrap().contains("html"));
    let disposition = file.header("content-disposition").unwrap();
    assert!(
        disposition.starts_with("attachment; filename=\""),
        "{disposition}"
    );
    assert_eq!(
        disposition.matches('"').count(),
        2,
        "the name cannot close the quotes: {disposition}"
    );
    assert_eq!(file.header("content-security-policy"), Some("sandbox"));
    assert_eq!(file.header("x-content-type-options"), Some("nosniff"));
}

#[test]
fn a_thumbnail_size_out_of_bounds_is_a_400_with_the_shape() {
    let rig = rig(RoutesConfig::default(), true);
    let key = block_on(rig.store.put(&png(40, 40), &ObjectInfo::new())).unwrap();
    let url = rig.routes.url(&key);
    for size in [
        "0x0",
        "4097x10",
        "10x4097",
        "-1x10",
        "99999999999x1",
        "10x10q",
        "10",
        "x",
        "0x10f",
        "1234567890123x1",
    ] {
        let answer = get(&rig, &format!("{url}&thumb={size}"));
        assert_eq!(answer.status(), 400, "{size}: {}", answer.text());
        assert!(answer.text().contains("300x200"), "{}", answer.text());
    }
}

#[test]
fn a_source_image_too_large_for_a_thumbnail_is_a_413() {
    let rig = rig(RoutesConfig::default(), true);
    let wide = png(brocanth_storage::MAX_SOURCE_DIM + 1, 1);
    let key = block_on(rig.store.put(&wide, &ObjectInfo::new())).unwrap();
    let answer = get(&rig, &format!("{}&thumb=10x10", rig.routes.url(&key)));
    assert_eq!(answer.status(), 413, "{}", answer.text());
    // The original is still served: the limit is on decoding, not storing.
    assert_eq!(get(&rig, &rig.routes.url(&key)).status(), 200);
}

#[test]
fn a_thumbnail_has_its_own_entity_tag() {
    let rig = rig(RoutesConfig::default(), true);
    let key = block_on(rig.store.put(&png(60, 40), &ObjectInfo::new())).unwrap();
    let url = rig.routes.url(&key);
    let original = get(&rig, &url);
    let thumb = get(&rig, &format!("{url}&thumb=30x20"));
    let tag = thumb.header("etag").unwrap().to_string();
    assert_ne!(Some(tag.as_str()), original.header("etag"));
    let again =
        (rig.send)(TestRequest::get(format!("{url}&thumb=30x20")).header("if-none-match", &tag));
    assert_eq!(again.status(), 304);
    assert!(again.body().is_empty());
    // The original's tag does not stand for the thumbnail.
    let other = (rig.send)(
        TestRequest::get(format!("{url}&thumb=30x20"))
            .header("if-none-match", original.header("etag").unwrap()),
    );
    assert_eq!(other.status(), 200);
}

#[test]
fn head_sends_the_headers_without_the_bytes() {
    let rig = rig(RoutesConfig::default(), true);
    let key = block_on(rig.store.put(b"twelve bytes", &ObjectInfo::new())).unwrap();
    let answer = (rig.send)(TestRequest::new(Verb::Head, rig.routes.url(&key)));
    assert_eq!(answer.status(), 200);
    assert!(answer.body().is_empty());
    assert_eq!(answer.header("etag"), Some(key.etag().as_str()));
}

#[test]
fn another_verb_on_a_file_is_a_405_that_names_get() {
    let rig = rig(RoutesConfig::default(), true);
    let key = block_on(rig.store.put(b"keep me", &ObjectInfo::new())).unwrap();
    for verb in [Verb::Delete, Verb::Put, Verb::Patch] {
        let answer = (rig.send)(
            TestRequest::new(verb, format!("/files/{}", key.to_hex()))
                .header("cookie", &format!("csrf_token={CSRF}"))
                .header("x-csrf-token", CSRF),
        );
        assert_eq!(answer.status(), 405, "{verb:?}");
        assert_eq!(answer.header("allow"), Some("GET, HEAD"));
    }
    assert!(block_on(rig.store.exists(&key)).unwrap());
}

#[test]
fn the_lift_never_overrides_a_header_the_caller_sent() {
    let rig = rig(RoutesConfig::default(), true);
    let sent = (rig.send)(
        TestRequest::post("/files")
            .header("cookie", &format!("csrf_token={CSRF}"))
            .header("x-csrf-token", &"cd".repeat(32))
            .body(
                "multipart/form-data; boundary=b",
                form(&[("_csrf_token", CSRF)], ("a.txt", "text/plain", b"x")),
            ),
    );
    assert_eq!(sent.status(), 403);
}

#[test]
fn with_the_lift_hook_off_a_plain_form_stops_at_the_csrf_check() {
    let rig = rig_with(
        RoutesConfig::default(),
        true,
        CsrfConfig::default(),
        &["lift"],
    );
    let bytes = b"no lift".to_vec();
    let sent = upload(
        &rig,
        form(&[("_csrf_token", CSRF)], ("a.txt", "text/plain", &bytes)),
    );
    assert_eq!(
        sent.status(),
        403,
        "the field reached the check: {}",
        sent.text()
    );
    assert!(!block_on(rig.store.exists(&ObjectKey::of(&bytes))).unwrap());
    // A fetch caller's header does not need the hook.
    let sent = (rig.send)(
        TestRequest::post("/files")
            .header("cookie", &format!("csrf_token={CSRF}"))
            .header("x-csrf-token", CSRF)
            .body(
                "multipart/form-data; boundary=b",
                form(&[], ("a.txt", "text/plain", &bytes)),
            ),
    );
    assert_eq!(sent.status(), 201, "{}", sent.text());
}

#[test]
fn the_csrf_settings_name_the_field_and_the_header() {
    let config = RoutesConfig {
        csrf_field: "authenticity".to_string(),
        csrf_header: "x-authenticity".to_string(),
        ..RoutesConfig::default()
    };
    let csrf = CsrfConfig::default()
        .field("authenticity")
        .header("x-authenticity");
    let rig = rig_with(config, true, csrf, &[]);
    let ok = upload(
        &rig,
        form(&[("authenticity", CSRF)], ("a.txt", "text/plain", b"one")),
    );
    assert_eq!(ok.status(), 201, "{}", ok.text());
    let old_name = upload(
        &rig,
        form(&[("_csrf_token", CSRF)], ("a.txt", "text/plain", b"two")),
    );
    assert_eq!(old_name.status(), 403);
}

#[test]
fn the_path_setting_moves_both_routes() {
    let config = RoutesConfig {
        path: "/uploads/".to_string(),
        ..RoutesConfig::default()
    };
    let rig = rig(config, true);
    assert_eq!(rig.routes.path(), "/uploads");
    let bytes = b"moved".to_vec();
    let sent = (rig.send)(
        TestRequest::post("/uploads")
            .header("cookie", &format!("csrf_token={CSRF}"))
            .body(
                "multipart/form-data; boundary=b",
                form(&[("_csrf_token", CSRF)], ("a.txt", "text/plain", &bytes)),
            ),
    );
    assert_eq!(sent.status(), 201, "{}", sent.text());
    let location = sent.location().unwrap();
    assert!(location.starts_with("/uploads/"), "{location}");
    assert_eq!(get(&rig, location).status(), 200);
    // The default path is the router's now.
    let hex = ObjectKey::of(&bytes).to_hex();
    let token = token_in(location);
    assert_eq!(
        get(&rig, &format!("/files/{hex}?token={token}")).status(),
        404
    );
}

#[test]
fn the_field_setting_names_the_file_part() {
    let config = RoutesConfig {
        field: "upload".to_string(),
        ..RoutesConfig::default()
    };
    let rig = rig(config, true);
    let sent = upload(
        &rig,
        form(&[("_csrf_token", CSRF)], ("a.txt", "text/plain", b"x")),
    );
    assert_eq!(sent.status(), 400);
    assert!(sent.text().contains("`upload`"), "{}", sent.text());
}

#[test]
fn a_path_or_a_limit_that_cannot_work_is_refused_when_built() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn Storage> = Arc::new(LocalDisk::open(dir.path()).unwrap());
    let build = |config: RoutesConfig| {
        FileRoutes::new(
            store.clone(),
            tokens(),
            config,
            |_: &http::Request<Vec<u8>>, _: &brocanth::plugin_hooks::RequestState| true,
        )
    };
    for path in ["files", "/", "", "//"] {
        let err = build(RoutesConfig {
            path: path.to_string(),
            ..RoutesConfig::default()
        })
        .expect_err(path);
        assert!(err.to_string().contains("help:"), "{err}");
    }
    let err = build(RoutesConfig {
        max_bytes: 0,
        ..RoutesConfig::default()
    })
    .expect_err("max_bytes 0");
    assert!(err.to_string().contains("help:"), "{err}");
}

#[test]
fn the_defaults_are_what_the_module_doc_says() {
    let config = RoutesConfig::default();
    assert_eq!(config.path, "/files");
    assert_eq!(config.max_bytes, 2_097_152);
    assert_eq!(config.field, "file");
    assert!(!config.public);
    assert!(config.thumbnails);
    assert_eq!(config.csrf_field, "_csrf_token");
    assert_eq!(config.csrf_header, "x-csrf-token");
}
