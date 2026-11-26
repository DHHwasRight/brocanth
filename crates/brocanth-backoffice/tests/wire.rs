//! The tab data contract over HTTP (docs/backoffice.md §7.5): two
//! applications in one test.
//!
//! The application's half, a `WireServer` over the admin's in-memory
//! fixture, listens on a real socket in a runtime of its own. The
//! backoffice's half, a `WireClient`, reaches it through HTTP: every call,
//! every transaction and every panel call crosses the socket, signed. The
//! admin's screens run over it through `TestClient`.

use std::sync::Arc;
use std::time::Duration;

use brocanth::plugin_api::Plugin as _;
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_hooks::{Request as HookRequest, RequestState};
use brocanth_admin::data::{AdminData, RequestCx, Scope, Search, SharedData};
use brocanth_admin::model::{ModelDescriptor, PrimaryKey, Value};
use brocanth_admin::panel::remote::PanelWire;
use brocanth_admin::policy::{policy_fn, Level, Principal};
use brocanth_admin::{discover, AdminConfig, AdminPlugin, AdminSetup, Discovery, Fixture};
use brocanth_backoffice::wire::{
    self, Ask, ServiceSecret, WireClient, WireServer, NONCE_HEADER, PATH, SIGNATURE_HEADER,
    TIME_HEADER,
};
use brocanth_backoffice::{
    Call, Caller, Claim, Contracted, ErrorKind, Op, Read, Scope as TabScope, Search as TabSearch,
    TabData, Write,
};
use brocanth_flags::admin::{FlagsData, FlagsPanel, FlagsServe, RemoteFlags};
use brocanth_flags::{Flags, MemoryStore, Rule};
use brocanth_server::{Response, Server, Stopper};
use brocanth_test::{TestClient, TestRequest};

const SECRET: &str = "a-service-secret-of-thirty-two-bytes-or-more";

fn secret() -> ServiceSecret {
    ServiceSecret::parse(SECRET).expect("long enough")
}

/// The application's half, listening on its own runtime and thread: the
/// address, and what stops it.
struct Serving {
    base: String,
    stopper: Stopper,
}

impl Drop for Serving {
    fn drop(&mut self) {
        self.stopper.stop();
    }
}

fn serve(server: WireServer) -> Serving {
    let (sent, got) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("a runtime");
        runtime.block_on(async move {
            let listening = Server::bind("127.0.0.1:0").await.expect("a port");
            let addr = listening.local_addr().expect("an address");
            sent.send((addr, listening.stopper()))
                .expect("the test waits");
            listening
                .run(move |request| {
                    let server = server.clone();
                    async move {
                        if server.answers(&request.path) {
                            server.handle(request).await
                        } else {
                            Response::text(404, "Not Found")
                        }
                    }
                })
                .await
                .expect("it serves");
        });
    });
    let (addr, stopper) = got.recv().expect("the server started");
    Serving {
        base: format!("http://{addr}"),
        stopper,
    }
}

fn post(data: &dyn AdminData) -> ModelDescriptor {
    discover(data.registry(), &Discovery::new())
        .into_iter()
        .find(|m| m.key == "post")
        .expect("the fixture holds `post`")
}

fn everything() -> Search<'static> {
    Search {
        term: "",
        columns: &[],
    }
}

/// The admin's end over the wire, bound to `claim` as a request would bind
/// it.
fn bound(client: &WireClient, claim: &Claim) -> SharedData {
    Contracted::over(client.clone().shared())
        .for_request(&RequestCx::new(Principal::new(claim)))
        .expect("a bound source")
}

fn editor() -> Claim {
    Claim::staff("2", false, ["post.edit", "user.read"])
}

fn reader() -> Claim {
    Claim::staff("1", false, ["post.read"])
}

async fn title(data: &dyn AdminData, id: &PrimaryKey) -> Option<String> {
    let model = post(data);
    data.fetch(&model, id, Scope::Unscoped)
        .await
        .expect("a fetch")
        .and_then(|row| row.get("title").map(ToString::to_string))
}

