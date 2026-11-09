//! Outgoing webhooks: endpoints, signed deliveries, retries and a log.
//!
//! # The shape
//!
//! Two tables in the application's database: the endpoints, each with its
//! own secret and the events it wants, and the deliveries, one row per event
//! per endpoint. [`Outgoing::emit`] writes the rows and enqueues one
//! [`Deliver`] job per row. The job posts the event, signed with Standard
//! Webhooks, and writes the outcome on the row.
//!
//! # Why the retries are the queue's
//!
//! A failed delivery is a failed job. `brocanth-jobs` already retries a job
//! with exponential backoff and jitter, keeps a job that ran out of attempts
//! in its dead letter, and survives a restart, so this crate writes none of
//! that. [`Deliver`] declares eight attempts over about a day; the row says
//! where each delivery is, for the panel and for the application.
//!
//! Only the public surface of `brocanth-jobs` is used: [`Job`], the
//! enqueue, and a worker over this crate's own queue name. The queue is
//! built over the `db` module's pool, so the deliveries and their jobs live
//! in one database.
//!
//! # What a receiver gets
//!
//! A POST with `content-type: application/json` and a body
//! `{"type": "<event>", "timestamp": <unix>, "data": <payload>}`, and the
//! three Standard Webhooks headers. `webhook-id` is the same on every retry
//! of one delivery, so the receiver can drop a duplicate. A `410 Gone`
//! answer turns the endpoint off, as the specification suggests.
//!
//! # What it refuses
//!
//! * An endpoint URL that is not `http` or `https`.
//! * An endpoint whose host is a loopback, private or link-local address,
//!   or `localhost`, unless `allow_private` is on. A webhook URL is chosen
//!   by whoever registers it, and without this check it is a way to make the
//!   server call its own network. The URL is checked when the endpoint is
//!   added and again before each attempt.
//! * A name that resolves to such an address, unless `allow_private` is on.
//!   The client resolves through [`PublicOnly`], which drops every address
//!   on this server's network at connect time. A check before the request
//!   would not do: the name can resolve to another address a moment later.
//! * A redirect: a 3xx answer is a failed attempt, not a hop to follow.

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::{ContextBuilder, ContextItem, ProvidesContext};
use brocanth::plugin_shutdown::ShutdownCx;
use brocanth_db::sqlx::{self, Row};
use brocanth_db::Db;
use brocanth_events::{ChangeKind, ChangeSink, RowChange};
use brocanth_jobs::{Backoff, Job, JobContext, Queue, Worker, WorkerHandle};
use serde::{Deserialize, Serialize};

use crate::postgres::run_schema;
use crate::sign::{decode_secret, new_message_id, new_secret, sign_standard};

/// The queue the delivery jobs wait on.
pub const QUEUE: &str = "webhooks";

/// The first [`Outgoing`] built in this process, for the job and the panel.
static INSTALLED: OnceLock<Outgoing> = OnceLock::new();

/// What failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutError(pub String);

impl std::fmt::Display for OutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for OutError {}

fn db_error(e: sqlx::Error) -> OutError {
    OutError(format!("webhooks: {e}"))
}

/// How the outgoing side runs.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "config", derive(serde::Deserialize))]
#[cfg_attr(feature = "config", serde(default, deny_unknown_fields))]
pub struct OutgoingConfig {
    /// Create the tables on the first readiness check.
    pub migrate: bool,
    /// Run a worker for the delivery jobs in this process.
    pub worker: bool,
    /// How many deliveries run at once.
    pub concurrency: usize,
    /// Seconds an attempt may take before it fails.
    pub timeout_secs: u64,
    /// Accept an endpoint on a loopback or private address. For
    /// development, where the receiver is on the same machine.
    pub allow_private: bool,
}

impl Default for OutgoingConfig {
    fn default() -> Self {
        Self {
            migrate: true,
            worker: true,
            concurrency: 4,
            timeout_secs: 10,
            allow_private: false,
        }
    }
}

/// One endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    /// Its row id.
    pub id: i64,
    /// Where deliveries go.
    pub url: String,
    /// The secret it verifies with, `whsec_...`. Show it once, to the
    /// person who registered the endpoint.
    pub secret: String,
    /// The events it wants: `*`, `posts.*`, or `posts.created`.
    pub events: Vec<String>,
    /// Whether deliveries go to it.
    pub active: bool,
    /// What it is for.
    pub description: String,
}

