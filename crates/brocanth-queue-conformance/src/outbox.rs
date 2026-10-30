//! The outbox's contract, as a suite any outbox store can be held to.
//!
//! The store is under test here, not the backend. The suite brings its own
//! sink, a backend that can be told to accept, reject, fail or be down, so a
//! case can put one bad row in a batch and watch what the relay does with
//! it.
//!
//! The suite cannot write a row by itself: the write side runs in the
//! caller's transaction, and only the caller knows how to open one. So the
//! caller passes a `write` function. The store must start empty, and every
//! case leaves it empty.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use brocanth_queue::{
    Backend, BackendError, Capabilities, ClaimRequest, Claimed, JobId, LeaseRef, LeaseState,
    NewJob, Outbox, OutboxInspect, Page, Pushed, Relay, RelayConfig, Settle, Tick, UniquenessKind,
};

use crate::{Outcome, Report, Status};

/// The payload the sink refuses.
const POISON: &[u8] = b"\"poison\"";

const PATIENCE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Take everything.
    Accept,
    /// Refuse a whole batch that holds the poison, as permanent.
    Reject,
    /// Fail a whole batch that holds the poison, as a reachable backend
    /// that errors.
    Fail,
    /// Unreachable.
    Down,
}

/// A backend that does what a case tells it to.
struct Sink {
    mode: Mutex<Mode>,
    got: Mutex<Vec<NewJob>>,
}

impl Sink {
    fn new(mode: Mode) -> Arc<Sink> {
        Arc::new(Sink {
            mode: Mutex::new(mode),
            got: Mutex::new(Vec::new()),
        })
    }

    fn set(&self, mode: Mode) {
        *self.mode.lock().unwrap() = mode;
    }

    fn got(&self) -> Vec<NewJob> {
        self.got.lock().unwrap().clone()
    }
}

impl Backend for Sink {
    fn name(&self) -> &'static str {
        "outbox-sink"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::unbounded()
    }

    async fn push(&self, batch: Vec<NewJob>) -> Result<Vec<Pushed>, BackendError> {
        let mode = *self.mode.lock().unwrap();
        let poisoned = batch.iter().any(|j| j.payload == POISON);
        match mode {
            Mode::Down => return Err(BackendError::Unavailable("sink is down".into())),
            Mode::Reject if poisoned => {
                return Err(BackendError::Rejected("payload is over the limit".into()))
            }
            Mode::Fail if poisoned => {
                return Err(BackendError::Other("the sink cannot take this one".into()))
            }
            _ => {}
        }
        let mut got = self.got.lock().unwrap();
        let start = got.len();
        let n = batch.len();
        got.extend(batch);
        Ok((start..start + n)
            .map(|i| Pushed::Enqueued(JobId(i.to_string())))
            .collect())
    }

    async fn claim(&self, _r: &ClaimRequest) -> Result<Vec<Claimed>, BackendError> {
        Ok(Vec::new())
    }

    async fn heartbeat(&self, _l: &LeaseRef, _e: Duration) -> Result<LeaseState, BackendError> {
        Ok(LeaseState::Lost)
    }

    async fn settle(&self, _l: &LeaseRef, _o: Settle) -> Result<LeaseState, BackendError> {
        Ok(LeaseState::Lost)
    }
}

fn job(n: u32, poison: bool) -> NewJob {
    NewJob {
        kind: "conformance.outbox".into(),
        queue: "outbox-conformance".into(),
        payload: if poison {
            POISON.to_vec()
        } else {
            format!("{{\"n\":{n}}}").into_bytes()
        },
        run_at: None,
        max_attempts: 5,
        unique_key: None,
        uniqueness: UniquenessKind::None,
    }
}

/// Short waits, so a case runs in milliseconds and not in minutes.
fn config(max_attempts: u32) -> RelayConfig {
    RelayConfig {
        lease: Duration::from_secs(5),
        idle: Duration::from_millis(5),
        retry_base: Duration::from_millis(5),
        retry_max: Duration::from_millis(20),
        max_attempts,
        ..RelayConfig::default()
    }
}

