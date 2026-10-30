//! `/up` and `/readyz`.
//!
//! Liveness asks nothing of anybody: a process that answers is alive.
//! Readiness runs every check the plugins registered through
//! [`Lifecycle::ready_when`](brocanth::plugin_api::Lifecycle::ready_when),
//! all at once, each under the same timeout, and reports each one with its
//! latency. Once shutdown began it answers 503 without running any.

use std::time::{Duration, Instant};

use brocanth::plugin_health::Readiness;
use brocanth_server::Response;

use crate::text::{json_f64, json_string, millis};

/// The body `/up` answers with.
pub(crate) fn live() -> Response {
    no_store(Response::json(200, r#"{"status":"ok"}"#))
}

/// One check's result.
#[derive(Clone, Debug, PartialEq)]
pub struct CheckReport {
    /// The name it registered under: the plugin instance, for a plugin.
    pub name: String,
    /// `Ok(())`, or why not.
    pub outcome: Result<(), String>,
    /// How long it took, or the timeout when it did not finish.
    pub latency: Duration,
}

/// What `/readyz` found.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadyReport {
    /// Shutdown began. No check ran.
    pub draining: bool,
    /// Every check, in registration order.
    pub checks: Vec<CheckReport>,
}

impl ReadyReport {
    /// Whether the process should get traffic: not draining, and every check
    /// passed.
    pub fn is_ready(&self) -> bool {
        !self.draining && self.checks.iter().all(|c| c.outcome.is_ok())
    }

    /// The probe body: `{"status":"ready","checks":[{"name":..,"status":"ok",
    /// "latency_ms":..}]}`. `status` is `ready`, `unready` or `draining`; a
    /// failed check adds `error`.
    pub fn to_json(&self) -> String {
        let mut out = String::from("{\"status\":");
        let status = if self.draining {
            "draining"
        } else if self.is_ready() {
            "ready"
        } else {
            "unready"
        };
        json_string(&mut out, status);
        out.push_str(",\"checks\":[");
        for (i, check) in self.checks.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str("{\"name\":");
            json_string(&mut out, &check.name);
            out.push_str(",\"status\":");
            json_string(&mut out, if check.outcome.is_ok() { "ok" } else { "fail" });
            out.push_str(",\"latency_ms\":");
            json_f64(&mut out, millis(check.latency));
            if let Err(error) = &check.outcome {
                out.push_str(",\"error\":");
                json_string(&mut out, error);
            }
            out.push('}');
        }
        out.push_str("]}");
        out
    }

    pub(crate) fn into_response(self) -> Response {
        let status = if self.is_ready() { 200 } else { 503 };
        no_store(Response::json(status, self.to_json()))
    }
}

/// Runs every check concurrently, each under `timeout`.
///
/// A check runs as its own task, so one that blocks its thread delays only
/// itself, and one that panics is a failed check rather than a failed probe.
pub(crate) async fn ready(readiness: &Readiness, timeout: Duration) -> ReadyReport {
    if readiness.is_draining() {
        return ReadyReport {
            draining: true,
            checks: Vec::new(),
        };
    }

    let running: Vec<_> = readiness
        .checks()
        .into_iter()
        .map(|check| {
            let name = check.name().to_string();
            let probe = check.probe();
            let task = tokio::spawn(async move {
                let started = Instant::now();
                let outcome = match tokio::time::timeout(timeout, probe.check()).await {
                    Ok(outcome) => outcome,
                    Err(_) => Err(format!("timed out after {} ms", timeout.as_millis())),
                };
                (outcome, started.elapsed())
            });
            (name, task)
        })
        .collect();

    let mut checks = Vec::with_capacity(running.len());
    for (name, task) in running {
        let (outcome, latency) = match task.await {
            Ok(done) => done,
            Err(_) => (Err("the check panicked".to_string()), Duration::ZERO),
        };
        checks.push(CheckReport {
            name,
            outcome,
            latency,
        });
    }

    // A check may have finished after shutdown began. The bit wins.
    ReadyReport {
        draining: readiness.is_draining(),
        checks,
    }
}

fn no_store(response: Response) -> Response {
    response.with_header("cache-control", "no-store")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_body_lists_each_check_with_its_latency_and_error() {
        let report = ReadyReport {
            draining: false,
            checks: vec![
                CheckReport {
                    name: "jobs".into(),
                    outcome: Ok(()),
                    latency: Duration::from_micros(1500),
                },
                CheckReport {
                    name: "cache".into(),
                    outcome: Err("connection \"refused\"".into()),
                    latency: Duration::from_millis(2),
                },
            ],
        };
        assert!(!report.is_ready());
        assert_eq!(
            report.to_json(),
            r#"{"status":"unready","checks":[{"name":"jobs","status":"ok","latency_ms":1.5},{"name":"cache","status":"fail","latency_ms":2,"error":"connection \"refused\""}]}"#
        );
    }
}