/// Where one delivery is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeliveryRow {
    /// Its row id.
    pub id: i64,
    /// The endpoint it goes to.
    pub endpoint_id: i64,
    /// The endpoint's URL.
    pub url: String,
    /// The `webhook-id`, the same on every attempt.
    pub message_id: String,
    /// The event type.
    pub event: String,
    /// The body that is sent.
    pub payload: String,
    /// `pending`, `retrying`, `delivered` or `failed`.
    pub state: String,
    /// Attempts made.
    pub attempts: i32,
    /// The status of the last answer, when there was one.
    pub last_status: Option<i32>,
    /// What went wrong last, when something did.
    pub last_error: Option<String>,
    /// When it was written, as Unix seconds.
    pub created_at: i64,
}

/// The outgoing side: a handle on the tables, the queue and the client.
///
/// On the per-request context, so a page emits an event:
///
/// ```ignore
/// let hooks = ctx.provided::<brocanth_webhooks::Outgoing, _>().expect("dispatched");
/// hooks.emit("invoice.paid", &serde_json::json!({ "id": 42 })).await?;
/// ```
#[derive(Clone)]
pub struct Outgoing {
    inner: Arc<Inner>,
}

struct Inner {
    db: Db,
    queue: Queue,
    client: reqwest::Client,
    allow_private: bool,
}

impl ContextItem for Outgoing {}

impl std::fmt::Debug for Outgoing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Outgoing").finish_non_exhaustive()
    }
}

impl Outgoing {
    /// The two tables and their index, idempotent.
    pub const SCHEMA: &'static str = "\
CREATE TABLE IF NOT EXISTS brocanth_webhook_endpoints (
    id          bigserial   PRIMARY KEY,
    url         text        NOT NULL,
    secret      text        NOT NULL,
    events      text[]      NOT NULL DEFAULT '{*}',
    active      boolean     NOT NULL DEFAULT true,
    description text        NOT NULL DEFAULT '',
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS brocanth_webhook_deliveries (
    id           bigserial   PRIMARY KEY,
    endpoint_id  bigint      NOT NULL REFERENCES brocanth_webhook_endpoints (id) ON DELETE CASCADE,
    message_id   text        NOT NULL,
    event        text        NOT NULL,
    payload      text        NOT NULL,
    state        text        NOT NULL DEFAULT 'pending',
    attempts     integer     NOT NULL DEFAULT 0,
    last_status  integer,
    last_error   text,
    created_at   timestamptz NOT NULL DEFAULT now(),
    delivered_at timestamptz
);
CREATE INDEX IF NOT EXISTS brocanth_webhook_deliveries_created
    ON brocanth_webhook_deliveries (created_at DESC)";

    /// The outgoing side over `db`: a queue on its pool and a client.
    ///
    /// Creates nothing: see [`Outgoing::migrate`]. Call it inside a Tokio
    /// runtime, because the queue's pool is the `db` module's.
    ///
    /// # Errors
    /// The client cannot be built.
    pub fn new(db: Db, config: &OutgoingConfig) -> Result<Self, OutError> {
        let client = client(config)?;
        let queue = Queue::from_pool(db.pool().clone());
        Ok(Self {
            inner: Arc::new(Inner {
                db,
                queue,
                client,
                allow_private: config.allow_private,
            }),
        })
    }

    /// The first `Outgoing` installed in this process: what the job and
    /// the admin panel use.
    pub fn installed() -> Option<Outgoing> {
        INSTALLED.get().cloned()
    }

    /// Makes this the one [`Outgoing::installed`] answers. The first call
    /// wins; the plugin makes it.
    pub fn install(&self) {
        let _ = INSTALLED.set(self.clone());
    }

    /// The queue the delivery jobs go on.
    pub fn queue(&self) -> &Queue {
        &self.inner.queue
    }

    /// Creates the tables, and the queue's.
    ///
    /// # Errors
    /// The database refused a statement.
    pub async fn migrate(&self) -> Result<(), OutError> {
        run_schema(&self.inner.db, Self::SCHEMA)
            .await
            .map_err(db_error)?;
        self.inner
            .queue
            .migrate()
            .await
            .map_err(|e| OutError(format!("webhooks: creating the queue tables: {e}")))
    }

    /// Registers an endpoint with a new secret.
    ///
    /// # Errors
    /// The URL is refused (see the module header), an event pattern is
    /// empty, or the database failed.
    pub async fn add_endpoint(
        &self,
        url: &str,
        events: &[&str],
        description: &str,
    ) -> Result<Endpoint, OutError> {
        check_url(url, self.inner.allow_private)?;
        let events: Vec<String> = if events.is_empty() {
            vec!["*".to_string()]
        } else {
            events.iter().map(|e| e.trim().to_string()).collect()
        };
        if events.iter().any(String::is_empty) {
            return Err(OutError(
                "webhooks: an event pattern is empty\nhelp: use `*`, `posts.*` or `posts.created`"
                    .to_string(),
            ));
        }
        let secret = new_secret();
        let row = sqlx::query(
            "INSERT INTO brocanth_webhook_endpoints (url, secret, events, description) \
             VALUES ($1, $2, $3, $4) RETURNING id",
        )
        .bind(url)
        .bind(&secret)
        .bind(&events)
        .bind(description)
        .fetch_one(self.inner.db.pool())
        .await
        .map_err(db_error)?;
        Ok(Endpoint {
            id: row.get("id"),
            url: url.to_string(),
            secret,
            events,
            active: true,
            description: description.to_string(),
        })
    }

