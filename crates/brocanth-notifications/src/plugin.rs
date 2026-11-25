//! [`NotificationsPlugin`]: the store on every request context, and the
//! table created at the first readiness check.

use std::sync::Arc;

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::{ContextBuilder, ContextItem, ProvidesContext};

use crate::store::NotificationStore;

impl ContextItem for NotificationStore {}

/// The settings: `[notifications]` in a config file.
#[derive(Clone, Debug)]
#[cfg_attr(
    feature = "config",
    derive(serde::Serialize, serde::Deserialize),
    serde(default, deny_unknown_fields)
)]
pub struct NotificationsConfig {
    /// Create the table on the first readiness check.
    pub migrate: bool,
    /// Notifications per page on the inbox.
    pub per_page: u32,
    /// Send each notification by mail too, when the caller gives an
    /// address and the project has the `mail` module.
    pub mail: bool,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            migrate: true,
            per_page: 20,
            mail: true,
        }
    }
}

/// Puts a [`NotificationStore`] on every request context.
///
/// It needs the `db` plugin's pool, registered before it. The first
/// readiness check creates the table (with `migrate`) and counts the rows,
/// so a missing table stops the start with a `help:` line rather than the
/// first request.
pub struct NotificationsPlugin {
    config: NotificationsConfig,
    store: Option<NotificationStore>,
}

impl NotificationsPlugin {
    /// The settings this plugin was built with.
    pub fn config(&self) -> &NotificationsConfig {
        &self.config
    }
}

impl Plugin for NotificationsPlugin {
    type Config = NotificationsConfig;
    type Provides = ProvidesContext<NotificationStore>;
    /// Also the configuration section.
    const NAME: &'static str = "notifications";

    fn new(config: NotificationsConfig) -> Self {
        Self {
            config,
            store: None,
        }
    }

    fn before_start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        let db = lc.require_service::<brocanth_db::Db>()?.clone();
        let store = NotificationStore::configured(db.clone(), self.config.clone());
        self.store = Some(store.clone());
        let migrate = self.config.migrate;
        let migrated = Arc::new(tokio::sync::OnceCell::new());
        lc.ready_when(move || {
            let store = store.clone();
            let db = db.clone();
            let migrated = Arc::clone(&migrated);
            async move {
                if migrate {
                    migrated
                        .get_or_try_init(|| store.migrate())
                        .await
                        .map_err(|e| {
                            format!(
                                "[notifications] creating the table: {e}\n\
                                 help: check `[db] url`; with `migrate = false`, run \
                                 `brocanth_notifications::SCHEMA` in a migration"
                            )
                        })?;
                }
                brocanth_db::sqlx::query("SELECT 1 FROM brocanth_notifications LIMIT 1")
                    .execute(db.pool())
                    .await
                    .map(drop)
                    .map_err(|e| {
                        format!(
                            "[notifications] reading the table: {e}\n\
                             help: with `migrate = false`, the `brocanth_notifications` table \
                             must exist: run `brocanth_notifications::SCHEMA` in a migration"
                        )
                    })
            }
        });
        Ok(())
    }

    fn provide(&self, cx: &mut ContextBuilder) {
        // Nothing before `before_start`, so a page on a hand-built context
        // gets `None`, not a store over no pool.
        if let Some(store) = &self.store {
            cx.insert(store.clone());
        }
    }
}