fn new_post(title: &str) -> Vec<(String, Value)> {
    vec![
        ("title".into(), title.into()),
        ("body".into(), "".into()),
        ("author_id".into(), 1.into()),
        ("published".into(), false.into()),
        ("views".into(), 0.into()),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn each_read_and_write_crosses_the_socket() {
    let fixture = Arc::new(Fixture::blog());
    let serving = serve(WireServer::new(fixture.clone(), secret()));
    let client = WireClient::connect(&serving.base, secret())
        .await
        .expect("it connects");
    assert_eq!(client.registry(), fixture.registry(), "read at connect");
    assert!(client.check().is_ok());

    let data = bound(&client, &editor());
    let model = post(&*data);
    let count = data
        .count(&model, everything(), Scope::Unscoped)
        .await
        .expect("a count");
    assert_eq!(
        count,
        fixture
            .count(&model, everything(), Scope::Unscoped)
            .await
            .unwrap()
    );
    let rows = data
        .list(brocanth_admin::data::ListRequest {
            model: &model,
            offset: 0,
            limit: 10,
            search: everything(),
            sort: None,
            filter: None,
            scope: Scope::Unscoped,
        })
        .await
        .expect("a page");
    assert_eq!(rows.len(), count.min(10));

    let key = data
        .insert(&model, &new_post("Over the wire"))
        .await
        .expect("an insert");
    assert_eq!(
        title(&*fixture, &key).await.as_deref(),
        Some("Over the wire")
    );
    let changed = data
        .update(
            &model,
            &key,
            &[("title".into(), "Renamed over the wire".into())],
            Scope::Unscoped,
        )
        .await
        .expect("an update");
    assert_eq!(changed, 1);
    assert_eq!(
        title(&*data, &key).await.as_deref(),
        Some("Renamed over the wire")
    );
    assert_eq!(data.delete(&model, &key, Scope::Unscoped).await.unwrap(), 1);
    assert_eq!(title(&*fixture, &key).await, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_transaction_is_held_between_requests() {
    let fixture = Arc::new(Fixture::blog());
    let server = WireServer::new(fixture.clone(), secret());
    let serving = serve(server.clone());
    let client = WireClient::connect(&serving.base, secret())
        .await
        .expect("it connects");
    let data = bound(&client, &editor());
    let model = post(&*data);
    let one = PrimaryKey::single("id", 1);
    let rename = |name: &str| vec![("title".to_string(), Value::from(name))];

    // Committed: both writes kept, after three requests.
    let mut tx = data.begin().await.expect("a transaction");
    assert_eq!(server.open_transactions(), 1);
    let key = tx
        .insert(&model, &new_post("In a transaction"))
        .await
        .unwrap();
    tx.update(&model, &one, &rename("Kept"), Scope::Unscoped)
        .await
        .unwrap();
    assert_eq!(title(&*fixture, &key).await, None, "not before the commit");
    tx.commit().await.expect("a commit");
    assert_eq!(server.open_transactions(), 0);
    assert_eq!(title(&*fixture, &one).await.as_deref(), Some("Kept"));
    assert_eq!(
        title(&*fixture, &key).await.as_deref(),
        Some("In a transaction")
    );

    // Rolled back: nothing kept.
    let mut tx = data.begin().await.expect("a transaction");
    tx.update(&model, &one, &rename("Gone"), Scope::Unscoped)
        .await
        .unwrap();
    tx.rollback().await.expect("a rollback");
    assert_eq!(title(&*fixture, &one).await.as_deref(), Some("Kept"));

    // Dropped: the client asks for a rollback.
    let mut tx = data.begin().await.expect("a transaction");
    tx.update(&model, &one, &rename("Dropped"), Scope::Unscoped)
        .await
        .unwrap();
    drop(tx);
    wait_until(|| server.open_transactions() == 0).await;
    assert_eq!(title(&*fixture, &one).await.as_deref(), Some("Kept"));
}

async fn wait_until(done: impl Fn() -> bool) {
    for _ in 0..200 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the condition never held");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_idle_transaction_rolls_back_and_the_cap_holds() {
    let fixture = Arc::new(Fixture::blog());
    let server = WireServer::new(fixture.clone(), secret()).holding(Duration::from_millis(200), 2);
    let serving = serve(server.clone());
    let client = WireClient::connect(&serving.base, secret())
        .await
        .expect("it connects");
    let claim = editor();

    let first = client.begin(&claim).await.expect("one");
    let _second = client.begin(&claim).await.expect("two");
    let third = client.begin(&claim).await.err().expect("past the cap");
    assert!(third.message.contains("2 open transactions"), "{third}");
    assert!(third.to_string().contains("tx_max"), "{third}");

    // Idle past the timeout: the reaper rolls both back, with no request.
    wait_until(|| server.open_transactions() == 0).await;
    let mut first = first;
    let write = Write::Update {
        model: "post".into(),
        key: PrimaryKey::single("id", 1),
        values: vec![("title".into(), "Late".into())],
        scope: TabScope::Unscoped,
    };
    let caller = Caller {
        tab: "post".into(),
        level: Level::Edit,
        claim: claim.clone(),
    };
    let gone = first.call(caller, write).await.expect_err("timed out");
    assert!(gone.message.contains("rolled back"), "{gone}");
    assert_eq!(
        title(&*fixture, &PrimaryKey::single("id", 1))
            .await
            .as_deref(),
        Some("On the Analytical Engine")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_data_side_checks_every_grant_on_the_wire() {
    let fixture = Arc::new(Fixture::blog());
    let server = WireServer::new(fixture.clone(), secret());
    let serving = serve(server.clone());
    let client = WireClient::connect(&serving.base, secret())
        .await
        .expect("it connects");
    let model = post(&*bound(&client, &reader()));

    // A reader reads, and cannot write.
    let as_reader = bound(&client, &reader());
    assert!(as_reader
        .count(&model, everything(), Scope::Unscoped)
        .await
        .is_ok());
    let refused = as_reader
        .insert(&model, &new_post("Forged"))
        .await
        .expect_err("a reader does not write");
    assert!(refused.to_string().contains("post.edit"), "{refused}");

    // Nobody reads nothing: the wire turns the check on, always.
    let call = Call {
        caller: Caller {
            tab: "post".into(),
            level: Level::Read,
            claim: Claim::anonymous(),
        },
        op: Op::Read(Read::Count {
            model: "post".into(),
            search: TabSearch::default(),
            scope: TabScope::Unscoped,
        }),
    };
    let nobody = client.call(call).await.expect_err("refused");
    assert_eq!(nobody.kind, ErrorKind::Refused);

    // A transaction belongs to the subject that opened it.
    let mut tx = client.begin(&editor()).await.expect("a transaction");
    let other = Caller {
        tab: "post".into(),
        level: Level::Edit,
        claim: Claim::staff("9", false, ["post.edit"]),
    };
    let write = Write::Delete {
        model: "post".into(),
        key: PrimaryKey::single("id", 1),
        scope: TabScope::Unscoped,
    };
    let stolen = tx.call(other, write).await.expect_err("another subject");
    assert_eq!(stolen.kind, ErrorKind::Refused);
    tx.rollback().await.expect("still open for its owner");
    assert!(title(&*fixture, &PrimaryKey::single("id", 1))
        .await
        .is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bad_secret_is_refused_at_connect_and_per_request() {
    let serving = serve(WireServer::new(Arc::new(Fixture::blog()), secret()));

    // Another secret: the server refuses the request.
    let other = ServiceSecret::parse("another-secret-of-thirty-two-bytes-long").expect("long");
    let refused = WireClient::connect(&serving.base, other)
        .await
        .expect_err("another secret");
    assert_eq!(refused.kind, ErrorKind::Refused);
    assert!(refused.to_string().contains("service_secret"), "{refused}");

    // A client with no secret, or a short one, fails its check at start,
    // before it sends anything.
    let lazy = WireClient::lazy(&serving.base, None);
    let error = lazy.check().expect_err("no secret");
    assert!(error.to_string().contains("service_secret"), "{error}");
    assert!(WireClient::lazy(&serving.base, Some("short"))
        .check()
        .is_err());
    assert!(WireClient::lazy("app:3000", Some(SECRET)).check().is_err());

    // The same client, right, connects lazily on a thread of its own.
    let lazy = WireClient::lazy(&serving.base, Some(SECRET));
    assert!(lazy.check().is_ok());
    assert!(!lazy.registry().is_empty());

    // Nobody listens: the check names the setting to fix.
    let down = WireClient::lazy("http://127.0.0.1:9", Some(SECRET));
    let error = down.check().expect_err("nobody listens");
    assert!(
        error.to_string().contains("--backoffice.wire=true"),
        "{error}"
    );
}

/// One raw request to the wire: the status and the body.
async fn raw(base: &str, headers: &[(&str, &str)], body: &[u8]) -> (u16, String) {
    let mut request = reqwest::Client::new().post(format!("{base}{PATH}"));
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = request
        .body(body.to_vec())
        .send()
        .await
        .expect("it answers");
    let status = response.status().as_u16();
    (status, response.text().await.unwrap_or_default())
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unsigned_a_stale_and_a_replayed_request_are_refused() {
    let serving = serve(WireServer::new(Arc::new(Fixture::blog()), secret()));
    let body = serde_json::to_vec(&Ask::Hello {
        version: wire::VERSION,
    })
    .unwrap();

    let (status, text) = raw(&serving.base, &[], &body).await;
    assert_eq!(status, 401, "{text}");
    assert!(text.contains("service secret"), "{text}");

    // Signed a few minutes ago.
    let stale = wire::sign(&secret(), PATH, &body, unix_now() - 300);
    let headers = [
        (TIME_HEADER, stale.time.as_str()),
        (NONCE_HEADER, stale.nonce.as_str()),
        (SIGNATURE_HEADER, stale.signature.as_str()),
    ];
    let (status, text) = raw(&serving.base, &headers, &body).await;
    assert_eq!(status, 401, "{text}");

    // Right once, refused the second time.
    let fresh = wire::sign(&secret(), PATH, &body, unix_now());
    let headers = [
        (TIME_HEADER, fresh.time.as_str()),
        (NONCE_HEADER, fresh.nonce.as_str()),
        (SIGNATURE_HEADER, fresh.signature.as_str()),
    ];
    assert_eq!(raw(&serving.base, &headers, &body).await.0, 200);
    let (status, text) = raw(&serving.base, &headers, &body).await;
    assert_eq!(status, 401);
    assert!(text.contains("already answered"), "{text}");

    // The signature binds the body.
    let other = wire::sign(&secret(), PATH, &body, unix_now());
    let headers = [
        (TIME_HEADER, other.time.as_str()),
        (NONCE_HEADER, other.nonce.as_str()),
        (SIGNATURE_HEADER, other.signature.as_str()),
    ];
    let forged = br#"{"Begin":{"subject":"1","superuser":true,"grants":[]}}"#;
    assert_eq!(raw(&serving.base, &headers, forged).await.0, 401);

    // Another verb.
    let response = reqwest::get(format!("{}{PATH}", serving.base))
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 405);
}

// -- the admin's screens over the wire -----------------------------------

/// A member of staff, as a session would give one.
struct Staff {
    claim: Claim,
}

fn principal<'a>(request: &'a HookRequest, _: &'a RequestState) -> Principal<'a> {
    static READER: std::sync::LazyLock<Staff> = std::sync::LazyLock::new(|| Staff {
        claim: Claim::staff("1", false, ["post.read", "user.read", "flags.read"]),
    });
    static EDITOR: std::sync::LazyLock<Staff> = std::sync::LazyLock::new(|| Staff {
        claim: Claim::staff("2", false, ["post.edit", "user.read", "flags.edit"]),
    });
    match request
        .headers()
        .get("x-staff")
        .and_then(|v| v.to_str().ok())
    {
        Some("reader") => Principal::new(&*READER),
        Some("editor") => Principal::new(&*EDITOR),
        _ => Principal::anonymous(),
    }
}

fn claim(principal: &Principal<'_>) -> Claim {
    principal
        .downcast::<Staff>()
        .map_or_else(Claim::anonymous, |staff| staff.claim.clone())
}

/// The backoffice: the admin, its rows and its flags tab over the wire, and
/// a policy that lets everybody in, so every refusal below is the data
/// side's.
fn backoffice(
    client: &WireClient,
) -> TestClient<impl Sized + 'static, brocanth::plugin_hooks::NoHooks> {
    let tabs = client.clone();
    let panels: Arc<dyn PanelWire> = client.panels(claim);
    let config = AdminConfig::new()
        .at("/admin")
        .discovering(Discovery::new().under("blog::models"))
        .unscoped_rows_unsafe_for_development()
        .with_policy(policy_fn(|_: &Principal<'_>, _| true))
        .panel(FlagsPanel::over(Arc::new(RemoteFlags::new(panels)), &[]))
        .data_through(move |_local| {
            Contracted::over(tabs.clone().shared())
                .claim(claim)
                .shared()
        });
    let mut app = AppBuilder::new()
        .plugin(AdminPlugin::new(
            // The backoffice's own source is never read: `data_through`
            // replaces it with the wire.
            AdminSetup::new(Arc::new(Fixture::new()), config).principal(principal),
        ))
        .build()
        .expect("an application");
    app.mount_routes::<AdminPlugin>("/admin")
        .expect("the routes");
    TestClient::new(app)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_admin_screens_run_over_the_wire() {
    let fixture = Arc::new(Fixture::blog());
    let flags = Flags::with_store(Arc::new(MemoryStore::new()));
    flags.set("beta", Rule::everyone()).await.expect("stored");
    let server = WireServer::new(fixture.clone(), secret())
        .panel("flags", FlagsServe::new(Arc::new(flags.clone())).shared());
    let serving = serve(server);
    let client = WireClient::connect(&serving.base, secret())
        .await
        .expect("it connects");
    let admin = backoffice(&client);

    let list = admin
        .send_async(TestRequest::get("/admin/post").header("x-staff", "reader"))
        .await;
    assert_eq!(list.status(), 200, "{}", list.text());
    list.html().assert_text("On the Analytical Engine");

    let edit = [
        ("title", "Edited over the wire"),
        ("body", "Notes."),
        ("author_id", "1"),
        ("published", "on"),
        ("views", "1843"),
        ("published_at", "1843-10-01"),
    ];
    let refused = admin
        .send_async(
            TestRequest::post("/admin/post/1")
                .header("x-staff", "reader")
                .form(&edit),
        )
        .await;
    assert_ne!(
        refused.status(),
        303,
        "the data side refuses a reader's write"
    );
    let one = PrimaryKey::single("id", 1);
    assert_eq!(
        title(&*fixture, &one).await.as_deref(),
        Some("On the Analytical Engine")
    );
    let saved = admin
        .send_async(
            TestRequest::post("/admin/post/1")
                .header("x-staff", "editor")
                .form(&edit),
        )
        .await;
    assert_eq!(saved.status(), 303);
    assert_eq!(
        title(&*fixture, &one).await.as_deref(),
        Some("Edited over the wire")
    );

    // The flags tab reads the application's flags over the wire.
    let tab = admin
        .send_async(TestRequest::get("/admin/flags").header("x-staff", "reader"))
        .await;
    assert_eq!(tab.status(), 200, "{}", tab.text());
    tab.html().assert_text("beta");
    let nobody = admin.send_async(TestRequest::get("/admin/flags")).await;
    assert!(
        nobody.text().contains("could not be read"),
        "the data side refuses nobody: {}",
        nobody.text()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_panel_call_crosses_the_wire_with_its_grant_checked() {
    let flags = Flags::with_store(Arc::new(MemoryStore::new()));
    let server = WireServer::new(Arc::new(Fixture::blog()), secret())
        .panel("flags", FlagsServe::new(Arc::new(flags.clone())).shared());
    let serving = serve(server);
    let client = WireClient::connect(&serving.base, secret())
        .await
        .expect("it connects");
    let remote = RemoteFlags::new(client.panels(|principal: &Principal<'_>| {
        principal
            .downcast::<Claim>()
            .cloned()
            .unwrap_or_else(Claim::anonymous)
    }));
    let editor = Claim::staff("2", false, ["flags.edit"]);
    let reader = Claim::staff("1", false, ["flags.read"]);

    remote
        .set(&Principal::new(&editor), "beta", Rule::percent(25))
        .await
        .expect("an editor sets a rule");
    assert_eq!(flags.rule("beta"), Some(Rule::percent(25)));
    let view = remote
        .read(&Principal::new(&reader))
        .await
        .expect("a reader reads");
    assert_eq!(view.rule("beta"), Some(&Rule::percent(25)));
    let refused = remote
        .remove(&Principal::new(&reader), "beta")
        .await
        .expect_err("a reader does not write");
    assert!(refused.contains("flags.edit"), "{refused}");
    assert!(remote.read(&Principal::anonymous()).await.is_err());
    remote
        .remove(&Principal::new(&editor), "beta")
        .await
        .expect("an editor removes it");
    assert_eq!(flags.rule("beta"), None);
}

/// Each tab serves itself: the application's wire serves every tab of its
/// admin that has a data contract (`AdminConfig::panel_serves`), and one
/// call turns the backoffice's tabs remote (`AdminConfig::panels_through`).
/// The backoffice's own flags cell stays empty: what the tab shows came
/// over the socket.
#[tokio::test(flavor = "multi_thread")]
async fn each_tab_serves_itself_over_the_wire() {
    use brocanth_admin::late::Late;
    use brocanth_admin::{Admin, Submission};

    let flags = Flags::with_store(Arc::new(MemoryStore::new()));
    flags.set("beta", Rule::everyone()).await.expect("stored");
    let application = AdminConfig::new().panel(FlagsPanel::new(flags.clone(), &[]));
    let served = application.panel_serves();
    assert_eq!(
        served
            .iter()
            .map(|(tab, _)| tab.as_str())
            .collect::<Vec<_>>(),
        ["flags"]
    );
    let server = served.into_iter().fold(
        WireServer::new(Arc::new(Fixture::blog()), secret()),
        |server, (tab, serve)| server.panel(tab, serve),
    );
    let serving = serve(server);
    let client = WireClient::connect(&serving.base, secret())
        .await
        .expect("it connects");
    let config = AdminConfig::new()
        .panel(FlagsPanel::late(Late::new(), &[]))
        .panels_through(client.panels(claim))
        .unscoped_rows_unsafe_for_development()
        .with_policy(policy_fn(|_: &Principal<'_>, _| true));
    let admin = Admin::try_new(Arc::new(Fixture::blog()), config).expect("the admin builds");
    let reader = Staff {
        claim: Claim::staff("1", false, ["flags.read"]),
    };
    let editor = Staff {
        claim: Claim::staff("2", false, ["flags.edit"]),
    };

    let page = admin
        .dispatch(
            Principal::new(&reader),
            "GET",
            "/admin/flags",
            "",
            &Submission::default(),
        )
        .await
        .expect("the tab");
    assert!(
        page.html().is_some_and(|html| html.contains("beta")),
        "{page:?}"
    );
    let form = Submission::from_pairs([("percent", "25")]);
    admin
        .dispatch(
            Principal::new(&reader),
            "POST",
            "/admin/flags/beta",
            "",
            &form,
        )
        .await
        .expect("a page");
    assert_eq!(
        flags.rule("beta"),
        Some(Rule::everyone()),
        "the data side refuses a reader's write"
    );
    let saved = admin
        .dispatch(
            Principal::new(&editor),
            "POST",
            "/admin/flags/beta",
            "",
            &form,
        )
        .await
        .expect("a page");
    assert_eq!(saved.status(), 303);
    assert_eq!(flags.rule("beta"), Some(Rule::percent(25)));
}