    /// Every endpoint, oldest first.
    ///
    /// # Errors
    /// The database failed.
    pub async fn endpoints(&self) -> Result<Vec<Endpoint>, OutError> {
        let rows = sqlx::query(
            "SELECT id, url, secret, events, active, description \
             FROM brocanth_webhook_endpoints ORDER BY id",
        )
        .fetch_all(self.inner.db.pool())
        .await
        .map_err(db_error)?;
        Ok(rows
            .iter()
            .map(|row| Endpoint {
                id: row.get("id"),
                url: row.get("url"),
                secret: row.get("secret"),
                events: row.get("events"),
                active: row.get("active"),
                description: row.get("description"),
            })
            .collect())
    }

    /// Turns an endpoint on or off. `false` when there is no such endpoint.
    ///
    /// # Errors
    /// The database failed.
    pub async fn set_active(&self, id: i64, active: bool) -> Result<bool, OutError> {
        let done = sqlx::query("UPDATE brocanth_webhook_endpoints SET active = $2 WHERE id = $1")
            .bind(id)
            .bind(active)
            .execute(self.inner.db.pool())
            .await
            .map_err(db_error)?;
        Ok(done.rows_affected() == 1)
    }

    /// Removes an endpoint and its deliveries.
    ///
    /// # Errors
    /// The database failed.
    pub async fn remove_endpoint(&self, id: i64) -> Result<bool, OutError> {
        let done = sqlx::query("DELETE FROM brocanth_webhook_endpoints WHERE id = $1")
            .bind(id)
            .execute(self.inner.db.pool())
            .await
            .map_err(db_error)?;
        Ok(done.rows_affected() == 1)
    }

    /// Sends `event` to every active endpoint that wants it: one delivery
    /// row and one job each. Answers the delivery ids.
    ///
    /// The rows are written before the jobs are enqueued. A process that
    /// dies between the two leaves a `pending` row with no job, which the
    /// panel's "Send again" button, or [`Outgoing::redeliver`], sends.
    ///
    /// # Errors
    /// The event name is empty, or the database or the queue failed.
    pub async fn emit(&self, event: &str, data: &serde_json::Value) -> Result<Vec<i64>, OutError> {
        if event.trim().is_empty() {
            return Err(OutError(
                "webhooks: an event needs a type\nhelp: name it `<thing>.<what happened>`, as in `invoice.paid`"
                    .to_string(),
            ));
        }
        let wanting: Vec<i64> = self
            .endpoints()
            .await?
            .into_iter()
            .filter(|e| e.active && wants(&e.events, event))
            .map(|e| e.id)
            .collect();
        let body = serde_json::json!({
            "type": event,
            "timestamp": unix_now(),
            "data": data,
        })
        .to_string();
        let mut ids = Vec::with_capacity(wanting.len());
        for endpoint in wanting {
            let row = sqlx::query(
                "INSERT INTO brocanth_webhook_deliveries (endpoint_id, message_id, event, payload) \
                 VALUES ($1, $2, $3, $4) RETURNING id",
            )
            .bind(endpoint)
            .bind(new_message_id())
            .bind(event)
            .bind(&body)
            .fetch_one(self.inner.db.pool())
            .await
            .map_err(db_error)?;
            let id: i64 = row.get("id");
            self.enqueue(id).await?;
            ids.push(id);
        }
        Ok(ids)
    }

    async fn enqueue(&self, delivery: i64) -> Result<(), OutError> {
        self.inner
            .queue
            .enqueue(Deliver { delivery })
            .await
            .map(drop)
            .map_err(|e| OutError(format!("webhooks: enqueuing delivery {delivery}: {e}")))
    }

