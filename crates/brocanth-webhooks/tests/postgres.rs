//! The Postgres side: idempotency keys every instance shares, and outgoing
//! deliveries that a worker posts, signs and records.
//!
//! Skipped without `BROCANTH_WEBHOOKS_TEST_POSTGRES` or
//! `BROCANTH_TEST_DATABASE_URL`. Each test works in its own schema and
//! drops it at the end, so nothing shared is truncated.

use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use brocanth_db::sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use brocanth_db::sqlx::{self, PgPool};
use brocanth_db::Db;
use brocanth_jobs::Worker;
use brocanth_webhooks::outgoing::{Deliver, QUEUE};
use brocanth_webhooks::{Outgoing, OutgoingConfig, PgSeen, Scheme, Seen};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn database_url() -> Option<String> {
    [
        "BROCANTH_WEBHOOKS_TEST_POSTGRES",
        "BROCANTH_TEST_DATABASE_URL",
    ]
    .iter()
    .find_map(|name| std::env::var(name).ok().filter(|v| !v.trim().is_empty()))
}

/// A pool on a new schema, and the admin pool that drops it.
struct Scoped {
    db: Db,
    admin: PgPool,
    schema: String,
}

impl Scoped {
    async fn drop_schema(self) {
        self.db.pool().close().await;
        let _ = sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP SCHEMA IF EXISTS {} CASCADE",
            self.schema
        )))
        .execute(&self.admin)
        .await;
    }
}

async fn scoped(test: &str) -> Option<Scoped> {
    let Some(url) = database_url() else {
        eprintln!("*** SKIPPED (no database): {test}");
        return None;
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let schema = format!(
        "brocanth_webhooks_t{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("connecting to the test database");
    for statement in [
        format!("DROP SCHEMA IF EXISTS {schema} CASCADE"),
        format!("CREATE SCHEMA {schema}"),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(statement))
            .execute(&admin)
            .await
            .expect("making the test schema");
    }
    let options = PgConnectOptions::from_str(&url)
        .expect("a Postgres URL")
        .options([("search_path", schema.as_str())]);
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await
        .expect("connecting to the test schema");
    Some(Scoped {
        db: Db::from_pool(pool),
        admin,
        schema,
    })
}

#[tokio::test]
async fn two_instances_share_the_keys_and_a_release_frees_one() {
    let Some(scope) = scoped("two_instances_share_the_keys_and_a_release_frees_one").await else {
        return;
    };
    let a = PgSeen::new(scope.db.clone());
    let b = PgSeen::new(scope.db.clone());
    a.migrate().await.expect("migrate");
    a.migrate().await.expect("migrate twice");
    let keep = Duration::from_secs(60);
    assert!(a.claim("stripe", "evt_1", keep).await.unwrap());
    assert!(
        !b.claim("stripe", "evt_1", keep).await.unwrap(),
        "b sees a's key"
    );
    assert!(
        b.claim("github", "evt_1", keep).await.unwrap(),
        "keys are per source"
    );
    a.release("stripe", "evt_1").await.unwrap();
    assert!(b.claim("stripe", "evt_1", keep).await.unwrap(), "released");
    scope.drop_schema().await;
}

/// One request a test receiver read.
#[derive(Clone, Debug)]
struct Got {
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Got {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A receiver on a free local port that answers every POST with `status`
/// and keeps what it read.
async fn receiver(status: u16) -> (String, Arc<Mutex<Vec<Got>>>) {
    receiver_with(status, "", "no").await
}

/// The same, with more header lines and a body of its own.
async fn receiver_with(
    status: u16,
    extra: &'static str,
    reply: &'static str,
) -> (String, Arc<Mutex<Vec<Got>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", listener.local_addr().unwrap());
    let got = Arc::new(Mutex::new(Vec::new()));
    let keep = Arc::clone(&got);
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let keep = Arc::clone(&keep);
            tokio::spawn(async move {
                let mut buffer = Vec::new();
                let mut chunk = [0_u8; 4096];
                let (head_end, length) = loop {
                    let n = socket.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buffer.extend_from_slice(&chunk[..n]);
                    if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buffer[..end]).to_string();
                        let length = head
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                k.eq_ignore_ascii_case("content-length")
                                    .then(|| v.trim().parse::<usize>().ok())?
                            })
                            .unwrap_or(0);
                        break (end + 4, length);
                    }
                };
                while buffer.len() < head_end + length {
                    let n = socket.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buffer.extend_from_slice(&chunk[..n]);
                }
                let head = String::from_utf8_lossy(&buffer[..head_end]).to_string();
                let headers = head
                    .lines()
                    .skip(1)
                    .filter_map(|l| l.split_once(':'))
                    .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                    .collect();
                keep.lock().unwrap().push(Got {
                    headers,
                    body: buffer[head_end..].to_vec(),
                });
                let answer = format!(
                    "HTTP/1.1 {status} X\r\n{extra}content-length: {}\r\nconnection: close\r\n\r\n{reply}",
                    reply.len()
                );
                let _ = socket.write_all(answer.as_bytes()).await;
            });
        }
    });
    (url, got)
}

