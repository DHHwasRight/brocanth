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
                    "HTTP/1.1 {status} X\r\ncontent-length: 2\r\nconnection: close\r\n\r\nno"
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