fn fail<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Tick until `done` says so, sleeping out the backoff between ticks.
async fn tick_until<O: Outbox>(
    relay: &mut Relay<O>,
    what: &str,
    done: impl Fn(&Tick) -> bool,
) -> Result<Tick, String> {
    let start = Instant::now();
    let mut total = Tick::default();
    loop {
        let t = relay.tick().await.map_err(fail)?;
        total.delivered += t.delivered;
        total.duplicates += t.duplicates;
        total.deferred += t.deferred;
        total.dead += t.dead;
        if done(&total) {
            return Ok(total);
        }
        if start.elapsed() > PATIENCE {
            return Err(format!("{what} did not happen in {PATIENCE:?}: {total:?}"));
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn expect_empty<O: Outbox + OutboxInspect>(store: &O) -> Result<(), String> {
    let pending = store.pending().await.map_err(fail)?;
    let dead = store.dead(Page::first(10)).await.map_err(fail)?;
    if pending != 0 || !dead.is_empty() {
        return Err(format!(
            "the store is not empty: {pending} pending, {} dead",
            dead.len()
        ));
    }
    Ok(())
}

async fn forget_all<O: OutboxInspect>(store: &O) -> Result<(), String> {
    for d in store
        .dead(Page::first(Page::MAX_LIMIT))
        .await
        .map_err(fail)?
    {
        store.forget(&d.id).await.map_err(fail)?;
    }
    Ok(())
}

/// One poisoned row is moved aside and the rest of its batch goes through.
async fn poison_does_not_block_its_batch<O, W, F>(store: &Arc<O>, write: &W) -> Result<(), String>
where
    O: Outbox + OutboxInspect,
    W: Fn(Vec<NewJob>) -> F,
    F: Future<Output = Result<(), String>>,
{
    write((0..5).map(|n| job(n, n == 2)).collect()).await?;
    let sink = Sink::new(Mode::Reject);
    let mut relay = Relay::new(Arc::clone(store), sink.clone(), config(5));

    let t = relay.tick().await.map_err(fail)?;
    if t.delivered != 4 || t.dead != 1 {
        return Err(format!(
            "one tick should deliver 4 rows and bury 1, it did {t:?}"
        ));
    }
    if sink.got().iter().any(|j| j.payload == POISON) {
        return Err("the sink took the row it rejected".into());
    }
    let pending = store.pending().await.map_err(fail)?;
    if pending != 0 {
        return Err(format!("{pending} rows still pending after the tick"));
    }
    let dead = store.dead(Page::first(10)).await.map_err(fail)?;
    match dead.as_slice() {
        [d] if d.payload == POISON && d.failure.contains("over the limit") => {}
        _ => return Err(format!("the dead letter should hold the poison: {dead:?}")),
    }
    // A rejection is final, so it does not wait for the attempt limit.
    if dead[0].attempts != 1 {
        return Err(format!(
            "a rejected row should die on its first attempt, not its {}",
            dead[0].attempts
        ));
    }
    if !store.forget(&dead[0].id).await.map_err(fail)? {
        return Err("forget did not find the dead row".into());
    }
    if store.forget(&dead[0].id).await.map_err(fail)? {
        return Err("forget found the dead row twice".into());
    }
    expect_empty(store.as_ref()).await
}

/// A row that keeps failing gets its full budget, then goes to the dead
/// letter, and its neighbours do not wait for it.
async fn failing_row_reaches_the_dead_letter_at_the_cap<O, W, F>(
    store: &Arc<O>,
    write: &W,
) -> Result<(), String>
where
    O: Outbox + OutboxInspect,
    W: Fn(Vec<NewJob>) -> F,
    F: Future<Output = Result<(), String>>,
{
    write(vec![job(0, false), job(1, true), job(2, false)]).await?;
    let sink = Sink::new(Mode::Fail);
    let mut relay = Relay::new(Arc::clone(store), sink.clone(), config(3));

    let first = relay.tick().await.map_err(fail)?;
    if first.delivered != 2 || first.deferred != 1 || first.dead != 0 {
        return Err(format!(
            "the first tick should deliver 2 and defer 1, it did {first:?}"
        ));
    }
    if !store.dead(Page::first(10)).await.map_err(fail)?.is_empty() {
        return Err("the row died before it used its attempts".into());
    }

    tick_until(
        &mut relay,
        "the failing row reaching the dead letter",
        |t| t.dead == 1,
    )
    .await?;

    let dead = store.dead(Page::first(10)).await.map_err(fail)?;
    match dead.as_slice() {
        [d] if d.payload == POISON && d.attempts == 3 => {
            if !d.failure.contains("cannot take this one") {
                return Err(format!("the dead row lost its reason: {:?}", d.failure));
            }
        }
        _ => {
            return Err(format!(
                "the dead letter should hold the poison after 3 attempts: {dead:?}"
            ))
        }
    }
    if sink.got().len() != 2 {
        return Err(format!("the sink took {} rows, not 2", sink.got().len()));
    }
    let pending = store.pending().await.map_err(fail)?;
    if pending != 0 {
        return Err(format!("{pending} rows still pending"));
    }
    forget_all(store.as_ref()).await?;
    expect_empty(store.as_ref()).await
}

/// An unreachable backend is not the row's fault and spends no attempt.
async fn an_outage_spends_no_attempt<O, W, F>(store: &Arc<O>, write: &W) -> Result<(), String>
where
    O: Outbox + OutboxInspect,
    W: Fn(Vec<NewJob>) -> F,
    F: Future<Output = Result<(), String>>,
{
    write(vec![job(0, false), job(1, false)]).await?;
    let sink = Sink::new(Mode::Down);
    let mut relay = Relay::new(Arc::clone(store), sink.clone(), config(2));

    // Far more outages than the budget allows failures.
    tick_until(&mut relay, "six outages", |t| t.deferred >= 12).await?;
    if !store.dead(Page::first(10)).await.map_err(fail)?.is_empty() {
        return Err("an outage moved a row to the dead letter".into());
    }
    if store.pending().await.map_err(fail)? != 2 {
        return Err("an outage lost a row".into());
    }

    sink.set(Mode::Accept);
    let t = tick_until(&mut relay, "delivery after the outage", |t| {
        t.delivered == 2
    })
    .await?;
    if t.dead != 0 {
        return Err(format!("a row died after the outage: {t:?}"));
    }
    expect_empty(store.as_ref()).await
}

/// A row the relay fetches and never settles is buried after the budget.
async fn an_unsettled_row_is_not_fetched_for_ever<O, W, F>(
    store: &Arc<O>,
    write: &W,
) -> Result<(), String>
where
    O: Outbox + OutboxInspect,
    W: Fn(Vec<NewJob>) -> F,
    F: Future<Output = Result<(), String>>,
{
    write(vec![job(0, false)]).await?;
    // Three relays that each take the row and die before they settle it.
    let lease = Duration::from_millis(20);
    let start = Instant::now();
    let mut fetched = 0;
    while fetched < 3 {
        if start.elapsed() > PATIENCE {
            return Err(format!("fetched the row {fetched} times, not 3"));
        }
        fetched += store.fetch(10, lease).await.map_err(fail)?.len();
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let sink = Sink::new(Mode::Accept);
    let mut relay = Relay::new(Arc::clone(store), sink.clone(), config(2));
    tick_until(
        &mut relay,
        "the unsettled row reaching the dead letter",
        |t| t.dead == 1,
    )
    .await?;
    if !sink.got().is_empty() {
        return Err("the relay pushed a row that was past its budget".into());
    }
    forget_all(store.as_ref()).await?;
    expect_empty(store.as_ref()).await
}

/// A dead row goes back to the outbox once, and is then delivered.
async fn a_dead_row_can_be_requeued<O, W, F>(store: &Arc<O>, write: &W) -> Result<(), String>
where
    O: Outbox + OutboxInspect,
    W: Fn(Vec<NewJob>) -> F,
    F: Future<Output = Result<(), String>>,
{
    write(vec![job(0, true)]).await?;
    let sink = Sink::new(Mode::Reject);
    let mut relay = Relay::new(Arc::clone(store), sink.clone(), config(5));
    let t = relay.tick().await.map_err(fail)?;
    if t.dead != 1 {
        return Err(format!("the poison should die at once, the tick did {t:?}"));
    }
    let dead = store.dead(Page::first(10)).await.map_err(fail)?;
    let [d] = dead.as_slice() else {
        return Err(format!("expected one dead row, found {}", dead.len()));
    };
    match store.dead_one(&d.id).await.map_err(fail)? {
        Some(one) if one == *d => {}
        other => return Err(format!("dead_one disagrees with dead: {other:?}")),
    }
    if d.max_attempts != Some(5) || d.job_id.is_some() {
        return Err(format!("the dead row does not describe the job: {d:?}"));
    }

    if store.requeue(&d.id).await.map_err(fail)?.is_none() {
        return Err("requeue found nothing".into());
    }
    if store.requeue(&d.id).await.map_err(fail)?.is_some() {
        return Err("a second requeue of one dead row wrote a second row".into());
    }
    if store.dead_one(&d.id).await.map_err(fail)?.is_some() {
        return Err("requeue left the dead row behind".into());
    }
    if store.pending().await.map_err(fail)? != 1 {
        return Err("requeue did not put the row back in the outbox".into());
    }

    // The fix shipped: the backend now takes it.
    sink.set(Mode::Accept);
    tick_until(&mut relay, "delivery of the requeued row", |t| {
        t.delivered == 1
    })
    .await?;
    match sink.got().as_slice() {
        [j] if j.payload == POISON && j.kind == "conformance.outbox" => {}
        got => return Err(format!("the sink did not get the requeued job: {got:?}")),
    }
    expect_empty(store.as_ref()).await
}

/// Run every outbox case against a store.
///
/// `write` must commit its rows before it returns, as the caller's
/// transaction would. The store must hold nothing when the run starts.
pub async fn run_outbox<O, W, F>(name: &'static str, store: Arc<O>, write: W) -> Report
where
    O: Outbox + OutboxInspect,
    W: Fn(Vec<NewJob>) -> F,
    F: Future<Output = Result<(), String>>,
{
    let mut outcomes = Vec::new();
    macro_rules! case {
        ($name:literal, $f:ident) => {
            // A case that fails can leave rows behind, and the next case
            // would read them as its own. Stop there instead.
            let status = match expect_empty(store.as_ref()).await {
                Err(why) => Status::Fail(format!("before the case: {why}")),
                Ok(()) => match $f(&store, &write).await {
                    Ok(()) => Status::Pass,
                    Err(why) => Status::Fail(why),
                },
            };
            let stop = matches!(status, Status::Fail(_));
            outcomes.push(Outcome {
                name: $name,
                status,
            });
            if stop {
                return Report {
                    backend: name,
                    outcomes,
                };
            }
        };
    }
    case!(
        "one poisoned row does not block its batch",
        poison_does_not_block_its_batch
    );
    case!(
        "a row that keeps failing reaches the dead letter at the cap",
        failing_row_reaches_the_dead_letter_at_the_cap
    );
    case!("an outage spends no attempt", an_outage_spends_no_attempt);
    case!(
        "a row that is never settled is not fetched for ever",
        an_unsettled_row_is_not_fetched_for_ever
    );
    case!("a dead row can be requeued", a_dead_row_can_be_requeued);
    Report {
        backend: name,
        outcomes,
    }
}

/// Run every outbox case and panic unless all of them hold.
pub async fn assert_outbox_conformant<O, W, F>(name: &'static str, store: Arc<O>, write: W)
where
    O: Outbox + OutboxInspect,
    W: Fn(Vec<NewJob>) -> F,
    F: Future<Output = Result<(), String>>,
{
    let report = run_outbox(name, store, write).await;
    report.assert_ok();
    assert_eq!(report.outcomes.len(), 5, "{report}");
}
