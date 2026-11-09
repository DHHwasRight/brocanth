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
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn Storage> = Arc::new(LocalDisk::open(dir.path()).unwrap());
    let tokens = FileTokens::new(FileTokenKey::from_secret(b"test secret"));
    let routes = FileRoutes::new(
        store.clone(),
        tokens,
        config,
        move |_: &http::Request<Vec<u8>>, _: &brocanth::plugin_hooks::RequestState| open,
    )
    .unwrap();
    let app = AppBuilder::new()
        .plugin_hooking(Csrf::new(CsrfConfig::default()))
        .plugin_hooking(Files::new(routes.clone()))
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
