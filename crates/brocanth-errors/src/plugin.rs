//! The plugin, and the reporter contract other modules implement.

use std::fmt;
use std::sync::{Arc, RwLock};

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::{ContextBuilder, ContextItem, ProvidesContext};
use brocanth::plugin_hooks::{
    ErrorHook, HandlerError, HookRegistrar, HookSource, OnError, Request, RequestState, Response,
};
use brocanth::plugin_order::Order;

/// Where a failure came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// A page returned an error.
    Page,
    /// A plugin's hook failed, before or after the page.
    Hook,
}

/// One 5xx, as a reporter hears it.
///
/// The fields borrow from the request, so a reporter that keeps them copies
/// what it keeps. The message is the cause, the text the visitor never sees.
#[derive(Clone, Copy, Debug)]
pub struct Report<'a> {
    pub status: u16,
    /// The cause: what the page's error or the hook's error said.
    pub message: &'a str,
    /// The request's method: `GET`.
    pub method: &'a str,
    /// The request's path, with no query.
    pub path: &'a str,
    pub origin: Origin,
}

/// The hook a module implements to hear about every 5xx.
///
/// A reporter sends the failure somewhere a person reads it: an error
/// tracker, a mail to the team, a chat channel. It runs on the request's
/// thread, after the log line, so it must not wait: hand the report to a
/// channel or a job and return.
///
/// A plugin adds its reporter in `after_config`, when the `errors` plugin
/// has published [`Reporters`]. An application without `errors` has no
/// `Reporters`, and the plugin then reports nothing:
///
/// ```
/// use brocanth::plugin_api::{Lifecycle, PluginError};
/// use brocanth_errors::{Report, Reporter, Reporters};
///
/// struct Stderr;
///
/// impl Reporter for Stderr {
///     fn report(&self, report: &Report<'_>) {
///         eprintln!("{} {} {}: {}", report.status, report.method, report.path, report.message);
///     }
/// }
///
/// // In the plugin's `after_config`:
/// fn after_config(lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
///     if let Some(reporters) = lc.services().get::<Reporters>() {
///         reporters.add(Stderr);
///     }
///     Ok(())
/// }
/// # let _ = after_config;
/// ```
pub trait Reporter: Send + Sync + 'static {
    fn report(&self, report: &Report<'_>);
}

/// Every reporter of one application, and whether a debug build shows the
/// cause.
///
/// Cheap to clone: the list is shared. The `errors` plugin publishes one in
/// the services and puts the same list on the context, so a reporter added
/// in `after_config` is heard by every request.
#[derive(Clone, Default)]
pub struct Reporters {
    list: Arc<RwLock<Vec<Arc<dyn Reporter>>>>,
    details: bool,
}

impl Reporters {
    /// An empty list. `details` is the `[errors] details` setting.
    pub fn new(details: bool) -> Self {
        Self {
            list: Arc::default(),
            details,
        }
    }

    /// Adds a reporter. Every clone of this list hears it.
    pub fn add(&self, reporter: impl Reporter) {
        if let Ok(mut list) = self.list.write() {
            list.push(Arc::new(reporter));
        }
    }

    /// Tells every reporter, in the order they were added.
    pub fn report(&self, report: &Report<'_>) {
        // A copy of the list, so a reporter that panics or is slow does not
        // hold the lock.
        let list: Vec<Arc<dyn Reporter>> = match self.list.read() {
            Ok(list) => list.clone(),
            Err(_) => return,
        };
        for reporter in list {
            reporter.report(report);
        }
    }

    /// How many reporters there are.
    pub fn len(&self) -> usize {
        self.list.read().map_or(0, |list| list.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether a debug build shows the cause of a 5xx. A release build never
    /// does, whatever this says: the caller checks `cfg!(debug_assertions)`.
    pub fn details(&self) -> bool {
        self.details
    }
}

impl fmt::Debug for Reporters {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reporters")
            .field("len", &self.len())
            .field("details", &self.details)
            .finish()
    }
}

impl ContextItem for Reporters {}

/// The `[errors]` section.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "config",
    derive(serde::Serialize, serde::Deserialize),
    serde(default, deny_unknown_fields)
)]
pub struct ErrorsConfig {
    /// Show the cause of a 5xx in a debug build. A release build never
    /// shows it. Turn it off to see `error.rsx` while you design it.
    pub details: bool,
}

