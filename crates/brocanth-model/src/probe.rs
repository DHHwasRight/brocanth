//! N+1 detection, as a value you hold rather than a global you configure.
//!
//! Attach a [`Probe`] to a connection with [`Probe::install`]. Open a
//! [`UnitOfWork`] around whatever counts as one piece of work — a request, a
//! job, a test — and the probe counts the statements issued inside it. Two or
//! more statements of the same *shape* are what an N+1 looks like from the
//! outside, so that is what it reports.
//!
//! Nothing here is global. A process can run two applications with two probes,
//! or one with none, and the probes do not see each other.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

use sea_orm::DatabaseConnection;

/// How a probe behaves.
#[derive(Clone)]
pub struct ProbeConfig {
    /// Whether to count at all. Off costs one atomic load per statement.
    pub enabled: bool,
    /// How many statements of one shape make a unit of work suspicious.
    pub repeat_threshold: usize,
    /// Called when a unit of work ends over the threshold.
    pub on_detect: Option<DetectCallback>,
}

impl fmt::Debug for ProbeConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProbeConfig")
            .field("enabled", &self.enabled)
            .field("repeat_threshold", &self.repeat_threshold)
            .field("on_detect", &self.on_detect.is_some())
            .finish()
    }
}

impl Default for ProbeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            repeat_threshold: 2,
            on_detect: None,
        }
    }
}

/// Called when a unit of work ends with a repeated statement shape.
pub type DetectCallback = Arc<dyn Fn(&Report) + Send + Sync>;

struct ProbeInner {
    enabled: AtomicBool,
    repeat_threshold: AtomicUsize,
    on_detect: Option<DetectCallback>,
    units: Mutex<Vec<Weak<UnitInner>>>,
}

/// A statement counter bound to one connection.
///
/// Cheap to clone; every clone is the same probe.
#[derive(Clone)]
pub struct Probe {
    inner: Arc<ProbeInner>,
}

impl fmt::Debug for Probe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Probe")
            .field("enabled", &self.enabled())
            .field("repeat_threshold", &self.repeat_threshold())
            .finish()
    }
}

impl Default for Probe {
    fn default() -> Self {
        Self::new(ProbeConfig::default())
    }
}

impl Probe {
    /// A probe with the given configuration.
    pub fn new(config: ProbeConfig) -> Self {
        Self {
            inner: Arc::new(ProbeInner {
                enabled: AtomicBool::new(config.enabled),
                repeat_threshold: AtomicUsize::new(config.repeat_threshold.max(2)),
                on_detect: config.on_detect,
                units: Mutex::new(Vec::new()),
            }),
        }
    }

    /// A probe that counts nothing until [`Probe::set_enabled`] turns it on.
    /// This is the shape to ship in production: a request can switch it on.
    pub fn off() -> Self {
        Self::new(ProbeConfig {
            enabled: false,
            ..Default::default()
        })
    }

    /// Route this connection's statements into the probe.
    ///
    /// Takes `&mut` because SeaORM's metric callback does. Install before the
    /// connection is shared.
    pub fn install(&self, conn: &mut DatabaseConnection) {
        let probe = self.clone();
        conn.set_metric_callback(move |info| probe.record(&info.statement.sql));
    }

    /// Whether statements are being counted.
    pub fn enabled(&self) -> bool {
        self.inner.enabled.load(Ordering::Relaxed)
    }

    /// Turn counting on or off. This is what a per-request switch calls.
    pub fn set_enabled(&self, on: bool) {
        self.inner.enabled.store(on, Ordering::Relaxed);
    }

    /// How many statements of one shape count as suspicious.
    pub fn repeat_threshold(&self) -> usize {
        self.inner.repeat_threshold.load(Ordering::Relaxed)
    }

    /// Change the repeat threshold. Values below 2 are clamped to 2.
    pub fn set_repeat_threshold(&self, n: usize) {
        self.inner
            .repeat_threshold
            .store(n.max(2), Ordering::Relaxed);
    }