    /// Makes one attempt at one delivery, and records it on the row.
    ///
    /// `Ok` when there is nothing more to do: delivered, already delivered,
    /// gone, or answered `410`. `Err` asks the job to try again; on the
    /// `last` attempt the row is marked `failed` first.
    ///
    /// # Errors
    /// The attempt failed and should be retried.
    pub async fn attempt(&self, delivery: i64, last: bool) -> Result<(), String> {
        let row = sqlx::query(
            "SELECT d.message_id, d.payload, d.state, e.id AS endpoint, e.url, e.secret, e.active \
             FROM brocanth_webhook_deliveries d \
             JOIN brocanth_webhook_endpoints e ON e.id = d.endpoint_id \
             WHERE d.id = $1",
        )
        .bind(delivery)
        .fetch_optional(self.inner.db.pool())
        .await
        .map_err(|e| e.to_string())?;
        let Some(row) = row else {
            // The endpoint was removed, and its deliveries with it.
            return Ok(());
        };
        let state: String = row.get("state");
        if state == "delivered" {
            return Ok(());
        }
        if !row.get::<bool, _>("active") {
            self.record(delivery, "failed", None, Some("the endpoint is off"))
                .await?;
            return Ok(());
        }
        let message_id: String = row.get("message_id");
        let payload: String = row.get("payload");
        let url: String = row.get("url");
        // Again here: the row may predate `allow_private = false`, or come
        // from SQL that never passed `add_endpoint`.
        if let Err(refused) = check_url(&url, self.inner.allow_private) {
            self.record(delivery, "failed", None, Some(&refused.0))
                .await?;
            return Ok(());
        }
        let secret: String = row.get("secret");
        let key = decode_secret(&secret).map_err(|e| e.to_string())?;
        let now = unix_now();
        let sent = self
            .inner
            .client
            .post(&url)
            .header("content-type", "application/json")
            .header("webhook-id", &message_id)
            .header("webhook-timestamp", now.to_string())
            .header(
                "webhook-signature",
                sign_standard(&key, &message_id, now, payload.as_bytes()),
            )
            .body(payload)
            .send()
            .await;
        let (status, error) = match sent {
            Ok(answer) => {
                let status = answer.status().as_u16();
                if answer.status().is_success() {
                    (status, None)
                } else {
                    let text = answer.text().await.unwrap_or_default();
                    (status, Some(format!("answered {status}: {}", clip(&text))))
                }
            }
            Err(e) => (0, Some(format!("could not reach the endpoint: {e}"))),
        };
        let code = (status != 0).then_some(i32::from(status));
        match error {
            None => {
                self.record(delivery, "delivered", code, None).await?;
                Ok(())
            }
            Some(why) if status == 410 => {
                let endpoint: i64 = row.get("endpoint");
                let _ = self.set_active(endpoint, false).await;
                self.record(
                    delivery,
                    "failed",
                    code,
                    Some(&format!("{why}; the endpoint is now off")),
                )
                .await?;
                Ok(())
            }
            Some(why) => {
                let state = if last { "failed" } else { "retrying" };
                self.record(delivery, state, code, Some(&why)).await?;
                Err(why)
            }
        }
    }

    async fn record(
        &self,
        delivery: i64,
        state: &str,
        status: Option<i32>,
        error: Option<&str>,
    ) -> Result<(), String> {
        sqlx::query(
            "UPDATE brocanth_webhook_deliveries SET state = $2, attempts = attempts + 1, \
             last_status = $3, last_error = $4, \
             delivered_at = CASE WHEN $2 = 'delivered' THEN now() ELSE delivered_at END \
             WHERE id = $1",
        )
        .bind(delivery)
        .bind(state)
        .bind(status)
        .bind(error)
        .execute(self.inner.db.pool())
        .await
        .map(drop)
        .map_err(|e| e.to_string())
    }

