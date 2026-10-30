//! The worker, end to end, against the reference backend.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use brocanth_queue::{
    Backend, BackendError, Backoff, Capabilities, ClaimRequest, Claimed, Client, DynBackend, Job,
    JobContext, LeaseRef, LeaseState, NewJob, Outbox, Pushed, Registry, Relay, RelayConfig, Settle,
    UniquenessKind, Worker, WorkerConfig,
};
use brocanth_queue_memory::{Memory, MemoryOutbox};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

static RUNS: AtomicU32 = AtomicU32::new(0);
static FAILS: AtomicU32 = AtomicU32::new(0);
static SLOW_ENTERED: AtomicU32 = AtomicU32::new(0);

#[derive(Serialize, Deserialize)]
struct Works;

impl Job for Works {
    const KIND: &'static str = "t.works";
    const QUEUE: &'static str = "q";
    type Error = std::io::Error;
    async fn run(&self, _cx: &JobContext) -> Result<(), Self::Error> {
        RUNS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct Fails;

impl Job for Fails {
    const KIND: &'static str = "t.fails";
    const QUEUE: &'static str = "q";
    const MAX_ATTEMPTS: u32 = 3;
    const BACKOFF: Backoff = Backoff::Fixed(Duration::from_millis(10));
    type Error = std::io::Error;
    async fn run(&self, _cx: &JobContext) -> Result<(), Self::Error> {
        FAILS.fetch_add(1, Ordering::SeqCst);
        Err(std::io::Error::other("no"))
    }
}

#[derive(Serialize, Deserialize)]
struct Panics;

impl Job for Panics {
    const KIND: &'static str = "t.panics";
    const QUEUE: &'static str = "q";
    const MAX_ATTEMPTS: u32 = 1;
    type Error = std::io::Error;
    async fn run(&self, _cx: &JobContext) -> Result<(), Self::Error> {
        panic!("the job exploded");
    }
}

#[derive(Serialize, Deserialize)]
struct Slow;

impl Job for Slow {
    const KIND: &'static str = "t.slow";
    const QUEUE: &'static str = "q";
    const BACKOFF: Backoff = Backoff::Fixed(Duration::from_millis(10));
    type Error = std::io::Error;
    async fn run(&self, _cx: &JobContext) -> Result<(), Self::Error> {
        SLOW_ENTERED.fetch_add(1, Ordering::SeqCst);
        // Longer than the grace period, and it never checks `is_stopping`,
        // so the worker has to drop it.
        tokio::time::sleep(Duration::from_secs(30)).await;
        Ok(())
    }
}

fn config() -> WorkerConfig {
    WorkerConfig {
        name: "test".into(),
        queue: "q".into(),
        concurrency: 4,
        lease: Duration::from_secs(5),
        heartbeat: Duration::from_millis(400),
        poll_idle: Duration::from_millis(10),
        shutdown_grace: Duration::from_millis(100),
        unknown_kind_backoff: Duration::from_millis(20),
        sweep_every: Duration::from_secs(60),
        settle_retries: 2,
    }
}

async fn until<F: Fn() -> bool>(what: &str, f: F) {
    let start = std::time::Instant::now();
    while !f() {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "timed out: {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_job_runs_once_and_leaves_nothing_behind() {
    let store = Arc::new(Memory::new());
    let backend: Arc<dyn DynBackend> = store.clone();
    let client = Client::new(backend.clone());
    let mut registry = Registry::new();
    registry.register::<Works>();

    client.enqueue(Works).await.unwrap();
    let (tx, rx) = watch::channel(false);
    let w = Worker::new(backend, Arc::new(registry), config()).unwrap();
    let handle = tokio::spawn(w.run(rx));

    until("the job ran", || RUNS.load(Ordering::SeqCst) >= 1).await;
    until("the queue emptied", || store.is_empty()).await;
    tx.send(true).unwrap();
    handle.await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_job_retries_to_its_limit_then_dies() {
    let store = Arc::new(Memory::new());
    let backend: Arc<dyn DynBackend> = store.clone();
    let client = Client::new(backend.clone());
    let mut registry = Registry::new();
    registry.register::<Fails>();

    client.enqueue(Fails).await.unwrap();
    let (tx, rx) = watch::channel(false);
    let handle = tokio::spawn(
        Worker::new(backend, Arc::new(registry), config())
            .unwrap()
            .run(rx),
    );

    until("the job reached the dead letter", || {
        !store.dead_letter().is_empty()
    })
    .await;
    assert_eq!(FAILS.load(Ordering::SeqCst), 3, "it ran its three attempts");
    let dead = store.dead_letter();
    assert_eq!(dead[0].kind, "t.fails");
    assert_eq!(dead[0].attempts, 3);
    tx.send(true).unwrap();
    handle.await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_panic_is_a_failure_and_not_a_lost_worker() {
    let store = Arc::new(Memory::new());
    let backend: Arc<dyn DynBackend> = store.clone();
    let client = Client::new(backend.clone());
    let mut registry = Registry::new();
    registry.register::<Panics>();
    registry.register::<Works>();

    client.enqueue(Panics).await.unwrap();
    client.enqueue(Works).await.unwrap();
    let (tx, rx) = watch::channel(false);
    let handle = tokio::spawn(
        Worker::new(backend, Arc::new(registry), config())
            .unwrap()
            .run(rx),
    );

    until("the panicking job died", || !store.dead_letter().is_empty()).await;
    let dead = store.dead_letter();
    assert!(
        dead[0].failure.message.contains("exploded"),
        "the panic message is kept: {:?}",
        dead[0].failure
    );
    // The worker is still alive and still working the queue.
    until("the next job still ran", || store.is_empty()).await;
    tx.send(true).unwrap();
    handle.await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_old_worker_hands_back_a_job_it_does_not_know() {
    // The rolling-deploy case. The old worker has no `t.works`; it must put
    // the job back without spending an attempt, so the new worker still has
    // the whole retry budget.
    let store = Arc::new(Memory::new());
    let backend: Arc<dyn DynBackend> = store.clone();
    let client = Client::new(backend.clone());

    client.enqueue(Works).await.unwrap();

    let mut old = Registry::new();
    old.register::<Fails>(); // anything but Works
    let (tx, rx) = watch::channel(false);
    let handle = tokio::spawn(
        Worker::new(backend.clone(), Arc::new(old), config())
            .unwrap()
            .run(rx),
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    tx.send(true).unwrap();
    handle.await.unwrap();

    assert_eq!(store.len(), 1, "the job is still there");
    assert!(
        store.dead_letter().is_empty(),
        "and it was not dead-lettered"
    );

    // Now the new worker rolls out.
    let before = RUNS.load(Ordering::SeqCst);
    let mut new = Registry::new();
    new.register::<Works>();
    let (tx, rx) = watch::channel(false);
    let handle = tokio::spawn(
        Worker::new(backend, Arc::new(new), config())
            .unwrap()
            .run(rx),
    );
    until("the new worker ran it", || {
        RUNS.load(Ordering::SeqCst) > before
    })
    .await;
    until("and finished it", || store.is_empty()).await;
    tx.send(true).unwrap();
    handle.await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_shutdown_gives_a_running_job_back_instead_of_failing_it() {
    let store = Arc::new(Memory::new());
    let backend: Arc<dyn DynBackend> = store.clone();
    let client = Client::new(backend.clone());
    let mut registry = Registry::new();
    registry.register::<Slow>();

    client.enqueue(Slow).await.unwrap();
    let (tx, rx) = watch::channel(false);
    let handle = tokio::spawn(
        Worker::new(backend, Arc::new(registry), config())
            .unwrap()
            .run(rx),
    );

    until("the job started", || {
        SLOW_ENTERED.load(Ordering::SeqCst) >= 1
    })
    .await;
    tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(3), handle)
        .await
        .expect("the worker stopped inside its grace period")
        .unwrap();

    assert_eq!(store.len(), 1, "the job is still queued");
    assert!(
        store.dead_letter().is_empty(),
        "a shutdown is not a job failure"
    );
}

#[derive(Serialize, Deserialize)]
struct Busy;

impl Job for Busy {
    const KIND: &'static str = "t.busy";
    const QUEUE: &'static str = "q";
    type Error = std::io::Error;
    async fn run(&self, _cx: &JobContext) -> Result<(), Self::Error> {
        BUSY_RUNS.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(5)).await;
        Ok(())
    }
}

static BUSY_RUNS: AtomicU32 = AtomicU32::new(0);

/// The sweep used to be an arm of the claim loop that only ran when a claim
/// came back empty. A queue with a standing backlog therefore never swept:
/// scheduled work was never promoted into the queue, exhausted rows were
/// never dead-lettered, and the deduplication table never shrank. The bug
/// needed load to appear, which is why nothing caught it.
#[tokio::test(flavor = "multi_thread")]
async fn the_sweep_runs_while_the_queue_is_never_empty() {
    let store = Arc::new(Memory::new());
    let backend: Arc<dyn DynBackend> = store.clone();
    let client = Client::new(backend.clone());
    let mut registry = Registry::new();
    registry.register::<Busy>();

    // Keep the queue loaded so no claim ever returns nothing.
    for _ in 0..200 {
        client.enqueue(Busy).await.unwrap();
    }

    let (tx, rx) = watch::channel(false);
    let w = Worker::new(
        backend.clone(),
        Arc::new(registry),
        WorkerConfig {
            sweep_every: Duration::from_millis(20),
            ..config()
        },
    )
    .unwrap();
    let handle = tokio::spawn(w.run(rx));

    let feeder = {
        let client = Client::new(backend.clone());
        tokio::spawn(async move {
            for _ in 0..400 {
                let _ = client.enqueue(Busy).await;
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
    };

    until("the sweep ran repeatedly", || store.sweeps() >= 3).await;
    assert!(
        BUSY_RUNS.load(Ordering::SeqCst) > 0,
        "the queue has to be busy for this test to mean anything"
    );

    feeder.abort();
    tx.send(true).unwrap();
    handle.await.unwrap();
}

static RELAYED: AtomicU32 = AtomicU32::new(0);

#[derive(Serialize, Deserialize)]
struct Relayed;

impl Job for Relayed {
    const KIND: &'static str = "t.relayed";
    const QUEUE: &'static str = "q";
    type Error = std::io::Error;
    async fn run(&self, _cx: &JobContext) -> Result<(), Self::Error> {
        RELAYED.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// The relay is what moves a bulk job out of the caller's database after the
/// transaction that wrote it commits. It was reachable as a type and started
/// by nothing, so a process that enqueued in a transaction wrote rows no
/// reader ever came for. The worker now runs one.
#[tokio::test(flavor = "multi_thread")]
async fn a_worker_relay_delivers_what_a_transaction_left_behind() {
    let store = Arc::new(Memory::new());
    let backend: Arc<dyn DynBackend> = store.clone();
    let mut registry = Registry::new();
    registry.register::<Relayed>();

    let outbox = Arc::new(MemoryOutbox::new());
    outbox.write(NewJob {
        kind: Relayed::KIND.into(),
        queue: "q".into(),
        payload: b"null".to_vec(),
        run_at: None,
        max_attempts: 3,
        unique_key: None,
        uniqueness: UniquenessKind::None,
    });

    let relay = Relay::new(outbox.clone(), backend.clone(), RelayConfig::default());
    let (tx, rx) = watch::channel(false);
    let w = Worker::new(backend, Arc::new(registry), config())
        .unwrap()
        .relay(relay);
    let handle = tokio::spawn(w.run(rx));

    until("the relayed job ran", || {
        RELAYED.load(Ordering::SeqCst) >= 1
    })
    .await;
    let mut emptied = false;
    for _ in 0..200 {
        if outbox.pending().await.unwrap() == 0 {
            emptied = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(emptied, "the outbox did not empty");

    tx.send(true).unwrap();
    handle.await.unwrap();
}

static FOREVER_ENTERED: AtomicU32 = AtomicU32::new(0);
static FOREVER_FINISHED: AtomicU32 = AtomicU32::new(0);
static SETTLES: AtomicU32 = AtomicU32::new(0);

#[derive(Serialize, Deserialize)]
struct Forever;

impl Job for Forever {
    const KIND: &'static str = "t.forever";
    const QUEUE: &'static str = "q";
    const MAX_ATTEMPTS: u32 = 100;
    type Error = std::io::Error;
    async fn run(&self, _cx: &JobContext) -> Result<(), Self::Error> {
        FOREVER_ENTERED.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_secs(30)).await;
        FOREVER_FINISHED.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// Every call forwarded except the heartbeat, which never lands.
struct DeafHeartbeat(Arc<dyn DynBackend>);

impl Backend for DeafHeartbeat {
    fn name(&self) -> &'static str {
        "deaf"
    }
    fn capabilities(&self) -> Capabilities {
        self.0.capabilities()
    }
    async fn push(&self, b: Vec<NewJob>) -> Result<Vec<Pushed>, BackendError> {
        self.0.push(b).await
    }
    async fn claim(&self, r: &ClaimRequest) -> Result<Vec<Claimed>, BackendError> {
        self.0.claim(r).await
    }
    async fn heartbeat(&self, _l: &LeaseRef, _e: Duration) -> Result<LeaseState, BackendError> {
        Err(BackendError::Unavailable(
            "the heartbeat went nowhere".into(),
        ))
    }
    async fn settle(&self, l: &LeaseRef, o: Settle) -> Result<LeaseState, BackendError> {
        SETTLES.fetch_add(1, Ordering::SeqCst);
        self.0.settle(l, o).await
    }
    async fn sweep(&self) -> Result<(), BackendError> {
        self.0.sweep().await
    }
}

/// A failed heartbeat used to be reported to the observer and otherwise
/// ignored, so the job kept running after its lease had gone. Another worker
/// is entitled to claim that job the moment the lease expires, and the two
/// then run the same body at the same time with neither aware of the other.
/// The worker now stops at the expiry it was given and settles nothing,
/// which is what it already did for a lease it was told it had lost.
#[tokio::test(flavor = "multi_thread")]
async fn a_job_whose_lease_expires_is_abandoned_and_not_settled() {
    let store = Arc::new(Memory::new());
    let inner: Arc<dyn DynBackend> = store.clone();
    let backend: Arc<dyn DynBackend> = Arc::new(DeafHeartbeat(inner.clone()));
    let client = Client::new(inner);
    let mut registry = Registry::new();
    registry.register::<Forever>();

    client.enqueue(Forever).await.unwrap();
    let (tx, rx) = watch::channel(false);
    let w = Worker::new(
        backend,
        Arc::new(registry),
        WorkerConfig {
            lease: Duration::from_millis(600),
            heartbeat: Duration::from_millis(150),
            ..config()
        },
    )
    .unwrap();
    let handle = tokio::spawn(w.run(rx));

    // Entered twice means the first run was abandoned at the lease and the
    // row became claimable again, which is the contract.
    until("the job was redelivered after its lease", || {
        FOREVER_ENTERED.load(Ordering::SeqCst) >= 2
    })
    .await;

    assert_eq!(
        SETTLES.load(Ordering::SeqCst),
        0,
        "a worker that lost its lease must not settle: the row is someone else's now"
    );
    assert_eq!(
        FOREVER_FINISHED.load(Ordering::SeqCst),
        0,
        "the body has to be aborted, not left running past the lease"
    );

    tx.send(true).unwrap();
    handle.await.unwrap();
}