    /// Start counting a piece of work. Counting stops when the guard drops.
    pub fn unit_of_work(&self, label: impl Into<String>) -> UnitOfWork {
        let inner = Arc::new(UnitInner {
            label: label.into(),
            state: Mutex::new(UnitState::default()),
        });
        if let Ok(mut units) = self.inner.units.lock() {
            units.retain(|w| w.strong_count() > 0);
            units.push(Arc::downgrade(&inner));
        }
        UnitOfWork {
            probe: self.clone(),
            inner,
            reported: false,
        }
    }

    fn record(&self, sql: &str) {
        if !self.enabled() {
            return;
        }
        let Ok(units) = self.inner.units.lock() else {
            return;
        };
        if units.is_empty() {
            return;
        }
        let shape = normalise(sql);
        for weak in units.iter() {
            if let Some(unit) = weak.upgrade() {
                if let Ok(mut state) = unit.state.lock() {
                    state.push(&shape, sql);
                }
            }
        }
    }
}

#[derive(Default)]
struct UnitState {
    total: usize,
    shapes: Vec<ShapeCount>,
}

struct ShapeCount {
    shape: String,
    count: usize,
    example: String,
}

impl UnitState {
    fn push(&mut self, shape: &str, sql: &str) {
        self.total += 1;
        if let Some(entry) = self.shapes.iter_mut().find(|e| e.shape == shape) {
            entry.count += 1;
        } else {
            self.shapes.push(ShapeCount {
                shape: shape.to_owned(),
                count: 1,
                example: sql.to_owned(),
            });
        }
    }
}

struct UnitInner {
    label: String,
    state: Mutex<UnitState>,
}

/// Counts the statements of one request, job or test.
pub struct UnitOfWork {
    probe: Probe,
    inner: Arc<UnitInner>,
    reported: bool,
}

impl fmt::Debug for UnitOfWork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnitOfWork")
            .field("label", &self.inner.label)
            .field("statements", &self.statements())
            .finish()
    }
}

impl UnitOfWork {
    /// Statements seen so far.
    pub fn statements(&self) -> usize {
        self.inner.state.lock().map(|s| s.total).unwrap_or_default()
    }

    /// What has been seen so far, without ending the unit.
    pub fn report(&self) -> Report {
        self.build_report()
    }

    /// End the unit and report.
    pub fn finish(mut self) -> Report {
        self.reported = true;
        self.build_report()
    }

    fn build_report(&self) -> Report {
        let threshold = self.probe.repeat_threshold();
        let Ok(state) = self.inner.state.lock() else {
            return Report {
                label: self.inner.label.clone(),
                statements: 0,
                suspects: Vec::new(),
            };
        };
        let mut suspects: Vec<Suspect> = state
            .shapes
            .iter()
            .filter(|s| s.count >= threshold)
            .map(|s| Suspect {
                shape: s.shape.clone(),
                count: s.count,
                example: s.example.clone(),
            })
            .collect();
        suspects.sort_by_key(|s| std::cmp::Reverse(s.count));
        Report {
            label: self.inner.label.clone(),
            statements: state.total,
            suspects,
        }
    }
}

impl Drop for UnitOfWork {
    fn drop(&mut self) {
        if self.reported {
            return;
        }
        let Some(callback) = self.probe.inner.on_detect.clone() else {
            return;
        };
        let report = self.build_report();
        if report.has_n_plus_one() {
            callback(&report);
        }
    }
}

/// One repeated statement shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suspect {
    /// The statement with its bind markers and `IN` lists collapsed.
    pub shape: String,
    /// How many times it was issued.
    pub count: usize,
    /// The first statement that had this shape.
    pub example: String,
}

/// What one unit of work issued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The unit's label.
    pub label: String,
    /// Statements issued.
    pub statements: usize,
    /// Shapes issued at least `repeat_threshold` times.
    pub suspects: Vec<Suspect>,
}