fn local() -> OutgoingConfig {
    OutgoingConfig {
        allow_private: true,
        ..OutgoingConfig::default()
    }
}

async fn state_of(outgoing: &Outgoing, id: i64) -> (String, i32, Option<i32>) {
    let row = outgoing
        .deliveries(100, 0)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.id == id)
        .expect("the delivery row");
    (row.state, row.attempts, row.last_status)
}

/// The whole path: emit, the job, the worker, the POST, the row. The only
/// test here that runs the worker, because the job finds its `Outgoing`
/// through the process-wide `Outgoing::installed`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_emitted_event_reaches_the_endpoint_signed_and_the_row_says_delivered() {
    let Some(scope) =
        scoped("an_emitted_event_reaches_the_endpoint_signed_and_the_row_says_delivered").await
    else {
        return;
    };
    let outgoing = Outgoing::new(scope.db.clone(), &local()).unwrap();
    outgoing.migrate().await.expect("migrate");
    outgoing.install();
    let (url, got) = receiver(204).await;
    let endpoint = outgoing
        .add_endpoint(&url, &["invoice.*"], "test")
        .await
        .unwrap();
    // An endpoint that wants other events gets nothing.
    let (other, other_got) = receiver(204).await;
    outgoing
        .add_endpoint(&other, &["user.*"], "other")
        .await
        .unwrap();

    let ids = outgoing
        .emit("invoice.paid", &serde_json::json!({ "id": 42 }))
        .await
        .unwrap();
    assert_eq!(ids.len(), 1, "one endpoint wants the event");

    let worker = Worker::builder(outgoing.queue())
        .register::<Deliver>()
        .queues([QUEUE])
        .poll_interval(Duration::from_millis(50))
        .start()
        .expect("a worker");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let (state, _, _) = state_of(&outgoing, ids[0]).await;
        if state == "delivered" {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "still {state}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let _ = worker.shutdown(Duration::from_secs(2)).await;

    let (_, attempts, status) = state_of(&outgoing, ids[0]).await;
    assert_eq!((attempts, status), (1, Some(204)));
    assert!(other_got.lock().unwrap().is_empty());

    // What the endpoint got verifies with the endpoint's secret, through
    // the incoming side of this crate.
    let got = got.lock().unwrap()[0].clone();
    let scheme = Scheme::standard(&endpoint.secret).unwrap();
    let now = got.header("webhook-timestamp").unwrap().parse().unwrap();
    let verified = scheme
        .verify(|name: &str| got.header(name), &got.body, now, 300)
        .expect("a valid signature");
    let body: serde_json::Value = serde_json::from_slice(&got.body).unwrap();
    assert_eq!(body["type"], "invoice.paid");
    assert_eq!(body["data"]["id"], 42);
    assert!(verified.id.is_some_and(|id| id.starts_with("msg_")));
    scope.drop_schema().await;
}

