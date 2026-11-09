//! The plugin and its two hooks: one inserts the banner and the tag, the
//! other records a consent choice when a log is configured.
//!
//! With the `i18n` feature, both read `brocanth_i18n::Current` from the
//! request state, which the `i18n` module's request hook writes. The banner
//! then shows the catalog's `gtag.banner.*` strings in the request's
//! locale, and the log records the version of the text that locale showed.

use std::io::Write;
use std::path::PathBuf;

use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_body::Body;
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{
    BodyShape, Both, Flow, HookRegistrar, HookSource, OnRequest, OnResponse, Request, RequestHook,
    RequestState, Response, ResponseHead, ResponseHook,
};

use crate::{parse_choice, Choice, GtagConfig, CONSENT_PATH};

/// Inserts the consent banner and the tag into every HTML page it may.
pub struct Gtag {
    config: GtagConfig,
}

impl Plugin for Gtag {
    type Config = GtagConfig;
    type Provides = NoContext;
    /// Also the configuration section and the flag prefix:
    /// `--gtag.measurement_id=G-…`.
    const NAME: &'static str = "gtag";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: Self::Config) -> Self {
        Self { config }
    }
}

impl HookSource for Gtag {
    type Families = Both<OnRequest, OnResponse>;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        self.config
            .check()
            .map_err(|why| PluginError::new(format!("gtag: {why}")))?;
        if let Some(path) = &self.config.consent_log {
            registrar.request(
                "consent-log",
                // After the locale is known, so the record names the text
                // this visitor's language showed.
                brocanth::plugin_order::Order::new().after_if_present("i18n.negotiate"),
                Record {
                    path: PathBuf::from(path),
                    banner: self.config.banner.clone(),
                    translate: self.config.translate,
                },
            );
        }
        let markup = self.config.markup();
        registrar.response(
            "insert",
            brocanth::plugin_order::Order::new(),
            Insert {
                config: self.config.clone(),
                markup,
                #[cfg(feature = "i18n")]
                translated: std::sync::Mutex::new(Vec::new()),
            },
        );
        Ok(())
    }
}

/// Appends a consent choice to the log and answers `204`.
struct Record {
    path: PathBuf,
    banner: crate::Banner,
    /// `GtagConfig::translate`: the banner follows the locale.
    #[cfg_attr(not(feature = "i18n"), allow(dead_code))]
    translate: bool,
}

impl Record {
    /// The version of the banner this request's locale shows.
    fn version(&self, state: &RequestState) -> String {
        #[cfg(feature = "i18n")]
        if let Some(current) = state
            .get::<brocanth_i18n::Current>()
            .filter(|_| self.translate)
        {
            return self.banner.translated(|key| current.text(key)).version();
        }
        let _ = state;
        self.banner.version()
    }

    fn append(&self, choice: &Choice, version: &str) -> Result<(), PluginError> {
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        // The version the server shows now, beside the one the browser
        // saw: a record whose two differ answered older text.
        let line = format!(
            "{{\"at\":{at},\"choice\":\"{}\",\"id\":\"{}\",\"seen\":\"{}\",\"banner\":\"{}\"}}\n",
            choice.choice, choice.id, choice.version, version
        );
        let failed = |e: std::io::Error| {
            PluginError::new(format!("gtag: consent log {}: {e}", self.path.display()))
        };
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(failed)?;
        }
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut file| file.write_all(line.as_bytes()))
            .map_err(failed)
    }
}

impl RequestHook for Record {
    fn before_dispatch(
        &self,
        request: &mut Request,
        state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        if request.uri().path() != CONSENT_PATH {
            return Ok(Flow::Continue);
        }
        let query = request.uri().query().unwrap_or("");
        let status = if request.method() != http::Method::GET {
            http::StatusCode::METHOD_NOT_ALLOWED
        } else if let Some(choice) = parse_choice(query) {
            self.append(&choice, &self.version(state))?;
            http::StatusCode::NO_CONTENT
        } else {
            http::StatusCode::BAD_REQUEST
        };
        let mut response = Response::new(Body::from(Vec::new()));
        *response.status_mut() = status;
        response.headers_mut().insert(
            http::header::CACHE_CONTROL,
            http::HeaderValue::from_static("no-store"),
        );
        Ok(Flow::respond(response))
    }
}

/// The hook, with its markup built once at startup.
struct Insert {
    config: GtagConfig,
    markup: String,
    /// The markup per locale tag, built on the first page in that locale.
    #[cfg(feature = "i18n")]
    translated: std::sync::Mutex<Vec<(&'static str, std::sync::Arc<str>)>>,
}

impl Insert {
    /// The markup for this request: in its locale when the `i18n` hook
    /// chose one, the settings' text otherwise.
    #[cfg(feature = "i18n")]
    fn markup_for(&self, state: &RequestState) -> std::sync::Arc<str> {
        let current = state
            .get::<brocanth_i18n::Current>()
            .filter(|_| self.config.translate);
        let Some(current) = current else {
            return self.markup.as_str().into();
        };
        let tag = current.tag();
        // A poisoned lock only means another page panicked while it built
        // a markup; the list is still whole.
        let mut built = self
            .translated
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((_, markup)) = built.iter().find(|(t, _)| *t == tag) {
            return std::sync::Arc::clone(markup);
        }
        let banner = self.config.banner.translated(|key| current.text(key));
        let markup: std::sync::Arc<str> = self.config.markup_with(&banner, Some(tag)).into();
        built.push((tag, std::sync::Arc::clone(&markup)));
        markup
    }
}

impl Insert {
    /// Whether this request's page gets the markup, from what the request
    /// and the response head say.
    fn wanted(&self, request: &Request, head: &ResponseHead) -> bool {
        let header = |name: &str| {
            request
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
        };
        let html = head
            .headers
            .get(http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|kind| kind.starts_with("text/html"));
        let signal = header("sec-gpc") == Some("1") || header("dnt") == Some("1");
        self.config.enabled
            && html
            && head.status == http::StatusCode::OK
            // A partial is a fragment for a page that already has it.
            && header("dp-partial").is_none()
            && !(self.config.respect_gpc && signal)
            && self.config.tracks(request.uri().path())
    }
}

impl ResponseHook for Insert {
    /// A page with the tag differs by the privacy signals, so a shared
    /// cache must keep one copy per signal.
    fn on_headers(
        &self,
        _request: &Request,
        head: &mut ResponseHead,
        _body: BodyShape<'_>,
        _state: &mut RequestState,
    ) -> Result<(), PluginError> {
        if self.config.enabled && self.config.respect_gpc {
            head.headers.append(
                http::header::VARY,
                http::HeaderValue::from_static("Sec-GPC, DNT"),
            );
        }
        Ok(())
    }

    fn on_body(
        &self,
        request: &Request,
        head: &mut ResponseHead,
        body: &mut Vec<u8>,
        state: &mut RequestState,
    ) -> Result<(), PluginError> {
        if !self.wanted(request, head) {
            return Ok(());
        }
        let Some(at) = last_close_body(body) else {
            return Ok(());
        };
        #[cfg(feature = "i18n")]
        body.splice(at..at, self.markup_for(state).bytes());
        #[cfg(not(feature = "i18n"))]
        {
            let _ = state;
            body.splice(at..at, self.markup.bytes());
        }
        Ok(())
    }
}

/// Where the last `</body>` starts, in any case.
fn last_close_body(body: &[u8]) -> Option<usize> {
    let needle = b"</body";
    (0..=body.len().checked_sub(needle.len())?)
        .rev()
        .find(|&at| body[at..at + needle.len()].eq_ignore_ascii_case(needle))
}