    /// The newest deliveries first.
    ///
    /// # Errors
    /// The database failed.
    pub async fn deliveries(&self, limit: i64, offset: i64) -> Result<Vec<DeliveryRow>, OutError> {
        let rows = sqlx::query(
            "SELECT d.id, d.endpoint_id, e.url, d.message_id, d.event, d.payload, d.state, \
             d.attempts, d.last_status, d.last_error, \
             extract(epoch FROM d.created_at)::bigint AS created \
             FROM brocanth_webhook_deliveries d \
             JOIN brocanth_webhook_endpoints e ON e.id = d.endpoint_id \
             ORDER BY d.id DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(self.inner.db.pool())
        .await
        .map_err(db_error)?;
        Ok(rows
            .iter()
            .map(|row| DeliveryRow {
                id: row.get("id"),
                endpoint_id: row.get("endpoint_id"),
                url: row.get("url"),
                message_id: row.get("message_id"),
                event: row.get("event"),
                payload: row.get("payload"),
                state: row.get("state"),
                attempts: row.get("attempts"),
                last_status: row.get("last_status"),
                last_error: row.get("last_error"),
                created_at: row.get("created"),
            })
            .collect())
    }

    /// Sends a delivery again, with a fresh set of attempts. `false` when
    /// there is no such delivery or it was delivered.
    ///
    /// # Errors
    /// The database or the queue failed.
    pub async fn redeliver(&self, delivery: i64) -> Result<bool, OutError> {
        let done = sqlx::query(
            "UPDATE brocanth_webhook_deliveries SET state = 'pending' \
             WHERE id = $1 AND state <> 'delivered'",
        )
        .bind(delivery)
        .execute(self.inner.db.pool())
        .await
        .map_err(db_error)?;
        if done.rows_affected() == 0 {
            return Ok(false);
        }
        self.enqueue(delivery).await?;
        Ok(true)
    }

    /// A change sink that emits `<collection>.created`, `.updated` and
    /// `.deleted` for the listed collections, or for every one when the list
    /// is empty. Add it to the sink the model layer writes through.
    ///
    /// The emit runs on a task: a sink must not block the write that caused
    /// it, so a change whose emit fails is lost. An event that must not be
    /// lost is emitted by the code that wrote the row, with
    /// [`Outgoing::emit`].
    ///
    /// # Errors
    /// No Tokio runtime is running.
    pub fn sink(&self, collections: &[&str]) -> Result<WebhookSink, OutError> {
        let runtime = tokio::runtime::Handle::try_current().map_err(|e| {
            OutError(format!(
                "webhooks: the change sink needs a Tokio runtime ({e})\nhelp: build it in a plugin stage or a handler"
            ))
        })?;
        Ok(WebhookSink {
            outgoing: self.clone(),
            collections: collections.iter().map(|c| c.to_string()).collect(),
            runtime,
        })
    }
}

/// Emits an event per row change. See [`Outgoing::sink`].
pub struct WebhookSink {
    outgoing: Outgoing,
    collections: Vec<String>,
    runtime: tokio::runtime::Handle,
}

impl WebhookSink {
    /// The event type and the payload one change becomes.
    pub fn event_of(change: &RowChange<'_>) -> (String, serde_json::Value) {
        let (kind, fields): (&str, Vec<&str>) = match &change.kind {
            ChangeKind::Created => ("created", Vec::new()),
            ChangeKind::Updated { fields } => ("updated", fields.iter().collect()),
            ChangeKind::Deleted => ("deleted", Vec::new()),
        };
        (
            format!("{}.{kind}", change.collection),
            serde_json::json!({
                "collection": change.collection,
                "id": change.row.to_string(),
                "fields": fields,
            }),
        )
    }
}

impl ChangeSink for WebhookSink {
    fn publish(&self, change: &RowChange<'_>) {
        let (event, data) = Self::event_of(change);
        let outgoing = self.outgoing.clone();
        self.runtime.spawn(async move {
            let _ = outgoing.emit(&event, &data).await;
        });
    }

    fn wants(&self, collection: &str) -> bool {
        self.collections.is_empty() || self.collections.iter().any(|c| c == collection)
    }
}

/// The job: one attempt at one delivery.
///
/// Eight attempts, from thirty seconds apart to six hours apart, with
/// jitter: about a day in all, then the job's dead letter and a `failed`
/// row. The waits are 30 s, 4 min, 32 min, 4 h 16 min, then 6 h three
/// times: 23 hours. The factor is 8, because doubling from 30 s reaches
/// only 32 minutes by the last attempt, an hour in all.
#[derive(Serialize, Deserialize)]
pub struct Deliver {
    /// The delivery row.
    pub delivery: i64,
}

impl Job for Deliver {
    const NAME: &'static str = "brocanth_webhooks.deliver";
    const QUEUE: &'static str = QUEUE;
    const MAX_ATTEMPTS: u32 = 8;
    const BACKOFF: Backoff =
        Backoff::new(Duration::from_secs(30), Duration::from_secs(6 * 3600)).factor(8);
    type Error = String;

    fn unique_key(&self) -> Option<String> {
        Some(format!("webhook-delivery:{}", self.delivery))
    }

    async fn perform(&self, cx: &JobContext) -> Result<(), Self::Error> {
        let outgoing = Outgoing::installed().ok_or_else(|| {
            "no `Outgoing` is installed in this process\nhelp: register the webhooks_out plugin in the process that runs this worker".to_string()
        })?;
        outgoing.attempt(self.delivery, cx.is_last_attempt()).await
    }
}

/// Whether an endpoint's patterns take this event.
pub fn wants(patterns: &[String], event: &str) -> bool {
    patterns.iter().any(|p| match p.strip_suffix('*') {
        Some("") => true,
        Some(prefix) => event.starts_with(prefix),
        None => p == event,
    })
}

/// Refuses a URL an endpoint must not have. See the module header.
///
/// # Errors
/// The refusal, with a `help:` line.
pub fn check_url(url: &str, allow_private: bool) -> Result<(), OutError> {
    let parsed = reqwest::Url::parse(url).map_err(|e| {
        OutError(format!(
            "webhooks: `{url}` is not a URL ({e})\nhelp: write it in full, as in `https://example.com/hooks`"
        ))
    })?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(OutError(format!(
            "webhooks: `{url}` is not http or https\nhelp: an endpoint is an HTTP URL"
        )));
    }
    if allow_private {
        return Ok(());
    }
    let private = match parsed.host() {
        Some(url::Host::Ipv4(ip)) => is_private(IpAddr::V4(ip)),
        Some(url::Host::Ipv6(ip)) => is_private(IpAddr::V6(ip)),
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost" || name.ends_with(".localhost")
        }
        None => true,
    };
    if private {
        return Err(OutError(format!(
            "webhooks: `{url}` points into this server's own network\n\
             help: use a public address, or set `allow_private = true` in development"
        )));
    }
    Ok(())
}