#[tokio::test]
async fn a_failed_attempt_is_retried_and_a_410_turns_the_endpoint_off() {
    let Some(scope) = scoped("a_failed_attempt_is_retried_and_a_410_turns_the_endpoint_off").await
    else {
        return;
    };
    let outgoing = Outgoing::new(scope.db.clone(), &local()).unwrap();
    outgoing.migrate().await.expect("migrate");

    let (broken, _) = receiver(500).await;
    outgoing
        .add_endpoint(&broken, &["*"], "broken")
        .await
        .unwrap();
    let ids = outgoing.emit("a.b", &serde_json::json!({})).await.unwrap();
    let err = outgoing
        .attempt(ids[0], false)
        .await
        .expect_err("a 500 is retried");
    assert!(err.contains("500"), "{err}");
    assert_eq!(
        state_of(&outgoing, ids[0]).await,
        ("retrying".into(), 1, Some(500))
    );
    let _ = outgoing.attempt(ids[0], true).await;
    assert_eq!(
        state_of(&outgoing, ids[0]).await,
        ("failed".into(), 2, Some(500))
    );
    assert!(
        outgoing.redeliver(ids[0]).await.unwrap(),
        "a failed delivery is sent again"
    );
    assert_eq!(state_of(&outgoing, ids[0]).await.0, "pending");
    outgoing.set_active(1, false).await.unwrap();

    let (gone, _) = receiver(410).await;
    let endpoint = outgoing.add_endpoint(&gone, &["*"], "gone").await.unwrap();
    let ids = outgoing.emit("a.b", &serde_json::json!({})).await.unwrap();
    assert_eq!(ids.len(), 1, "only the active endpoint");
    outgoing
        .attempt(ids[0], false)
        .await
        .expect("a 410 is final");
    assert_eq!(state_of(&outgoing, ids[0]).await.0, "failed");
    let off = outgoing.endpoints().await.unwrap();
    assert!(!off.iter().find(|e| e.id == endpoint.id).unwrap().active);

    let refused = Outgoing::new(scope.db.clone(), &OutgoingConfig::default())
        .unwrap()
        .add_endpoint(&gone, &["*"], "private")
        .await
        .expect_err("a loopback endpoint is refused by default");
    assert!(refused.0.contains("help:"), "{refused}");
    scope.drop_schema().await;
}

