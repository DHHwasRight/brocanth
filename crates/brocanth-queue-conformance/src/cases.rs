//! The cases. Each one states a clause of the contract and nothing else.

use std::time::{Duration, Instant, SystemTime};

use brocanth_queue::{
    BackendError, BoxFuture, Capabilities, ClaimRequest, Claimed, JobFailure, LeaseState, NewJob,
    Pushed, ReleaseReason, Settle, Uniqueness, UniquenessKind, UniquenessSupport,
};

use crate::Ctx;

pub(crate) struct Case {
    pub name: &'static str,
    pub needs: fn(&Capabilities) -> Option<&'static str>,
    pub run: for<'a> fn(&'a Ctx) -> BoxFuture<'a, Result<(), String>>,
}

fn always(_: &Capabilities) -> Option<&'static str> {
    None
}

macro_rules! case {
    ($name:literal, $needs:expr, $f:ident) => {
        Case {
            name: $name,
            needs: $needs,
            run: |cx| Box::pin($f(cx)),
        }
    };
}

pub(crate) fn all() -> Vec<Case> {
    vec![
        case!(
            "a pushed job can be claimed and finished",
            always,
            round_trip
        ),
        case!("done removes the job", always, done_removes),
        case!("a claim spends an attempt", always, claim_spends_attempt),
        case!(
            "a release does not spend an attempt",
            always,
            release_keeps_attempt
        ),
        case!("a retry becomes claimable again", always, retry_returns),
        case!("an expired lease is claimable again", always, lease_expires),
        case!(
            "a stale token cannot settle a job",
            always,
            stale_token_is_fenced
        ),
        case!(
            "a stale token cannot extend a lease",
            always,
            stale_token_cannot_heartbeat
        ),
        case!("a heartbeat holds the lease", always, heartbeat_holds),
        case!("two claimers never hold one job", always, exclusive),
        case!("claim returns no more than asked", always, respects_max),
        case!("dead takes the job out of the queue", always, dead_removes),
        case!(
            "settling twice with one token is safe",
            always,
            settle_is_idempotent
        ),
        case!(
            "an unsupported call answers, and does not panic",
            always,
            unsupported_answers
        ),
        case!("a delayed job waits", needs_delay, delay_waits),
        case!(
            "a delay past the ceiling is refused",
            needs_delay_ceiling,
            delay_ceiling
        ),
        case!(
            "a payload over the limit is refused, not cut",
            needs_payload_limit,
            payload_limit
        ),
        case!("a kind filter is honoured", needs_kind_filter, kind_filter),
        case!(
            "a unique key stops a second copy",
            needs_uniqueness,
            unique_key
        ),
        case!("purge empties a queue", always, purge_empties),
        case!(
            "cancel removes a job that has not started",
            needs_cancel,
            cancel_pending
        ),
    ]
}

fn needs_delay(c: &Capabilities) -> Option<&'static str> {
    match c.max_delay {
        Some(d) if d.is_zero() => Some("the backend cannot delay a job"),
        _ => None,
    }
}

fn needs_delay_ceiling(c: &Capabilities) -> Option<&'static str> {
    c.max_delay
        .is_none()
        .then_some("the backend has no delay ceiling")
}

fn needs_payload_limit(c: &Capabilities) -> Option<&'static str> {
    c.max_payload_bytes
        .is_none()
        .then_some("the backend has no payload limit")
}

fn needs_kind_filter(c: &Capabilities) -> Option<&'static str> {
    (!c.filter_by_kind).then_some("the backend cannot filter by kind")
}

fn needs_uniqueness(c: &Capabilities) -> Option<&'static str> {
    (c.uniqueness == UniquenessSupport::None).then_some("the backend does not deduplicate")
}

fn needs_cancel(c: &Capabilities) -> Option<&'static str> {
    (!c.cancel_pending).then_some("the backend cannot cancel a pending job")
}

// ---------------------------------------------------------------- helpers

const PATIENCE: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(20);

fn job(queue: &str, kind: &str) -> NewJob {
    NewJob {
        kind: kind.into(),
        queue: queue.into(),
        payload: b"{\"n\":1}".to_vec(),
        run_at: None,
        max_attempts: 5,
        unique_key: None,
        uniqueness: UniquenessKind::None,
    }
}

fn req(queue: &str, max: usize, lease: Duration) -> ClaimRequest {
    ClaimRequest {
        queue: queue.into(),
        max,
        lease,
        kinds: None,
        worker: "conformance".into(),
    }
}