/// The HTTP client every attempt uses: no redirect, a timeout, and
/// [`PublicOnly`] as its resolver unless `allow_private` is on.
fn client(config: &OutgoingConfig) -> Result<reqwest::Client, OutError> {
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(config.timeout_secs.max(1)))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("brocanth-webhooks/", env!("CARGO_PKG_VERSION")));
    if !config.allow_private {
        builder = builder.dns_resolver(PublicOnly);
    }
    builder
        .build()
        .map_err(|e| OutError(format!("webhooks: building the HTTP client: {e}")))
}

/// A resolver that keeps only public addresses.
///
/// It runs when the client connects, so a name that resolves to a public
/// address when the endpoint is added and to `127.0.0.1` when it is called
/// still reaches nothing here.
#[derive(Clone, Copy, Debug)]
pub struct PublicOnly;

impl reqwest::dns::Resolve for PublicOnly {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let found = tokio::net::lookup_host((host.as_str(), 0)).await?;
            let public = public_only(&host, found)?;
            Ok(Box::new(public.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// The addresses of `found` that are not on this server's network.
///
/// # Errors
/// None is left, with a `help:` line.
pub fn public_only(
    host: &str,
    found: impl IntoIterator<Item = SocketAddr>,
) -> Result<Vec<SocketAddr>, OutError> {
    let public: Vec<SocketAddr> = found
        .into_iter()
        .filter(|addr| !is_private(addr.ip()))
        .collect();
    if public.is_empty() {
        return Err(OutError(format!(
            "webhooks: `{host}` resolves to no public address
             help: use a public address, or set `allow_private = true` in development"
        )));
    }
    Ok(public)
}

fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                // 0.0.0.0/8: "this network", and 0.0.0.0 reaches this host.
                || a == 0
                || v4.is_broadcast()
                || v4.is_multicast()
                // 100.64.0.0/10: carrier-grade NAT.
                || a == 100 && (b & 0xc0) == 64
        }
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // fc00::/7, unique local.
                || (first & 0xfe00) == 0xfc00
                // fe80::/10, link-local, and fec0::/10, the old site-local.
                || (first & 0xffc0) == 0xfe80
                || (first & 0xffc0) == 0xfec0
                // An IPv4 address inside: `::ffff:a.b.c.d`, `::a.b.c.d`,
                // and the NAT64 prefix 64:ff9b::/96.
                || v6.to_ipv4().is_some_and(|v4| is_private(IpAddr::V4(v4)))
                || (v6.segments()[..6] == [0x64, 0xff9b, 0, 0, 0, 0]
                    && is_private(IpAddr::V4(std::net::Ipv4Addr::from_bits(
                        (v6.to_bits() & 0xffff_ffff) as u32,
                    ))))
        }
    }
}

