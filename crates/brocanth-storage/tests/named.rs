//! Two stores behind one hook: the default at `/files`, a named store at
//! `/files/archive` (`docs/object-storage.md` §9.1). Each keeps its bytes,
//! its variants and its tokens.

#![cfg(all(feature = "routes", feature = "variants"))]

use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{HookRegistrar, HookSource, OnAsyncRequest, RequestState};
use brocanth::plugin_order::{Order, Phase};
use brocanth_storage::named::AnyRoutes;
use brocanth_storage::routes::{FileRoutes, RoutesConfig};
use brocanth_storage::{FileTokenKey, FileTokens, LocalDisk, ObjectInfo, Storage, VariantSpec};
use brocanth_test::{TestClient, TestRequest, TestResponse};

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

/// The routes of both stores, as the generated plugin installs them.
struct Files {
    routes: AnyRoutes,
}

impl Plugin for Files {
    type Config = AnyRoutes;
    type Provides = NoContext;
    const NAME: &'static str = "storage_routes";
    const INSTALLS_HOOKS: bool = true;

    fn new(routes: AnyRoutes) -> Self {
        Self { routes }
    }
}

impl HookSource for Files {
    type Families = OnAsyncRequest;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        registrar.request_async("lift", Phase::Guard, Order::new(), self.routes.csrf_lift());
        registrar.request_async("answer", Phase::Cache, Order::new(), self.routes.clone());
        Ok(())
    }
}

const SECRET: &[u8] = b"one deployment secret";

struct Rig {
    send: Box<dyn Fn(TestRequest) -> TestResponse>,
    default: Arc<dyn Storage>,
    archive: Arc<dyn Storage>,
    default_tokens: FileTokens,
    archive_tokens: FileTokens,
    _dirs: (tempfile::TempDir, tempfile::TempDir),
}

fn open() -> (Arc<dyn Storage>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn Storage> = Arc::new(LocalDisk::open(dir.path()).unwrap());
    (store, dir)
}

fn rig() -> Rig {
    let (default, a) = open();
    let (archive, b) = open();
    let default_tokens = FileTokens::new(FileTokenKey::from_secret(SECRET));
    let archive_tokens = FileTokens::new(FileTokenKey::from_secret_for(SECRET, "archive"));
    let base = RoutesConfig {
        thumb_sizes: vec!["30x20".parse().unwrap()],
        dedupe: true,
        ..RoutesConfig::default()
    };
    let open = |_: &http::Request<Vec<u8>>, _: &RequestState| true;
    let named = FileRoutes::new(
        archive.clone(),
        archive_tokens.clone(),
        RoutesConfig {
            path: "/files/archive".to_string(),
            ..base.clone()
        },
        open,
    )
    .unwrap();
    let first = FileRoutes::new(default.clone(), default_tokens.clone(), base, open).unwrap();
    let app = AppBuilder::new()
        .plugin_hooking(Files::new(AnyRoutes::new(vec![named, first])))
        .build()
        .expect("an application");
    let client = TestClient::new(app);
    Rig {
        send: Box::new(move |request| block_on(client.send_async(request))),
        default,
        archive,
        default_tokens,
        archive_tokens,
        _dirs: (a, b),
    }
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbImage::from_pixel(width, height, image::Rgb([30, 200, 30]));
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn get(rig: &Rig, target: &str) -> TestResponse {
    (rig.send)(TestRequest::get(target))
}

#[test]
fn a_token_of_one_store_is_refused_by_the_other() {
    let rig = rig();
    let key = block_on(rig.archive.put(b"archived", &ObjectInfo::new())).unwrap();
    let own = rig.archive_tokens.issue(&key);
    let other = rig.default_tokens.issue(&key);
    let at = format!("/files/archive/{}", key.to_hex());
    assert_eq!(
        get(&rig, &format!("{at}?token={}", own.as_str())).status(),
        200
    );
    assert_eq!(
        get(&rig, &format!("{at}?token={}", other.as_str())).status(),
        403
    );
    // And the archive's token at the default store's path.
    let there = format!("/files/{}?token={}", key.to_hex(), own.as_str());
    assert_eq!(get(&rig, &there).status(), 403);
}

#[test]
fn each_store_keeps_its_bytes_and_a_delete_leaves_the_other() {
    let rig = rig();
    let key = block_on(rig.default.put(b"twice", &ObjectInfo::new())).unwrap();
    assert!(!block_on(rig.archive.exists(&key)).unwrap());
    block_on(rig.archive.put_at(&key, b"twice", &ObjectInfo::new())).unwrap();
    block_on(rig.default.delete(&key)).unwrap();
    assert!(block_on(rig.archive.exists(&key)).unwrap());
    let token = rig.archive_tokens.issue(&key);
    let at = format!("/files/archive/{}?token={}", key.to_hex(), token.as_str());
    let file = get(&rig, &at);
    assert_eq!(file.status(), 200);
    assert_eq!(file.body(), b"twice");
}

#[test]
fn a_thumbnail_stays_in_the_store_of_its_object() {
    let rig = rig();
    let key = block_on(rig.archive.put(&png(300, 200), &ObjectInfo::new())).unwrap();
    let token = rig.archive_tokens.issue(&key);
    let at = format!(
        "/files/archive/{}?token={}&thumb=30x20",
        key.to_hex(),
        token.as_str()
    );
    assert_eq!(get(&rig, &at).status(), 200);
    let spec: VariantSpec = "30x20".parse().unwrap();
    assert!(block_on(rig.archive.get_variant(&key, spec))
        .unwrap()
        .is_some());
    assert!(block_on(rig.default.get_variant(&key, spec))
        .unwrap()
        .is_none());
}

#[test]
fn an_upload_to_a_named_store_reaches_its_routes_and_not_the_default() {
    let rig = rig();
    let body = b"--b\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\n\
                 Content-Type: text/plain\r\n\r\nnamed upload\r\n--b--\r\n"
        .to_vec();
    let sent = (rig.send)(
        TestRequest::post("/files/archive").body("multipart/form-data; boundary=b", body),
    );
    // The default routes would answer 405, as if `archive` were an address.
    assert_eq!(sent.status(), 201, "{}", sent.text());
    let hex = sent.text();
    let key = brocanth_storage::ObjectKey::parse(hex.trim()).unwrap();
    assert!(block_on(rig.archive.exists(&key)).unwrap());
    assert!(!block_on(rig.default.exists(&key)).unwrap());
}