async fn push(cx: &Ctx, jobs: Vec<NewJob>) -> Result<Vec<Pushed>, String> {
    cx.backend.push(jobs).await.map_err(|e| e.to_string())
}

/// Claim until `want` jobs are in hand or patience runs out.
///
/// Every case goes through this. A backend is free to return nothing on any
/// one call, so a case that claims once is a case that only works against a
/// database.
async fn claim_until(cx: &Ctx, r: &ClaimRequest, want: usize) -> Result<Vec<Claimed>, String> {
    let start = Instant::now();
    let mut have = Vec::new();
    while have.len() < want {
        if start.elapsed() > PATIENCE {
            return Err(format!(
                "claimed {} of {want} jobs from {:?} within {PATIENCE:?}",
                have.len(),
                r.queue
            ));
        }
        let mut got = cx.backend.claim(r).await.map_err(|e| e.to_string())?;
        if got.len() > r.max {
            return Err(format!(
                "claim returned {} for a max of {}",
                got.len(),
                r.max
            ));
        }
        if got.is_empty() {
            tokio::time::sleep(POLL).await;
            continue;
        }
        have.append(&mut got);
    }
    Ok(have)
}

/// Claim nothing, and prove it stays nothing for a while.
async fn claim_none_for(cx: &Ctx, r: &ClaimRequest, window: Duration) -> Result<(), String> {
    let start = Instant::now();
    while start.elapsed() < window {
        let got = cx.backend.claim(r).await.map_err(|e| e.to_string())?;
        if !got.is_empty() {
            return Err(format!(
                "claimed {} job(s) from {:?} that should not have been claimable",
                got.len(),
                r.queue
            ));
        }
        tokio::time::sleep(POLL).await;
    }
    Ok(())
}

