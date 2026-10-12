//! The brocanth plugin, which is how an application names the admin in `main`.
//!
//! PLAN §8 is the shape of this: the dependency arrow points from here to the
//! core, the core knows nothing about an admin, and an application that does
//! not write `app.plugin(AdminPlugin::new(..))` links none of this.
//!
//! The plugin publishes the built [`Admin`] into [`Services`] during
//! `register`, so a handler reaches it the way it reaches any other plugin's
//! value. It offers no middleware, because it needs none.
//!
//! **The core publishes no route-registration seam yet** — `configure_router`
//! takes a [`MiddlewareCatalog`], and the request hooks land with the HTTP
//! server layer. So the application mounts the admin itself, with one
//! catch-all handler that calls [`Admin::dispatch`]. That is a gap in the
//! core to be closed by widening the seam, not by this crate reaching around
//! it.
//!
//! [`Services`]: brocanth::plugin_api::Services
//! [`MiddlewareCatalog`]: brocanth::plugin_api::MiddlewareCatalog

use std::sync::Arc;

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::NoContext;

use crate::admin::Admin;
use crate::config::AdminConfig;
use crate::data::SharedData;

/// What the admin plugin is built from: a data source and a configuration.
pub struct AdminSetup {
    /// Where the rows come from.
    pub data: SharedData,
    /// How the admin is configured, including the policy and the views.
    pub config: AdminConfig,
}

impl AdminSetup {
    /// A setup from a data source and a configuration.
    pub fn new(data: SharedData, config: AdminConfig) -> Self {
        AdminSetup { data, config }
    }
}

/// The admin, as a brocanth plugin.
///
/// ```ignore
/// use std::sync::Arc;
///
/// use brocanth_admin::{AdminConfig, AdminPlugin, AdminSetup};
///
/// let app = App::builder()
///     .plugin(AdminPlugin::new(AdminSetup::new(
///         Arc::new(source),
///         AdminConfig::new().at("/admin").with_policy(my_policy),
///     )))
///     .build()?;
/// ```
pub struct AdminPlugin {
    admin: Arc<Admin>,
}

impl AdminPlugin {
    /// The built admin, for an application that would rather hold it directly
    /// than reach it through [`Services`].
    ///
    /// [`Services`]: brocanth::plugin_api::Services
    pub fn admin(&self) -> Arc<Admin> {
        Arc::clone(&self.admin)
    }
}

impl Plugin for AdminPlugin {
    type Config = AdminSetup;
    type Provides = NoContext;

    const NAME: &'static str = "admin";

    fn new(config: AdminSetup) -> Self {
        AdminPlugin {
            admin: Arc::new(Admin::new(config.data, config.config)),
        }
    }

    fn register(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        lc.services_mut().insert(Arc::clone(&self.admin));
        Ok(())
    }
}
