//! Readiness flips before any plugin closes its gate (ADR 2).

use std::sync::{Arc, Mutex};

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_context::NoContext;
use brocanth::plugin_health::Readiness;
use brocanth::plugin_shutdown::{ShutdownCx, ShutdownReason};

struct Pool {
    seen: Arc<Mutex<Vec<String>>>,
    readiness: Option<Readiness>,
}

impl Plugin for Pool {
    type Config = Arc<Mutex<Vec<String>>>;
    type Provides = NoContext;
    const NAME: &'static str = "pool";

    fn new(seen: Self::Config) -> Self {
        Self {
            seen,
            readiness: None,
        }
    }

    fn register(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        lc.ready_when(|| async { Ok(()) });
        self.readiness = Some(lc.readiness().clone());
        Ok(())
    }

    fn begin_shutdown(&mut self, _sd: &mut ShutdownCx<'_>) -> Result<(), PluginError> {
        let draining = self.readiness.as_ref().is_some_and(Readiness::is_draining);
        self.seen
            .lock()
            .unwrap()
            .push(format!("begin_shutdown draining={draining}"));
        Ok(())
    }
}

#[test]
fn a_plugin_check_is_named_after_its_instance() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let app = AppBuilder::new()
        .plugin(Pool::new(seen))
        .setup(|setup| {
            setup.readiness().add("upstream", || async { Ok(()) });
            Ok(())
        })
        .build()
        .expect("builds");
    let names: Vec<String> = app
        .readiness()
        .checks()
        .iter()
        .map(|c| c.name().to_string())
        .collect();
    assert_eq!(names, ["pool", "upstream"]);
    assert!(!app.readiness().is_draining());
}

#[test]
fn shutdown_reports_not_ready_before_begin_shutdown_runs() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut app = AppBuilder::new()
        .plugin(Pool::new(Arc::clone(&seen)))
        .build()
        .expect("builds");
    let probe = app.readiness().clone();

    let report = app.shutdown(ShutdownReason::Signal, None);
    assert!(report.is_clean());
    assert!(probe.is_draining());
    assert_eq!(*seen.lock().unwrap(), ["begin_shutdown draining=true"]);
}