async fn finish(cx: &Ctx, jobs: Vec<Claimed>) -> Result<(), String> {
    for c in jobs {
        cx.backend
            .settle(&c.lease, Settle::Done)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn fail(msg: &str) -> JobFailure {
    JobFailure::returned(msg)
}

// ------------------------------------------------------------------ cases

async fn round_trip(cx: &Ctx) -> Result<(), String> {
    let q = cx.queue("round-trip");
    push(cx, vec![job(&q, "k.one")]).await?;
    let got = claim_until(cx, &req(&q, 1, Duration::from_secs(30)), 1).await?;
    if got[0].kind != "k.one" {
        return Err(format!("claimed kind {:?}", got[0].kind));
    }
    if got[0].payload != b"{\"n\":1}" {
        return Err("the payload did not survive the round trip".into());
    }
    finish(cx, got).await
}

async fn done_removes(cx: &Ctx) -> Result<(), String> {
    let q = cx.queue("done-removes");
    push(cx, vec![job(&q, "k.one")]).await?;
    let got = claim_until(cx, &req(&q, 1, Duration::from_secs(30)), 1).await?;
    finish(cx, got).await?;
    claim_none_for(
        cx,
        &req(&q, 5, Duration::from_secs(5)),
        Duration::from_millis(200),
    )
    .await
}

async fn claim_spends_attempt(cx: &Ctx) -> Result<(), String> {
    let q = cx.queue("attempts");
    push(cx, vec![job(&q, "k.one")]).await?;
    let r = req(&q, 1, Duration::from_millis(150));
    let first = claim_until(cx, &r, 1).await?;
    if first[0].attempt != 1 {
        return Err(format!(
            "the first claim reported attempt {}",
            first[0].attempt
        ));
    }
    // Let the lease lapse rather than settling, which is what a worker that
    // died looks like.
    tokio::time::sleep(Duration::from_millis(250)).await;
    let second = claim_until(cx, &r, 1).await?;
    if second[0].attempt != 2 {
        return Err(format!(
            "a job reclaimed after its lease lapsed reported attempt {}; \
             a worker that dies must still spend an attempt, or a job that \
             kills its worker loops for ever",
            second[0].attempt
        ));
    }
    finish(cx, second).await
}

async fn release_keeps_attempt(cx: &Ctx) -> Result<(), String> {
    let q = cx.queue("release");
    push(cx, vec![job(&q, "k.unknown")]).await?;
    let r = req(&q, 1, Duration::from_secs(30));
    let first = claim_until(cx, &r, 1).await?;
    let before = first[0].attempt;
    cx.backend
        .settle(
            &first[0].lease,
            Settle::Release {
                not_before: SystemTime::now(),
                reason: ReleaseReason::UnknownKind,
            },
        )
        .await
        .map_err(|e| e.to_string())?;
    let second = claim_until(cx, &r, 1).await?;
    if second[0].attempt != before {
        return Err(format!(
            "a release moved the attempt count from {before} to {}; \
             an old worker that meets a new job kind would burn its retry \
             budget and dead-letter it before the rollout finishes",
            second[0].attempt
        ));
    }
    finish(cx, second).await
}

async fn retry_returns(cx: &Ctx) -> Result<(), String> {
    let q = cx.queue("retry");
    push(cx, vec![job(&q, "k.one")]).await?;
    let r = req(&q, 1, Duration::from_secs(30));
    let first = claim_until(cx, &r, 1).await?;
    cx.backend
        .settle(
            &first[0].lease,
            Settle::Retry {
                at: SystemTime::now() + Duration::from_millis(100),
                failure: fail("not yet"),
            },
        )
        .await
        .map_err(|e| e.to_string())?;
    let second = claim_until(cx, &r, 1).await?;
    if second[0].attempt <= first[0].attempt {
        return Err("a retry did not spend an attempt".into());
    }
    finish(cx, second).await
}

async fn lease_expires(cx: &Ctx) -> Result<(), String> {
    let q = cx.queue("expiry");
    push(cx, vec![job(&q, "k.one")]).await?;
    let r = req(&q, 1, Duration::from_millis(150));
    let first = claim_until(cx, &r, 1).await?;
    // While the lease is held nobody else may have it.
    claim_none_for(cx, &r, Duration::from_millis(80)).await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let second = claim_until(cx, &r, 1).await?;
    if second[0].lease.token == first[0].lease.token {
        return Err("a reclaim after expiry reused the token; the fence is not a fence".into());
    }
    finish(cx, second).await
}

async fn stale_token_is_fenced(cx: &Ctx) -> Result<(), String> {
    let q = cx.queue("fence");
    push(cx, vec![job(&q, "k.one")]).await?;
    let r = req(&q, 1, Duration::from_millis(150));
    let first = claim_until(cx, &r, 1).await?;
    tokio::time::sleep(Duration::from_millis(220)).await;
    let second = claim_until(cx, &r, 1).await?;

    let state = cx
        .backend
        .settle(&first[0].lease, Settle::Done)
        .await
        .map_err(|e| e.to_string())?;
    if state != LeaseState::Lost {
        return Err(
            "the first worker settled a job the second worker owns. A stalled \
             process must not delete another worker's row"
                .into(),
        );
    }
    // The second worker still has it.
    let state = cx
        .backend
        .settle(&second[0].lease, Settle::Done)
        .await
        .map_err(|e| e.to_string())?;
    if state != LeaseState::Held {
        return Err("the owner could not settle its own job".into());
    }
    Ok(())
}

async fn stale_token_cannot_heartbeat(cx: &Ctx) -> Result<(), String> {
    let q = cx.queue("fence-beat");
    push(cx, vec![job(&q, "k.one")]).await?;
    let r = req(&q, 1, Duration::from_millis(150));
    let first = claim_until(cx, &r, 1).await?;
    tokio::time::sleep(Duration::from_millis(220)).await;
    let second = claim_until(cx, &r, 1).await?;
    let state = cx
        .backend
        .heartbeat(&first[0].lease, Duration::from_secs(60))
        .await
        .map_err(|e| e.to_string())?;
    if state != LeaseState::Lost {
        return Err("a stale worker extended a lease it no longer holds".into());
    }
    finish(cx, second).await
}

async fn heartbeat_holds(cx: &Ctx) -> Result<(), String> {
    let q = cx.queue("heartbeat");
    push(cx, vec![job(&q, "k.one")]).await?;
    let r = req(&q, 1, Duration::from_millis(200));
    let got = claim_until(cx, &r, 1).await?;
    for _ in 0..4 {
        tokio::time::sleep(Duration::from_millis(80)).await;
        let state = cx
            .backend
            .heartbeat(&got[0].lease, Duration::from_millis(200))
            .await
            .map_err(|e| e.to_string())?;
        if state != LeaseState::Held {
            return Err("a heartbeat inside the lease reported the lease lost".into());
        }
    }
    claim_none_for(cx, &r, Duration::from_millis(60)).await?;
    finish(cx, got).await
}

async fn exclusive(cx: &Ctx) -> Result<(), String> {
    let q = cx.queue("exclusive");
    let jobs: Vec<NewJob> = (0..6).map(|_| job(&q, "k.one")).collect();
    push(cx, jobs).await?;
    let r = req(&q, 6, Duration::from_secs(30));
    let all = claim_until(cx, &r, 6).await?;
    let mut ids: Vec<String> = all.iter().map(|c| c.lease.id.to_string()).collect();
    ids.sort();
    let before = ids.len();
    ids.dedup();
    if ids.len() != before {
        return Err("one job was handed to two claimers at the same time".into());
    }
    claim_none_for(cx, &r, Duration::from_millis(100)).await?;
    finish(cx, all).await
}

async fn respects_max(cx: &Ctx) -> Result<(), String> {
    let q = cx.queue("max");
    let jobs: Vec<NewJob> = (0..5).map(|_| job(&q, "k.one")).collect();
    push(cx, jobs).await?;
    let r = req(&q, 2, Duration::from_secs(30));
    let mut held = Vec::new();
    let start = Instant::now();
    while held.len() < 5 {
        if start.elapsed() > PATIENCE {
            break;
        }
        let got = cx.backend.claim(&r).await.map_err(|e| e.to_string())?;
        if got.len() > 2 {
            return Err(format!("claim returned {} for a max of 2", got.len()));
        }
        if got.is_empty() {
            tokio::time::sleep(POLL).await;
        }
        held.extend(got);
    }
    finish(cx, held).await
}

async fn dead_removes(cx: &Ctx) -> Result<(), String> {
    let q = cx.queue("dead");
    push(cx, vec![job(&q, "k.one")]).await?;
    let r = req(&q, 1, Duration::from_secs(30));
    let got = claim_until(cx, &r, 1).await?;
    cx.backend
        .settle(
            &got[0].lease,
            Settle::Dead {
                failure: fail("out of attempts"),
            },
        )
        .await
        .map_err(|e| e.to_string())?;
    claim_none_for(cx, &r, Duration::from_millis(200)).await
}

async fn settle_is_idempotent(cx: &Ctx) -> Result<(), String> {
    let q = cx.queue("idempotent");
    push(cx, vec![job(&q, "k.one")]).await?;
    let r = req(&q, 1, Duration::from_secs(30));
    let got = claim_until(cx, &r, 1).await?;
    cx.backend
        .settle(&got[0].lease, Settle::Done)
        .await
        .map_err(|e| e.to_string())?;
    // A worker whose first settle timed out tries again. It must not
    // resurrect the job or produce a second one.
    cx.backend
        .settle(&got[0].lease, Settle::Done)
        .await
        .map_err(|e| format!("the second settle errored: {e}"))?;
    claim_none_for(cx, &r, Duration::from_millis(150)).await
}

async fn unsupported_answers(cx: &Ctx) -> Result<(), String> {
    // Every optional call must answer. A backend that panics on a call its
    // capabilities say it cannot do takes the worker down instead of
    // reporting a configuration mistake.
    let q = cx.queue("unsupported");
    let checks: [(&str, Result<(), BackendError>); 3] = [
        (
            "approx_depth",
            cx.backend.approx_depth(&q).await.map(|_| ()),
        ),
        (
            "cancel",
            cx.backend
                .cancel(&brocanth_queue::JobId("0".into()))
                .await
                .map(|_| ()),
        ),
        ("purge", cx.backend.purge(&q).await.map(|_| ())),
    ];
    for (name, r) in checks {
        match r {
            Ok(()) => {}
            Err(BackendError::Unsupported(_)) => {}
            Err(e) => return Err(format!("{name} failed with {e}, not Unsupported")),
        }
    }
    cx.backend.sweep().await.map_err(|e| e.to_string())
}

async fn delay_waits(cx: &Ctx) -> Result<(), String> {
    let q = cx.queue("delay");
    let mut j = job(&q, "k.one");
    j.run_at = Some(SystemTime::now() + Duration::from_millis(400));
    push(cx, vec![j]).await?;
    let r = req(&q, 1, Duration::from_secs(30));
    claim_none_for(cx, &r, Duration::from_millis(250)).await?;
    let got = claim_until(cx, &r, 1).await?;
    finish(cx, got).await
}

async fn delay_ceiling(cx: &Ctx) -> Result<(), String> {
    let q = cx.queue("delay-ceiling");
    let limit = cx.caps.max_delay.expect("gated on a ceiling");
    let mut j = job(&q, "k.one");
    j.run_at = Some(SystemTime::now() + limit + Duration::from_secs(600));
    match cx.backend.push(vec![j]).await {
        Err(BackendError::Rejected(_)) => Ok(()),
        Err(e) => Err(format!(
            "a delay past the ceiling failed with {e}, not Rejected"
        )),
        Ok(_) => Err(
            "a delay past the declared ceiling was accepted. The job would be \
             delivered at the wrong time and nothing would report it"
                .into(),
        ),
    }
}

async fn payload_limit(cx: &Ctx) -> Result<(), String> {
    let q = cx.queue("payload");
    let limit = cx.caps.max_payload_bytes.expect("gated on a limit");
    let mut j = job(&q, "k.one");
    j.payload = vec![b'x'; limit + 1];
    match cx.backend.push(vec![j]).await {
        Err(BackendError::Rejected(_)) => Ok(()),
        Err(e) => Err(format!(
            "an oversized payload failed with {e}, not Rejected"
        )),
        Ok(_) => Err("an oversized payload was accepted; it would be cut or lost later".into()),
    }
}

async fn kind_filter(cx: &Ctx) -> Result<(), String> {
    let q = cx.queue("filter");
    push(cx, vec![job(&q, "k.old"), job(&q, "k.new")]).await?;
    let mut r = req(&q, 10, Duration::from_secs(30));
    r.kinds = Some(vec!["k.new".into()]);
    let got = claim_until(cx, &r, 1).await?;
    for c in &got {
        if c.kind != "k.new" {
            return Err(format!("the filter asked for k.new and got {:?}", c.kind));
        }
    }
    finish(cx, got).await?;
    let mut all = req(&q, 10, Duration::from_secs(30));
    all.kinds = None;
    let rest = claim_until(cx, &all, 1).await?;
    finish(cx, rest).await
}

async fn unique_key(cx: &Ctx) -> Result<(), String> {
    let q = cx.queue("unique");
    let kind = match cx.caps.uniqueness {
        UniquenessSupport::Unbounded => Uniqueness::UntilSuccess,
        UniquenessSupport::Window(w) => Uniqueness::Window(w.min(Duration::from_secs(60))),
        UniquenessSupport::None => return Ok(()),
    };
    let make = || {
        let mut j = job(&q, "k.one");
        j.unique_key = Some(format!("{}-key", cx.prefix));
        j.uniqueness = kind.into();
        j
    };
    let first = push(cx, vec![make()]).await?;
    if !matches!(first[0], Pushed::Enqueued(_)) {
        return Err("the first copy was refused".into());
    }
    let second = push(cx, vec![make()]).await?;
    if second[0] != Pushed::Duplicate {
        return Err(
            "a second copy with a live unique key was enqueued. A duplicate must \
             report Duplicate, not an error and not a second job"
                .into(),
        );
    }
    let got = claim_until(cx, &req(&q, 5, Duration::from_secs(30)), 1).await?;
    finish(cx, got).await
}

async fn purge_empties(cx: &Ctx) -> Result<(), String> {
    let q = cx.queue("purge");
    push(cx, vec![job(&q, "k.one"), job(&q, "k.one")]).await?;
    match cx.backend.purge(&q).await {
        Ok(_) => {}
        Err(BackendError::Unsupported(_)) => return Ok(()),
        Err(e) => return Err(e.to_string()),
    }
    claim_none_for(
        cx,
        &req(&q, 5, Duration::from_secs(5)),
        Duration::from_millis(150),
    )
    .await
}

async fn cancel_pending(cx: &Ctx) -> Result<(), String> {
    let q = cx.queue("cancel");
    let pushed = push(cx, vec![job(&q, "k.one")]).await?;
    let Pushed::Enqueued(id) = &pushed[0] else {
        return Err("the job was reported as a duplicate".into());
    };
    let removed = cx.backend.cancel(id).await.map_err(|e| e.to_string())?;
    if !removed {
        return Err("cancel reported that it removed nothing".into());
    }
    claim_none_for(
        cx,
        &req(&q, 5, Duration::from_secs(5)),
        Duration::from_millis(150),
    )
    .await
}