fn clip(text: &str) -> String {
    let mut end = text.len().min(300);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// The plugin: builds [`Outgoing`] over the `db` module's pool, creates the
/// tables, runs the delivery worker and puts `Outgoing` on the context.
///
/// Register it after the `db` plugin, which publishes the pool it reads.
pub struct OutgoingPlugin {
    config: OutgoingConfig,
    outgoing: Option<Outgoing>,
    worker: Option<WorkerHandle>,
}

/// The time the worker keeps to give back what did not finish.
const GIVE_BACK: Duration = Duration::from_secs(2);

impl Plugin for OutgoingPlugin {
    type Config = OutgoingConfig;
    type Provides = ProvidesContext<Outgoing>;
    /// Also the configuration section.
    const NAME: &'static str = "webhooks_out";

    fn new(config: OutgoingConfig) -> Self {
        Self {
            config,
            outgoing: None,
            worker: None,
        }
    }

    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        if self.config.worker && self.config.concurrency == 0 {
            return Err(PluginError::new(
                "[webhooks_out] concurrency is 0, so the worker would deliver nothing\n\
                 help: set it to at least 1, or set worker = false",
            ));
        }
        Ok(())
    }

    fn before_start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        let Some(db) = lc.services().get::<Db>().cloned() else {
            return Err(PluginError::new(
                "[webhooks_out] no database: the deliveries are rows in the `db` module's database\n\
                 help: register the db plugin before this one",
            ));
        };
        let runtime = tokio::runtime::Handle::try_current().map_err(|e| {
            PluginError::new(format!(
                "[webhooks_out] needs a Tokio runtime at startup ({e})\n\
                 help: build the application inside the runtime that serves it"
            ))
        })?;
        let outgoing = {
            let _entered = runtime.enter();
            Outgoing::new(db, &self.config)
                .map_err(|e| PluginError::new(format!("[webhooks_out] {e}")))?
        };
        outgoing.install();
        if self.config.migrate {
            let probe = outgoing.clone();
            let migrated = Arc::new(tokio::sync::OnceCell::new());
            lc.ready_when(move || {
                let probe = probe.clone();
                let migrated = Arc::clone(&migrated);
                async move {
                    migrated
                        .get_or_try_init(|| probe.migrate())
                        .await
                        .map(drop)
                        .map_err(|e| {
                            format!(
                                "creating the webhook tables: {e}\n\
                                 help: check `[db] url`; with `migrate = false`, run \
                                 `brocanth_webhooks::Outgoing::SCHEMA` in a migration"
                            )
                        })
                }
            });
        }
        self.outgoing = Some(outgoing);
        Ok(())
    }

    fn start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        let (Some(outgoing), true) = (&self.outgoing, self.config.worker) else {
            return Ok(());
        };
        let worker = Worker::builder(outgoing.queue())
            .register::<Deliver>()
            .queues([QUEUE])
            .concurrency(self.config.concurrency)
            .start()
            .map_err(|e| PluginError::new(format!("[webhooks_out] starting the worker: {e}")))?;
        worker.stop_when(
            lc.readiness().draining(),
            Duration::from_secs(10).saturating_sub(GIVE_BACK),
        );
        self.worker = Some(worker);
        Ok(())
    }

    fn begin_shutdown(&mut self, sd: &mut ShutdownCx<'_>) -> Result<(), PluginError> {
        if let Some(worker) = &self.worker {
            let grace = sd.remaining().unwrap_or(Duration::from_secs(10));
            worker.stop(grace.saturating_sub(GIVE_BACK));
        }
        Ok(())
    }

    fn provide(&self, cx: &mut ContextBuilder) {
        if let Some(outgoing) = &self.outgoing {
            cx.insert(outgoing.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pattern_takes_its_events() {
        let p = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(wants(&p(&["*"]), "posts.created"));
        assert!(wants(&p(&["posts.*"]), "posts.deleted"));
        assert!(!wants(&p(&["posts.*"]), "postsx.deleted"));
        assert!(wants(&p(&["invoice.paid"]), "invoice.paid"));
        assert!(!wants(&p(&["invoice.paid"]), "invoice.void"));
        assert!(!wants(&p(&[]), "anything"));
    }

    #[test]
    fn an_endpoint_on_this_network_is_refused_unless_allowed() {
        for url in [
            "http://127.0.0.1:9000/hook",
            "http://localhost/hook",
            "http://10.1.2.3/",
            "http://192.168.0.1/",
            "http://169.254.169.254/latest/meta-data",
            "http://[::1]/",
            "http://[fd00::1]/",
            "http://100.64.0.1/",
        ] {
            let err = check_url(url, false).expect_err(url);
            assert!(err.0.contains("help:"), "{err}");
            assert!(check_url(url, true).is_ok(), "{url}");
        }
        assert!(check_url("https://example.com/hooks", false).is_ok());
        assert!(check_url("ftp://example.com/", true).is_err());
        assert!(check_url("not a url", true).is_err());
    }

    #[test]
    fn every_spelling_of_a_private_address_is_refused() {
        for url in [
            // IPv4 written as one number, in octal, in hex, and short.
            "http://2130706433/",
            "http://0177.0.0.1/",
            "http://0x7f.0.0.1/",
            "http://127.1/",
            "http://0.0.0.0/",
            "http://0.1.2.3/",
            "http://255.255.255.255/",
            "http://224.0.0.1/",
            "http://172.16.5.4/",
            "http://172.31.255.255/",
            "http://100.127.255.254/",
            // IPv6: mapped, compatible, NAT64, link-local, unique local.
            "http://[::ffff:127.0.0.1]/",
            "http://[::ffff:7f00:1]/",
            "http://[::ffff:10.0.0.1]/",
            "http://[::ffff:169.254.169.254]/",
            "http://[::127.0.0.1]/",
            "http://[64:ff9b::7f00:1]/",
            "http://[64:ff9b::a9fe:a9fe]/",
            "http://[fe80::1]/",
            "http://[fe80::1%25en0]/",
            "http://[fec0::1]/",
            "http://[fc00::1]/",
            "http://[::]/",
            "http://[ff02::1]/",
            // Names.
            "http://LOCALHOST/",
            "http://localhost./",
            "http://api.localhost/",
            // The host is after the `@`.
            "http://example.com@127.0.0.1/",
            "http://user:pw@[::1]:8080/",
        ] {
            assert!(check_url(url, false).is_err(), "{url} was accepted");
        }
        for url in [
            "http://8.8.8.8/",
            "http://172.32.0.1/",
            "http://100.128.0.1/",
            "http://[2606:4700:4700::1111]/",
            "http://[::ffff:8.8.8.8]/",
            "http://[64:ff9b::808:808]/",
            "http://127.0.0.1@example.com/",
        ] {
            assert!(check_url(url, false).is_ok(), "{url} was refused");
        }
    }

    #[test]
    fn a_name_that_resolves_only_to_private_addresses_is_refused() {
        let addr = |text: &str| text.parse::<SocketAddr>().unwrap();
        let err = public_only("evil.example", [addr("127.0.0.1:0"), addr("[::1]:0")])
            .expect_err("only private addresses");
        assert!(err.0.contains("help:"), "{err}");
        assert!(public_only("evil.example", []).is_err());
        let kept = public_only(
            "mixed.example",
            [
                addr("10.0.0.1:0"),
                addr("93.184.216.34:0"),
                addr("[fd00::1]:0"),
            ],
        )
        .unwrap();
        assert_eq!(kept, vec![addr("93.184.216.34:0")], "only the public one");
    }

    #[tokio::test]
    async fn the_resolver_refuses_localhost_through_the_system_resolver() {
        use reqwest::dns::Resolve;
        let found = PublicOnly.resolve("localhost".parse().unwrap()).await;
        let err = found.err().expect("localhost resolves only to loopback");
        assert!(err.to_string().contains("no public address"), "{err}");
    }

    /// The client of an `Outgoing` reaches a name on this machine only when
    /// `allow_private` is on: the resolver is wired into it.
    #[tokio::test]
    async fn the_client_does_not_connect_to_a_name_on_this_machine() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let accepted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = Arc::clone(&accepted);
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                use tokio::io::AsyncWriteExt;
                let _ = socket
                    .write_all(b"HTTP/1.1 204 No Content\r\nconnection: close\r\n\r\n")
                    .await;
            }
        });
        // `localhost` by name: check_url would refuse it before the client,
        // so this asks the client directly.
        let url = format!("http://localhost:{port}/hook");
        let strict = client(&OutgoingConfig::default()).unwrap();
        assert!(strict.post(&url).send().await.is_err());
        assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 0);

        let open = client(&OutgoingConfig {
            allow_private: true,
            ..OutgoingConfig::default()
        })
        .unwrap();
        let answer = open.post(&url).send().await.expect("allowed");
        assert_eq!(answer.status().as_u16(), 204);
    }

    #[test]
    fn the_retries_span_about_a_day_from_thirty_seconds_to_six_hours() {
        let backoff = Deliver::BACKOFF;
        assert_eq!(Deliver::MAX_ATTEMPTS, 8);
        let waits: Vec<u64> = (1..Deliver::MAX_ATTEMPTS)
            .map(|attempt| backoff.base_delay(attempt).as_secs())
            .collect();
        assert_eq!(waits.first(), Some(&30));
        assert_eq!(waits.last(), Some(&(6 * 3600)));
        assert!(waits.windows(2).all(|w| w[0] <= w[1]), "{waits:?}");
        let total: u64 = waits.iter().sum();
        assert!(
            (20 * 3600..=28 * 3600).contains(&total),
            "about a day, not {total}s: {waits:?}"
        );
    }

    #[test]
    fn one_delivery_is_one_job_however_often_it_is_enqueued() {
        let a = Deliver { delivery: 7 }.unique_key();
        assert_eq!(a, Deliver { delivery: 7 }.unique_key());
        assert_ne!(a, Deliver { delivery: 8 }.unique_key());
    }

    #[test]
    fn a_row_change_becomes_a_typed_event() {
        let fields = ["title"];
        let change = RowChange::updated("posts", 7_i64, fields.as_slice());
        let (event, data) = WebhookSink::event_of(&change);
        assert_eq!(event, "posts.updated");
        assert_eq!(data["id"], "7");
        assert_eq!(data["fields"][0], "title");
        let (event, _) = WebhookSink::event_of(&RowChange::deleted("posts", 7_i64));
        assert_eq!(event, "posts.deleted");
    }
}
