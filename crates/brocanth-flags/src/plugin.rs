//! [`FlagsPlugin`] and [`FlagsConfig`]: the copy on the request context, and
//! the task that keeps it fresh.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::{ContextBuilder, ProvidesContext};
use brocanth::plugin_shutdown::ShutdownCx;

use crate::flags::Flags;
use crate::store::MemoryStore;

/// The first copy built in this process, for a panel the admin mounts
/// before a plugin can hand it one. See [`FlagsPlugin::installed`].
static INSTALLED: OnceLock<Flags> = OnceLock::new();

/// The flag settings: `[flags]` in a config file.
#[derive(Clone, Debug)]
#[cfg_attr(
    feature = "config",
    derive(serde::Serialize, serde::Deserialize),
    serde(default, deny_unknown_fields)
)]
pub struct FlagsConfig {
    /// Seconds between two reads of the table. A change made in another
    /// process shows here within this time. 0 reads only at start.
    pub refresh_secs: u64,
    /// Create the table on the first readiness check.
    pub migrate: bool,
}

impl Default for FlagsConfig {
    fn default() -> Self {
        FlagsConfig {
            refresh_secs: 10,
            migrate: true,
        }
    }
}

/// Puts [`Flags`] on every request context and keeps it fresh.
///
/// The store is picked in `before_start`: a `brocanth_db::Db` in the
/// application's services (the `db` plugin, registered before this one)
/// gives a [`PgStore`](crate::PgStore); without one the rules are kept in
/// memory and lost at restart. The first readiness check creates the table
/// (with `migrate`) and reads it, so the server never answers a request
/// with every flag off because the copy was still empty.
pub struct FlagsPlugin {
    config: FlagsConfig,
    flags: Flags,
    refresher: Option<tokio::task::JoinHandle<()>>,
}

impl FlagsPlugin {
    /// The copy this plugin serves: for a test, and for
    /// [`FlagsPanel`](crate::admin::FlagsPanel).
    pub fn flags(&self) -> Flags {
        self.flags.clone()
    }

    /// The copy of the first `FlagsPlugin` built in this process.
    ///
    /// A generated admin mounts its routes before any plugin can hand it a
    /// value, so its panel asks here, when it renders. A process builds one
    /// application, so "the first" is "the one".
    pub fn installed() -> Option<Flags> {
        INSTALLED.get().cloned()
    }
}

impl Plugin for FlagsPlugin {
    type Config = FlagsConfig;
    type Provides = ProvidesContext<Flags>;
    /// Also the configuration section.
    const NAME: &'static str = "flags";

    fn new(config: FlagsConfig) -> Self {
        let flags = Flags::new();
        let _ = INSTALLED.set(flags.clone());
        FlagsPlugin {
            config,
            flags,
            refresher: None,
        }
    }

    fn before_start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        if !self.flags.has_store() {
            self.attach_store(lc);
        }
        // The first read is the readiness check's. Then a task reads again
        // every `refresh_secs`. No runtime: a test or a tool that builds the
        // application without serving it, which calls `refresh` itself.
        if self.config.refresh_secs == 0 {
            return Ok(());
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return Ok(());
        };
        let flags = self.flags.clone();
        let every = Duration::from_secs(self.config.refresh_secs);
        self.refresher = Some(runtime.spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // The first tick is now; the readiness check already read.
            tick.tick().await;
            loop {
                tick.tick().await;
                // A failed read keeps the last rules and records the error
                // for the panel. Nothing here reaches a request.
                let _ = flags.refresh().await;
            }
        }));
        Ok(())
    }

    fn shutdown(&mut self, _sd: &mut ShutdownCx<'_>) -> Result<(), PluginError> {
        if let Some(refresher) = self.refresher.take() {
            refresher.abort();
        }
        Ok(())
    }

    fn provide(&self, cx: &mut ContextBuilder) {
        cx.insert(self.flags.clone());
    }
}

impl FlagsPlugin {
    #[cfg(feature = "postgres")]
    fn attach_store(&mut self, lc: &mut Lifecycle<'_>) {
        let Some(db) = lc.services().get::<brocanth_db::Db>().cloned() else {
            self.flags.attach(Arc::new(MemoryStore::new()));
            return;
        };
        let store = Arc::new(crate::postgres::PgStore::new(db));
        self.flags.attach(store.clone());
        let migrate = self.config.migrate;
        let migrated = Arc::new(tokio::sync::OnceCell::new());
        let flags = self.flags.clone();
        // Every check reads the table again: it proves the store answers,
        // and it is one small query.
        lc.ready_when(move || {
            let store = store.clone();
            let migrated = Arc::clone(&migrated);
            let flags = flags.clone();
            async move {
                if migrate {
                    migrated
                        .get_or_try_init(|| store.migrate())
                        .await
                        .map_err(|e| {
                            format!(
                                "[flags] creating the table: {e}\n\
                                 help: check `[db] url`; with `migrate = false`, run \
                                 `brocanth_flags::PgStore::SCHEMA` in a migration"
                            )
                        })?;
                }
                flags.refresh().await.map_err(|e| {
                    format!(
                        "[flags] reading the table: {e}\n\
                         help: check `[db] url`; with `migrate = false`, the \
                         `brocanth_flags` table must exist"
                    )
                })
            }
        });
    }

    #[cfg(not(feature = "postgres"))]
    fn attach_store(&mut self, _lc: &mut Lifecycle<'_>) {
        self.flags.attach(Arc::new(MemoryStore::new()));
    }
}
