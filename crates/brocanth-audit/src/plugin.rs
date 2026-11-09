//! [`AuditPlugin`] and [`AuditConfig`]: the trail on the request context,
//! the model hook that feeds it, and the task that writes and purges it.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::{ContextBuilder, ProvidesContext};
use brocanth::plugin_hooks::{HookRegistrar, HookSource, OnModel};
use brocanth::plugin_model::ModelHook;
use brocanth::plugin_order::Order;
use brocanth::plugin_shutdown::ShutdownCx;
use brocanth_events::RowChange;
use brocanth_model::plugin::RowWritten;

use crate::store::MemoryStore;
use crate::trail::Trail;

/// The first trail built in this process, for a panel the admin mounts
/// before a plugin can hand it one. See [`AuditPlugin::installed`].
static INSTALLED: OnceLock<Trail> = OnceLock::new();

/// Seconds between two purges: once a day.
const PURGE_EVERY: u64 = 86_400;

/// The audit settings: `[audit]` in a config file.
#[derive(Clone, Debug)]
#[cfg_attr(
    feature = "config",
    derive(serde::Serialize, serde::Deserialize),
    serde(default, deny_unknown_fields)
)]
pub struct AuditConfig {
    /// Days an entry is kept. 0 keeps every entry.
    pub retention_days: u32,
    /// Seconds between two writes of the queue.
    pub flush_secs: u64,
    /// Create the table on the first readiness check.
    pub migrate: bool,
}

impl Default for AuditConfig {
    fn default() -> Self {
        AuditConfig {
            retention_days: 365,
            flush_secs: 5,
            migrate: true,
        }
    }
}

/// Keeps the change history.
///
/// - A model hook, `audit.record`: every change the model layer publishes
///   through `brocanth_model::plugin::HookSink` becomes an entry with no
///   actor.
/// - [`Trail`] on every request context, so a page records a write with
///   its actor: `trail.by(user)` is a `ChangeSink`.
/// - A task that writes the queue every `flush_secs` and purges entries
///   older than `retention_days` once a day.
///
/// The store is picked in `before_start`: a `brocanth_db::Db` in the
/// application's services (the `db` plugin, registered before this one)
/// gives a [`PgStore`](crate::PgStore); without one the history is kept in
/// memory and lost at restart.
pub struct AuditPlugin {
    config: AuditConfig,
    trail: Trail,
    worker: Option<tokio::task::JoinHandle<()>>,
}

impl AuditPlugin {
    /// The trail this plugin feeds: for a test, and for the admin panel.
    pub fn trail(&self) -> Trail {
        self.trail.clone()
    }

    /// The trail of the first `AuditPlugin` built in this process.
    ///
    /// A generated admin mounts its routes before any plugin can hand it a
    /// value, so its panel and its journal ask here. A process builds one
    /// application, so "the first" is "the one".
    pub fn installed() -> Option<Trail> {
        INSTALLED.get().cloned()
    }
}

impl Plugin for AuditPlugin {
    type Config = AuditConfig;
    type Provides = ProvidesContext<Trail>;
    /// Also the configuration section.
    const NAME: &'static str = "audit";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: AuditConfig) -> Self {
        let trail = Trail::new(config.retention_days);
        let _ = INSTALLED.set(trail.clone());
        AuditPlugin {
            config,
            trail,
            worker: None,
        }
    }

    fn before_start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        if !self.trail.has_store() {
            self.attach_store(lc);
        }
        // No runtime: a test or a tool that builds the application without
        // serving it, which calls `flush` itself.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return Ok(());
        };
        let trail = self.trail.clone();
        let every = Duration::from_secs(self.config.flush_secs.max(1));
        let purge_each = (PURGE_EVERY / every.as_secs()).max(1);
        self.worker = Some(runtime.spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut ticks: u64 = 0;
            loop {
                tick.tick().await;
                ticks = ticks.wrapping_add(1);
                // A failure keeps the entries queued and records the error
                // for the panel. Nothing here reaches a request.
                let _ = trail.flush().await;
                // The first tick is at start, before the readiness check
                // made the table. Purge at the second, then once a day.
                if ticks == 2 || ticks.is_multiple_of(purge_each) {
                    let _ = trail.purge().await;
                }
            }
        }));
        Ok(())
    }

    fn shutdown(&mut self, sd: &mut ShutdownCx<'_>) -> Result<(), PluginError> {
        if let Some(worker) = self.worker.take() {
            worker.abort();
        }
        if self.trail.pending() == 0 {
            return Ok(());
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return Ok(());
        };
        let trail = self.trail.clone();
        let remaining = sd.remaining();
        let last = async move {
            match remaining {
                Some(left) => {
                    let _ = tokio::time::timeout(left, trail.flush()).await;
                }
                None => {
                    let _ = trail.flush().await;
                }
            }
        };
        // The same rule as the analytics plugin: on a current-thread
        // runtime a blocking wait would stall the only thread.
        if runtime.runtime_flavor() == tokio::runtime::RuntimeFlavor::CurrentThread {
            runtime.spawn(last);
            return Ok(());
        }
        std::thread::scope(|scope| {
            scope
                .spawn(|| runtime.block_on(last))
                .join()
                .map_err(|_| PluginError::new("[audit] the last flush panicked"))
        })
    }

    fn provide(&self, cx: &mut ContextBuilder) {
        cx.insert(self.trail.clone());
    }
}

impl AuditPlugin {
    #[cfg(feature = "postgres")]
    fn attach_store(&mut self, lc: &mut Lifecycle<'_>) {
        let Some(db) = lc.services().get::<brocanth_db::Db>().cloned() else {
            self.trail.attach(Arc::new(MemoryStore::new()));
            return;
        };
        let store = Arc::new(crate::postgres::PgStore::new(db));
        self.trail.attach(store.clone());
        if !self.config.migrate {
            return;
        }
        let migrated = Arc::new(tokio::sync::OnceCell::new());
        lc.ready_when(move || {
            let store = store.clone();
            let migrated = Arc::clone(&migrated);
            async move {
                migrated
                    .get_or_try_init(|| store.migrate())
                    .await
                    .map(|_| ())
                    .map_err(|e| {
                        format!(
                            "[audit] creating the table: {e}\n\
                             help: check `[db] url`; with `migrate = false`, run \
                             `brocanth_audit::PgStore::SCHEMA` in a migration"
                        )
                    })
            }
        });
    }

    #[cfg(not(feature = "postgres"))]
    fn attach_store(&mut self, _lc: &mut Lifecycle<'_>) {
        self.trail.attach(Arc::new(MemoryStore::new()));
    }
}

impl HookSource for AuditPlugin {
    type Families = OnModel;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        registrar.model::<RowWritten>("record", Order::new(), Record(self.trail.clone()));
        Ok(())
    }
}

/// The model hook: one entry per published change.
struct Record(Trail);

impl ModelHook<RowWritten> for Record {
    fn on_event(&self, change: &mut RowChange<'_>) -> Result<(), PluginError> {
        self.0.change(change, None);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brocanth::plugin_app::AppBuilder;
    use brocanth_events::ChangeSink;
    use brocanth_model::plugin::HookSink;

    #[test]
    fn a_change_published_through_the_hooks_reaches_the_trail() {
        let plugin = AuditPlugin::new(AuditConfig::default());
        let trail = plugin.trail();
        let app = AppBuilder::new()
            .plugin_hooking(plugin)
            .build()
            .expect("build");
        let sink = HookSink::new(app.model_dispatch());
        assert!(sink.wants("post"));
        sink.publish(&RowChange::deleted("post", 3_i64));
        assert_eq!(trail.pending(), 1);
    }
}
