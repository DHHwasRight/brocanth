#![doc = include_str!("../README.md")]
//!
//! # Design notes
//!
//! Each case talks to `dyn Cache` and nothing else. It names no table, no
//! connection and no command. A backend passes the suite when it keeps the
//! promises the trait documents, and the suite checks only those promises.
//!
//! **Real time, short TTLs.** The expiry cases store a value for a fraction
//! of a second and sleep past it. A manual clock would be exact, but a
//! server-side backend (Redis) keeps its own clock, and the suite must run
//! against it too. The TTLs are short and the sleeps are twice as long, so
//! the whole suite takes a few seconds.
//!
//! **Isolation by name.** Every namespace and every tag starts with a prefix
//! unique to the run. Two runs against one live server do not meet, and no
//! case clears anything it did not write.
//!
//! **What the suite does not check.** How many rows `purge_expired`
//! removes: a backend whose store expires entries by itself (Redis) has
//! nothing to remove and answers 0. Counts that include expired entries or
//! leases: the trait does not promise them. Speed.

#![forbid(unsafe_code)]

mod cases;

use std::fmt;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use brocanth_cache::{Cache, Key, Namespace, Tag};

/// How one case ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// The case held.
    Pass,
    /// The case did not hold, with the reason.
    Fail(String),
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
    /// The name the caller gave the backend.
    pub backend: String,
    /// One entry per case, in the order they ran.
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

    /// Panic with the failures, for use from a test.
    pub fn assert_ok(&self) {
        assert!(self.failures().is_empty(), "{self}");
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "cache conformance: {} — {} passed, {} failed",
            self.backend,
            self.passed(),
            self.failures().len()
        )?;
        for o in &self.outcomes {
            match &o.status {
                Status::Pass => writeln!(f, "  pass  {}", o.name)?,
                Status::Fail(why) => writeln!(f, "  FAIL  {}\n          {why}", o.name)?,
            }
        }
        Ok(())
    }
}

/// What the suite hands each case.
pub struct Ctx {
    /// The cache under test.
    pub cache: Arc<dyn Cache>,
    /// A prefix unique to this run.
    pub prefix: String,
}

impl Ctx {
    /// A namespace no other case and no other run uses.
    ///
    /// # Panics
    /// When `label` makes the name longer than a namespace may be. Labels
    /// are the suite's own, so that is a bug in the suite.
    pub fn namespace(&self, label: &str) -> Namespace {
        Namespace::new(format!("{}.{label}", self.prefix)).expect("a short label")
    }

    /// A key in [`Ctx::namespace`]`(label)`.
    ///
    /// # Panics
    /// As [`Ctx::namespace`].
    pub fn key(&self, label: &str, key: &str) -> Key {
        self.namespace(label).key(key).expect("a short key")
    }

    /// A tag no other case and no other run uses. Tags cross namespaces,
    /// so they carry the prefix too.
    ///
    /// # Panics
    /// As [`Ctx::namespace`].
    pub fn tag(&self, label: &str) -> Tag {
        Tag::new(format!("{}#{label}", self.prefix)).expect("a short tag")
    }
}

/// Run every case against `cache`, and report.
///
/// `backend` names the backend in the report. The cache must be migrated,
/// or have nothing to migrate.
pub async fn run(backend: impl Into<String>, cache: Arc<dyn Cache>) -> Report {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let cx = Ctx {
        cache,
        prefix: format!("conf{:x}", stamp & 0xffff_ffff_ffff),
    };
    let mut outcomes = Vec::new();
    for case in cases::all() {
        let status = match (case.run)(&cx).await {
            Ok(()) => Status::Pass,
            Err(why) => Status::Fail(why),
        };
        outcomes.push(Outcome {
            name: case.name,
            status,
        });
    }
    Report {
        backend: backend.into(),
        outcomes,
    }
}

/// Run every case and panic unless all of them hold.
///
/// ```no_run
/// # async fn demo(cache: std::sync::Arc<dyn brocanth_cache::Cache>) {
/// brocanth_cache_conformance::assert_conformant("my-backend", cache).await;
/// # }
/// ```
pub async fn assert_conformant(backend: impl Into<String>, cache: Arc<dyn Cache>) {
    run(backend, cache).await.assert_ok();
}

/// The names of every case, in the order they run.
pub fn case_names() -> Vec<&'static str> {
    cases::all().iter().map(|c| c.name).collect()
}