#[tokio::test]
async fn the_receiver_shares_its_keys_through_postgres_and_a_failure_frees_one() {
    use brocanth_webhooks::sign::{decode_secret, new_secret, sign_standard};
    use brocanth_webhooks::{Delivery, Handler, Incoming, Rejection};

    let Some(scope) =
        scoped("the_receiver_shares_its_keys_through_postgres_and_a_failure_frees_one").await
    else {
        return;
    };
    #[derive(Clone, Default)]
    struct Count(Arc<AtomicU64>);
    impl Handler for Count {
        type Event = serde_json::Value;
        async fn handle(&self, event: serde_json::Value, _: &Delivery) -> Result<(), Rejection> {
            if event["fail"] == true {
                return Err(Rejection::retry("later"));
            }
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    let secret = new_secret();
    let key = decode_secret(&secret).unwrap();
    let count = Count::default();
    let seen = Arc::new(PgSeen::new(scope.db.clone()));
    seen.migrate().await.expect("migrate");
    // Two instances of the application, one table of keys.
    let instance = || {
        let receiver = Incoming::new("/webhooks")
            .source("app", Scheme::standard(&secret).unwrap(), count.clone())
            .unwrap()
            .build()
            .unwrap();
        receiver.set_seen(seen.clone());
        receiver
    };
    let (a, b) = (instance(), instance());
    let now = 1_700_000_000;
    let signed = |id: &str, body: &[u8]| {
        vec![
            ("webhook-id".to_string(), id.to_string()),
            ("webhook-timestamp".to_string(), now.to_string()),
            (
                "webhook-signature".to_string(),
                sign_standard(&key, id, now, body),
            ),
        ]
    };

    let body = br#"{"n":1}"#;
    let h = signed("msg_pg", body);
    let first = a.receive_at("app", look(&h), body, now).await;
    assert_eq!(first.message, "ok");
    let copy = b.receive_at("app", look(&h), body, now).await;
    assert_eq!(copy.message, "already received", "b sees a's key");

    let failing = br#"{"fail":true}"#;
    let h = signed("msg_fail", failing);
    let first = a.receive_at("app", look(&h), failing, now).await;
    assert_eq!(first.status, 500);
    let retry = b.receive_at("app", look(&h), failing, now).await;
    assert_eq!(
        retry.status, 500,
        "the key was given back in the table: {retry:?}"
    );
    assert_eq!(count.0.load(Ordering::SeqCst), 1);
    scope.drop_schema().await;
}

/// Each retry of one delivery carries the same `webhook-id`, and the
/// incoming side of this crate verifies it and drops the copy.
#[tokio::test]
async fn every_retry_carries_one_id_and_the_incoming_receiver_drops_the_copy() {
    use brocanth_webhooks::{Delivery, Handler, Incoming, Rejection};

    let Some(scope) =
        scoped("every_retry_carries_one_id_and_the_incoming_receiver_drops_the_copy").await
    else {
        return;
    };
    let outgoing = Outgoing::new(scope.db.clone(), &local()).unwrap();
    outgoing.migrate().await.expect("migrate");
    let (url, got) = receiver(503).await;
    let endpoint = outgoing.add_endpoint(&url, &["*"], "flaky").await.unwrap();
    let ids = outgoing
        .emit("invoice.paid", &serde_json::json!({ "id": 7 }))
        .await
        .unwrap();
    for _ in 0..2 {
        let err = outgoing.attempt(ids[0], false).await.expect_err("a 503");
        assert!(err.contains("503"), "{err}");
    }
    assert_eq!(
        state_of(&outgoing, ids[0]).await,
        ("retrying".into(), 2, Some(503))
    );
    let sent = got.lock().unwrap().clone();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].header("webhook-id"), sent[1].header("webhook-id"));
    assert_eq!(sent[0].body, sent[1].body);
    assert_eq!(sent[0].header("content-type"), Some("application/json"));

    #[derive(Clone, Default)]
    struct Count(Arc<AtomicU64>);
    impl Handler for Count {
        type Event = serde_json::Value;
        async fn handle(&self, event: serde_json::Value, d: &Delivery) -> Result<(), Rejection> {
            assert_eq!(event["type"], "invoice.paid");
            assert_eq!(event["data"]["id"], 7);
            assert!(d.id.starts_with("msg_"));
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    let count = Count::default();
    let incoming = Incoming::new("/webhooks")
        .source(
            "partner",
            Scheme::standard(&endpoint.secret).unwrap(),
            count.clone(),
        )
        .unwrap()
        .build()
        .unwrap();
    for (n, one) in sent.iter().enumerate() {
        let answer = incoming
            .receive("partner", |name| one.header(name), &one.body)
            .await;
        let want = if n == 0 { "ok" } else { "already received" };
        assert_eq!(answer.message, want, "attempt {n}");
    }
    assert_eq!(count.0.load(Ordering::SeqCst), 1);

    // Signed with another endpoint's secret, it is refused.
    let other = Incoming::new("/webhooks")
        .source(
            "partner",
            Scheme::standard(&brocanth_webhooks::sign::new_secret()).unwrap(),
            count.clone(),
        )
        .unwrap()
        .build()
        .unwrap();
    let answer = other
        .receive("partner", |name| sent[0].header(name), &sent[0].body)
        .await;
    assert_eq!(answer.status, 401);
    scope.drop_schema().await;
}

#[tokio::test]
async fn a_redirect_is_a_failed_attempt_and_is_not_followed() {
    let Some(scope) = scoped("a_redirect_is_a_failed_attempt_and_is_not_followed").await else {
        return;
    };
    let outgoing = Outgoing::new(scope.db.clone(), &local()).unwrap();
    outgoing.migrate().await.expect("migrate");
    let (target, target_got) = receiver(204).await;
    let location: &'static str = Box::leak(format!("location: {target}\r\n").into_boxed_str());
    let (url, _) = receiver_with(307, location, "").await;
    outgoing.add_endpoint(&url, &["*"], "moved").await.unwrap();
    let ids = outgoing.emit("a.b", &serde_json::json!({})).await.unwrap();
    let err = outgoing.attempt(ids[0], false).await.expect_err("a 307");
    assert!(err.contains("307"), "{err}");
    assert_eq!(state_of(&outgoing, ids[0]).await.2, Some(307));
    assert!(
        target_got.lock().unwrap().is_empty(),
        "the redirect was followed"
    );
    scope.drop_schema().await;
}

#[tokio::test]
async fn an_unreachable_endpoint_is_retried_and_its_last_attempt_fails_it() {
    let Some(scope) =
        scoped("an_unreachable_endpoint_is_retried_and_its_last_attempt_fails_it").await
    else {
        return;
    };
    let outgoing = Outgoing::new(scope.db.clone(), &local()).unwrap();
    outgoing.migrate().await.expect("migrate");
    // A port that was free a moment ago: nothing listens on it.
    let closed = {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        format!("http://{}/hook", listener.local_addr().unwrap())
    };
    outgoing
        .add_endpoint(&closed, &["*"], "down")
        .await
        .unwrap();
    let ids = outgoing.emit("a.b", &serde_json::json!({})).await.unwrap();
    let err = outgoing
        .attempt(ids[0], false)
        .await
        .expect_err("unreachable");
    assert!(err.contains("could not reach"), "{err}");
    assert_eq!(
        state_of(&outgoing, ids[0]).await,
        ("retrying".into(), 1, None)
    );
    let _ = outgoing.attempt(ids[0], true).await;
    assert_eq!(
        state_of(&outgoing, ids[0]).await,
        ("failed".into(), 2, None)
    );
    scope.drop_schema().await;
}

#[tokio::test]
async fn a_delivered_row_is_not_sent_again_and_an_endpoint_turned_off_gets_nothing() {
    let Some(scope) =
        scoped("a_delivered_row_is_not_sent_again_and_an_endpoint_turned_off_gets_nothing").await
    else {
        return;
    };
    let outgoing = Outgoing::new(scope.db.clone(), &local()).unwrap();
    outgoing.migrate().await.expect("migrate");
    let (url, got) = receiver(200).await;
    let endpoint = outgoing.add_endpoint(&url, &["*"], "ok").await.unwrap();
    let ids = outgoing.emit("a.b", &serde_json::json!({})).await.unwrap();
    outgoing.attempt(ids[0], false).await.unwrap();
    outgoing.attempt(ids[0], false).await.unwrap();
    assert_eq!(got.lock().unwrap().len(), 1, "sent once");
    assert_eq!(
        state_of(&outgoing, ids[0]).await,
        ("delivered".into(), 1, Some(200))
    );
    assert!(
        !outgoing.redeliver(ids[0]).await.unwrap(),
        "a delivered row is not sent again"
    );

    let pending = outgoing.emit("a.b", &serde_json::json!({})).await.unwrap();
    outgoing.set_active(endpoint.id, false).await.unwrap();
    outgoing.attempt(pending[0], false).await.expect("final");
    let (state, _, _) = state_of(&outgoing, pending[0]).await;
    assert_eq!(state, "failed");
    assert_eq!(
        got.lock().unwrap().len(),
        1,
        "an endpoint that is off gets nothing"
    );
    assert!(outgoing
        .emit("a.b", &serde_json::json!({}))
        .await
        .unwrap()
        .is_empty());
    assert!(outgoing.remove_endpoint(endpoint.id).await.unwrap());
    // Its deliveries went with it, and an attempt on one is a no-op.
    outgoing.attempt(pending[0], false).await.expect("gone");
    scope.drop_schema().await;
}

/// A row written before `allow_private` was turned off, or by SQL that
/// never passed `add_endpoint`, is still refused at the attempt.
#[tokio::test]
async fn a_private_endpoint_already_in_the_table_is_refused_at_the_attempt() {
    let Some(scope) =
        scoped("a_private_endpoint_already_in_the_table_is_refused_at_the_attempt").await
    else {
        return;
    };
    let lax = Outgoing::new(scope.db.clone(), &local()).unwrap();
    lax.migrate().await.expect("migrate");
    let (url, got) = receiver(204).await;
    lax.add_endpoint(&url, &["*"], "inside").await.unwrap();
    let ids = lax.emit("a.b", &serde_json::json!({})).await.unwrap();

    let strict = Outgoing::new(scope.db.clone(), &OutgoingConfig::default()).unwrap();
    strict
        .attempt(ids[0], false)
        .await
        .expect("refused for good, not retried");
    let row = strict
        .deliveries(10, 0)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.id == ids[0])
        .unwrap();
    assert_eq!(row.state, "failed");
    assert!(
        row.last_error
            .as_deref()
            .unwrap_or("")
            .contains("own network"),
        "{:?}",
        row.last_error
    );
    assert!(got.lock().unwrap().is_empty(), "the request was sent");
    scope.drop_schema().await;
}

#[tokio::test]
async fn an_endpoint_url_that_is_not_http_or_is_private_is_refused_when_added() {
    let Some(scope) =
        scoped("an_endpoint_url_that_is_not_http_or_is_private_is_refused_when_added").await
    else {
        return;
    };
    let outgoing = Outgoing::new(scope.db.clone(), &OutgoingConfig::default()).unwrap();
    outgoing.migrate().await.expect("migrate");
    for url in [
        "ftp://example.com/",
        "javascript:alert(1)",
        "file:///etc/passwd",
        "http://[::ffff:127.0.0.1]/",
        "http://169.254.169.254/latest/meta-data/",
        "http://2130706433/",
        "not a url",
    ] {
        let err = outgoing
            .add_endpoint(url, &["*"], "x")
            .await
            .expect_err(url);
        assert!(err.0.contains("help:"), "{err}");
    }
    assert!(outgoing.endpoints().await.unwrap().is_empty());
    scope.drop_schema().await;
}

/// The admin tab shows what a receiver answered. A receiver is not trusted:
/// its answer is escaped, and only a person who may act sees the button.
#[tokio::test]
async fn the_admin_tab_escapes_a_receivers_answer_and_hides_the_button_from_a_reader() {
    use brocanth_admin::form::Submission;
    use brocanth_admin::panel::{Panel, PanelPage, PanelRequest};
    use brocanth_webhooks::admin::DeliveriesPanel;

    let Some(scope) =
        scoped("the_admin_tab_escapes_a_receivers_answer_and_hides_the_button_from_a_reader").await
    else {
        return;
    };
    let outgoing = Outgoing::new(scope.db.clone(), &local()).unwrap();
    outgoing.migrate().await.expect("migrate");
    let (url, _) = receiver_with(500, "", "<script>alert(1)</script>").await;
    outgoing
        .add_endpoint(&url, &["<img src=x onerror=alert(2)>"], "x")
        .await
        .unwrap();
    let ids = outgoing
        .emit("<img src=x onerror=alert(2)>", &serde_json::json!({}))
        .await
        .unwrap();
    let _ = outgoing.attempt(ids[0], false).await;

    let panel = DeliveriesPanel::new(outgoing.clone());
    let submission = Submission::default();
    let render = |can_act: bool, post: bool, path: &'static str| {
        let panel = &panel;
        let submission = &submission;
        async move {
            panel
                .render(PanelRequest {
                    path,
                    query: "",
                    post,
                    can_act,
                    submission,
                    mount: "/admin/webhooks",
                })
                .await
                .unwrap()
        }
    };
    let Some(PanelPage::Body { html, .. }) = render(true, false, "").await else {
        panic!("a body");
    };
    assert!(!html.contains("<script>"), "{html}");
    assert!(!html.contains("<img"), "{html}");
    assert!(html.contains("&lt;script&gt;"), "{html}");
    assert!(html.contains("Send again"));

    let Some(PanelPage::Body { html, .. }) = render(false, false, "").await else {
        panic!("a body");
    };
    assert!(!html.contains("Send again"), "a reader sees no button");

    // A POST from someone who may not act changes nothing.
    let leaked: &'static str = Box::leak(format!("redeliver/{}", ids[0]).into_boxed_str());
    let before = state_of(&outgoing, ids[0]).await;
    render(false, true, leaked).await;
    assert_eq!(state_of(&outgoing, ids[0]).await, before);
    let Some(PanelPage::Redirect { .. }) = render(true, true, leaked).await else {
        panic!("a redirect after the button");
    };
    assert_eq!(state_of(&outgoing, ids[0]).await.0, "pending");
    scope.drop_schema().await;
}

/// Reads a header out of pairs, by its lower-case name.
fn look<'a>(headers: &'a [(String, String)]) -> impl Fn(&str) -> Option<&'a str> + 'a {
    move |name| {
        headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}