impl Report {
    /// `true` when at least one shape repeated.
    pub fn has_n_plus_one(&self) -> bool {
        !self.suspects.is_empty()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {} statement", self.label, self.statements)?;
        if self.statements != 1 {
            f.write_str("s")?;
        }
        if self.suspects.is_empty() {
            return Ok(());
        }
        f.write_str("\n  N+1 suspected:")?;
        for s in &self.suspects {
            write!(f, "\n    {}x  {}", s.count, s.shape)?;
        }
        Ok(())
    }
}

/// Collapse a statement to its shape.
///
/// Whitespace runs become one space, and a bind list — `(?, ?, ?)` or
/// `($1, $2)` — becomes `(?)`, so fetching children for three parents and for
/// thirty look like the same query, which they are.
pub fn normalise(sql: &str) -> String {
    let collapsed = sql.split_whitespace().collect::<Vec<_>>().join(" ");
    let numbered = numbered_params().replace_all(&collapsed, "?");
    bind_lists().replace_all(&numbered, "(?)").into_owned()
}

fn numbered_params() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\$\d+").expect("static pattern"))
}

fn bind_lists() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\(\s*\?(?:\s*,\s*\?)+\s*\)").expect("static pattern"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_lists_of_any_arity_share_one_shape() {
        let three = normalise(r#"SELECT * FROM "post" WHERE "user_id" IN (?, ?, ?)"#);
        let thirty = normalise(r#"SELECT  *  FROM "post"  WHERE "user_id" IN (?, ?, ?, ?, ?)"#);
        assert_eq!(three, thirty);
        assert_eq!(three, r#"SELECT * FROM "post" WHERE "user_id" IN (?)"#);
    }

    #[test]
    fn numbered_parameters_normalise_like_question_marks() {
        assert_eq!(
            normalise(r#"SELECT * FROM "p" WHERE "a" = $1 AND "b" = $2"#),
            r#"SELECT * FROM "p" WHERE "a" = ? AND "b" = ?"#
        );
    }

    #[test]
    fn repeated_shapes_are_reported_and_singletons_are_not() {
        let probe = Probe::new(ProbeConfig::default());
        let unit = probe.unit_of_work("GET /posts");
        probe.record(r#"SELECT * FROM "user""#);
        for id in 1..=4 {
            probe.record(&format!(r#"SELECT * FROM "post" WHERE "user_id" = {id}"#));
        }
        let report = unit.finish();
        assert_eq!(report.statements, 5);
        // The four per-user fetches differ only in their literal, which is not
        // normalised away here, so each is its own shape...
        assert!(!report.has_n_plus_one());

        let unit = probe.unit_of_work("GET /posts (bound)");
        probe.record(r#"SELECT * FROM "user""#);
        for _ in 0..4 {
            probe.record(r#"SELECT * FROM "post" WHERE "user_id" = ?"#);
        }
        let report = unit.finish();
        assert_eq!(report.statements, 5);
        assert!(report.has_n_plus_one());
        assert_eq!(report.suspects[0].count, 4);
    }

    #[test]
    fn a_disabled_probe_counts_nothing() {
        let probe = Probe::off();
        let unit = probe.unit_of_work("quiet");
        probe.record("SELECT 1");
        assert_eq!(unit.statements(), 0);
        probe.set_enabled(true);
        probe.record("SELECT 1");
        assert_eq!(unit.statements(), 1);
    }

    #[test]
    fn units_do_not_see_each_other_across_probes() {
        let a = Probe::default();
        let b = Probe::default();
        let ua = a.unit_of_work("a");
        let ub = b.unit_of_work("b");
        a.record("SELECT 1");
        assert_eq!(ua.statements(), 1);
        assert_eq!(ub.statements(), 0);
    }

    #[test]
    fn the_callback_fires_once_when_a_unit_is_dropped_unreported() {
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = Arc::clone(&seen);
        let probe = Probe::new(ProbeConfig {
            on_detect: Some(Arc::new(move |r: &Report| {
                sink.lock().expect("lock").push(r.label.clone());
            })),
            ..Default::default()
        });
        {
            let _unit = probe.unit_of_work("dropped");
            probe.record("SELECT ?");
            probe.record("SELECT ?");
        }
        assert_eq!(seen.lock().expect("lock").as_slice(), ["dropped"]);
    }
}
