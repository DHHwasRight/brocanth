//! The contract, as a suite any backend can be held to.
//!
//! Nothing here names Postgres, a connection, a transaction or SQL. A case
//! declares the capabilities it needs and is skipped, not failed, when the
//! backend does not declare them — so an adapter that supports less is still
//! held to everything it claims.
//!
//! Run it twice: once against the backend, once through
//! [`Adversary`], which uses every freedom the contract gives it. The two
//! results must match. They stop matching the moment a case assumes
//! something only a database gives away.
//!
//! ```no_run
//! # async fn demo(backend: std::sync::Arc<dyn brocanth_queue::DynBackend>) {
//! let report = brocanth_queue_conformance::run(backend).await;
//! println!("{report}");
//! report.assert_ok();
//! # }
//! ```

#![forbid(unsafe_code)]

mod adversary;
mod cases;

use std::fmt;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use brocanth_queue::{Capabilities, DynBackend};

pub use adversary::Adversary;

/// How one case ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// The case held.
    Pass,
    /// The case did not hold, with the reason.
    Fail(String),
    /// The backend does not declare what the case needs.
    Skip(&'static str),
}

/// One case and what it did.
#[derive(Debug, Clone)]
pub struct Outcome {
    /// The case's name.
    pub name: &'static str,
    /// What it did.
    pub status: Status,
}

/// What a run produced.
#[derive(Debug, Clone)]
pub struct Report {
    /// The backend's name.
    pub backend: &'static str,
    /// One entry per case.
    pub outcomes: Vec<Outcome>,
}

impl Report {
    /// How many cases held.
    pub fn passed(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|o| o.status == Status::Pass)
            .count()
    }

    /// Every case that did not hold.
    pub fn failures(&self) -> Vec<&Outcome> {
        self.outcomes
            .iter()
            .filter(|o| matches!(o.status, Status::Fail(_)))
            .collect()
    }

    /// Every case the backend's capabilities put out of scope.
    pub fn skipped(&self) -> Vec<&Outcome> {
        self.outcomes
            .iter()
            .filter(|o| matches!(o.status, Status::Skip(_)))
            .collect()
    }

    /// Which cases ran and what they did, without the detail. Two runs that
    /// agree here agree on the contract.
    pub fn shape(&self) -> Vec<(&'static str, &'static str)> {
        self.outcomes
            .iter()
            .map(|o| {
                (
                    o.name,
                    match o.status {
                        Status::Pass => "pass",
                        Status::Fail(_) => "fail",
                        Status::Skip(_) => "skip",
                    },
                )
            })
            .collect()
    }

    /// Panic with the failures, for use from a test.
    pub fn assert_ok(&self) {
        let failed = self.failures();
        assert!(failed.is_empty(), "{self}");
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "conformance: {} — {} passed, {} failed, {} skipped",
            self.backend,
            self.passed(),
            self.failures().len(),
            self.skipped().len()
        )?;
        for o in &self.outcomes {
            match &o.status {
                Status::Pass => writeln!(f, "  pass  {}", o.name)?,
                Status::Skip(why) => writeln!(f, "  skip  {} ({why})", o.name)?,
                Status::Fail(why) => writeln!(f, "  FAIL  {}\n          {why}", o.name)?,
            }
        }
        Ok(())
    }
}

/// What the suite hands each case.
pub struct Ctx {
    /// The backend under test.
    pub backend: Arc<dyn DynBackend>,
    /// What it declared.
    pub caps: Capabilities,
    /// A prefix unique to this run, so a suite can run twice against a live
    /// server without the second run meeting the first run's rows.
    pub prefix: String,
}

impl Ctx {
    /// A queue name no other case uses.
    pub fn queue(&self, case: &str) -> String {
        format!("{}-{case}", self.prefix)
    }
}

/// Run every case against a backend.
pub async fn run(backend: Arc<dyn DynBackend>) -> Report {
    let caps = backend.capabilities();
    let name = backend.name();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let cx = Ctx {
        backend,
        caps,
        prefix: format!("conf{stamp:x}"),
    };
    let mut outcomes = Vec::new();
    for case in cases::all() {
        let status = match (case.needs)(&cx.caps) {
            Some(why) => Status::Skip(why),
            None => match (case.run)(&cx).await {
                Ok(()) => Status::Pass,
                Err(why) => Status::Fail(why),
            },
        };
        outcomes.push(Outcome {
            name: case.name,
            status,
        });
    }
    Report {
        backend: name,
        outcomes,
    }
}

/// Run the suite twice: plain, then through [`Adversary`].
///
/// The two reports must agree case by case. A case that only passes plain is
/// a case that depends on something the contract does not promise.
pub async fn run_both(backend: Arc<dyn DynBackend>) -> (Report, Report) {
    let plain = run(Arc::clone(&backend)).await;
    let hostile = run(Adversary::new(backend)).await;
    (plain, hostile)
}

/// Run both passes and panic unless they agree and both hold.
pub async fn assert_conformant(backend: Arc<dyn DynBackend>) {
    let (plain, hostile) = run_both(backend).await;
    plain.assert_ok();
    hostile.assert_ok();
    assert_eq!(
        plain.shape(),
        hostile.shape(),
        "the suite behaved differently when the backend used its freedoms.\
         \n--- plain ---\n{plain}\n--- hostile ---\n{hostile}"
    );
}