impl Default for ErrorsConfig {
    fn default() -> Self {
        Self { details: true }
    }
}

/// Publishes [`Reporters`] and reports the failures of hooks.
///
/// A page's failure is reported by the project's `caught` function, which
/// reads `Reporters` from the context. A hook's failure never reaches a
/// page, so the error hook here reports it.
pub struct Errors {
    reporters: Reporters,
}

impl Plugin for Errors {
    type Config = ErrorsConfig;
    type Provides = ProvidesContext<Reporters>;
    /// Also the configuration section and the flag prefix:
    /// `--errors.details=false`.
    const NAME: &'static str = "errors";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: ErrorsConfig) -> Self {
        Self {
            reporters: Reporters::new(config.details),
        }
    }

    fn register(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        // Before `after_config`, which is where another plugin adds its
        // reporter. A clone: both hold the same list.
        lc.services_mut().insert(self.reporters.clone());
        Ok(())
    }

    fn provide(&self, cx: &mut ContextBuilder) {
        cx.insert(self.reporters.clone());
    }
}

impl HookSource for Errors {
    type Families = OnError;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        registrar.error(
            "report",
            Order::new(),
            Report500 {
                reporters: self.reporters.clone(),
            },
        );
        Ok(())
    }
}

/// Tells the reporters about a hook's failure, and leaves the answer to
/// the core.
struct Report500 {
    reporters: Reporters,
}

impl ErrorHook for Report500 {
    fn on_error(
        &self,
        error: &HandlerError,
        request: &Request,
        _state: &mut RequestState,
    ) -> Option<Response> {
        let message = error.as_error().to_string();
        self.reporters.report(&Report {
            status: 500,
            message: &message,
            method: request.method().as_str(),
            path: request.uri().path(),
            origin: Origin::Hook,
        });
        None
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use brocanth::plugin_app::{App, AppBuilder};
    use brocanth::plugin_context::{After, Base};
    use brocanth::plugin_hooks::{Both, NoHooks};

    use super::*;

    struct Heard(Arc<Mutex<Vec<String>>>);

    impl Reporter for Heard {
        fn report(&self, report: &Report<'_>) {
            if let Ok(mut heard) = self.0.lock() {
                heard.push(format!(
                    "{} {:?} {}",
                    report.status, report.origin, report.message
                ));
            }
        }
    }

    type Installed = After<Errors, Base>;
    type Hooks = Both<<Errors as HookSource>::Families, NoHooks>;

    fn app() -> App<Installed, Hooks> {
        AppBuilder::new()
            .plugin_hooking(Errors::new(ErrorsConfig::default()))
            .build()
            .expect("an application")
    }

    #[test]
    fn a_reporter_added_to_the_context_copy_hears_a_hook_failure() {
        let app = app();
        let heard = Arc::new(Mutex::new(Vec::new()));
        let reporters: &Reporters = &app.context().get::<Reporters, _>().clone();
        reporters.add(Heard(Arc::clone(&heard)));

        let mut request = http::Request::builder()
            .uri("/x")
            .body(Vec::new())
            .expect("a request");
        let mut state = RequestState::new();
        let _ = app.run(&mut request, &mut state, |_, _| {
            Err(HandlerError::message("the cache is down"))
        });
        assert_eq!(*heard.lock().expect("lock"), ["500 Hook the cache is down"]);
    }

    #[test]
    fn details_is_the_setting_and_is_on_by_default() {
        let app = app();
        let cx = app.context();
        assert!(cx.get::<Reporters, _>().details());
        assert!(cx.get::<Reporters, _>().is_empty());
    }
}
