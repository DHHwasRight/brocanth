//! The plugin and its two hooks: one inserts the banner and the tag, the
//! other records a consent choice when a log is configured.
//!
//! With the `i18n` feature, both read `brocanth_i18n::Current` from the
//! request state, which the `i18n` module's request hook writes. The banner
//! then shows the catalog's `gtag.banner.*` strings in the request's
//! locale, and the log records the version of the text in the locale the
//! report names: the one the page showed.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_body::Body;
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{
    BodyShape, Both, Flow, HookRegistrar, HookSource, OnRequest, OnResponse, Request, RequestHook,
    RequestState, Response, ResponseHead, ResponseHook,
};

use crate::{parse_choice, Choice, GtagConfig, CONSENT_LOG_MAX_BYTES, CONSENT_PATH};

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

    /// The settings are checked here, at the stage the other plugins check
    /// theirs, so a bad one stops the start before any hook exists.
    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        self.config.check().map_err(PluginError::new)
    }
}

impl HookSource for Gtag {
    type Families = Both<OnRequest, OnResponse>;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        if let Some(path) = &self.config.consent_log {
            registrar.request(
                "consent-log",
                // After the locale is known, so the record can read the
                // catalogs, and after the rate limiter, when present, so a
                // flood is refused first.
                brocanth::plugin_order::Order::new()
                    .after_if_present("i18n.negotiate")
                    .after_if_present("rate_limit.count"),
                Record {
                    log: ConsentLog::open(PathBuf::from(path), CONSENT_LOG_MAX_BYTES)?,
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
    log: ConsentLog,
    banner: crate::Banner,
    /// `GtagConfig::translate`: the banner follows the locale.
    #[cfg_attr(not(feature = "i18n"), allow(dead_code))]
    translate: bool,
}

impl Record {
    /// The version of the banner the page showed: in the locale the report
    /// names, else in the one this request negotiated.
    fn version(&self, state: &RequestState, locale: Option<&str>) -> String {
        #[cfg(feature = "i18n")]
        if let Some(current) = state
            .get::<brocanth_i18n::Current>()
            .filter(|_| self.translate)
        {
            let shown = locale
                .and_then(|tag| current.with_tag(tag))
                .unwrap_or(*current);
            return self.banner.translated(|key| shown.text(key)).version();
        }
        let _ = (state, locale);
        self.banner.version()
    }

    fn line(choice: &Choice, version: &str) -> String {
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        // The version the server shows now, beside the one the browser
        // saw: a record whose two differ answered older text.
        format!(
            "{{\"at\":{at},\"choice\":\"{}\",\"id\":\"{}\",\"seen\":\"{}\",\"banner\":\"{}\"}}\n",
            choice.choice, choice.id, choice.version, version
        )
    }
}

/// Whether a browser sent this request from another site. A report comes
/// from the site's own pages, so `Sec-Fetch-Site` says `same-origin`, or,
/// in a browser too old to send it, `Origin` (when sent) names this host.
/// A request with neither header is no browser's cross-site request.
fn cross_site(request: &Request) -> bool {
    let header = |name: &str| request.headers().get(name).and_then(|v| v.to_str().ok());
    if let Some(site) = header("sec-fetch-site") {
        return !site.trim().eq_ignore_ascii_case("same-origin");
    }
    if let Some(origin) = header("origin") {
        let host = header("host").or_else(|| request.uri().authority().map(|a| a.as_str()));
        let from = origin.split_once("://").map(|(_, host)| host);
        let same = from
            .zip(host)
            .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b));
        return !same;
    }
    false
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
        } else if cross_site(request) {
            http::StatusCode::FORBIDDEN
        } else if let Some(choice) = parse_choice(query) {
            let line = Record::line(&choice, &self.version(state, choice.locale.as_deref()));
            if self.log.push(line) {
                http::StatusCode::NO_CONTENT
            } else {
                // The writer is behind by a whole queue: a flood.
                http::StatusCode::SERVICE_UNAVAILABLE
            }
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

/// How many lines may wait for the writer. Past it, a report is refused
/// with a 503 rather than held in memory.
const CONSENT_QUEUE: usize = 1_024;

/// The consent log: a thread appends each line, so a request never waits
/// on the disk. It rotates the file past a size, so a flood of reports
/// cannot fill the disk.
struct ConsentLog {
    sender: mpsc::SyncSender<String>,
    #[cfg_attr(not(test), allow(dead_code))]
    writer: std::thread::JoinHandle<()>,
}

impl ConsentLog {
    /// Opens the file now, so a path this process cannot write stops the
    /// start, not the first visitor's click.
    fn open(path: PathBuf, max_bytes: u64) -> Result<Self, PluginError> {
        let shown = path.display().to_string();
        let failed = |e: std::io::Error| {
            PluginError::new(format!(
                "[gtag] consent log {shown}: {e}\n\
                 help: set [gtag] consent_log to a file this process can create and append to"
            ))
        };
        let file = append_to(&path).map_err(failed)?;
        let (sender, receiver) = mpsc::sync_channel::<String>(CONSENT_QUEUE);
        let writer = std::thread::Builder::new()
            .name("gtag-consent-log".into())
            .spawn(move || write_lines(&path, file, max_bytes, &receiver))
            .map_err(failed)?;
        Ok(Self { sender, writer })
    }

    /// Queues a line. `false` when the queue is full.
    fn push(&self, line: String) -> bool {
        self.sender.try_send(line).is_ok()
    }

    /// Ends the writer once it has written every queued line.
    #[cfg(test)]
    fn close(self) {
        let ConsentLog { sender, writer } = self;
        std::mem::drop(sender);
        let _ = writer.join();
    }
}

fn append_to(path: &Path) -> std::io::Result<std::fs::File> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
}

/// The writer thread. It ends when the hook, and with it the sender, is
/// dropped.
fn write_lines(
    path: &Path,
    mut file: std::fs::File,
    max_bytes: u64,
    lines: &mpsc::Receiver<String>,
) {
    let mut size = file.metadata().map_or(0, |m| m.len());
    let mut rotated = path.as_os_str().to_owned();
    rotated.push(".1");
    for line in lines {
        let len = line.len() as u64;
        if size > 0 && size + len > max_bytes {
            let next = std::fs::rename(path, &rotated).and_then(|()| append_to(path));
            match next {
                Ok(fresh) => {
                    file = fresh;
                    size = 0;
                }
                Err(e) => eprintln!(
                    "[gtag] consent log {}: cannot rotate: {e}\n\
                     help: check that the directory is writable; the log grows until it is",
                    path.display()
                ),
            }
        }
        match file.write_all(line.as_bytes()) {
            Ok(()) => size += len,
            Err(e) => eprintln!(
                "[gtag] consent log {}: a choice was not recorded: {e}\n\
                 help: check the disk and the file's permissions",
                path.display()
            ),
        }
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
        let html = brocanth::plugin_body::is_html(head.headers.get(http::header::CONTENT_TYPE));
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
        // The rule the analytics beacon follows too: before the last
        // `</body>`, at the end of a whole document with none, never into
        // a fragment.
        use brocanth::plugin_body::insert_before_body_end;
        #[cfg(feature = "i18n")]
        insert_before_body_end(body, self.markup_for(state).as_bytes());
        #[cfg(not(feature = "i18n"))]
        {
            let _ = state;
            insert_before_body_end(body, self.markup.as_bytes());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_is_rotated_past_its_size_and_keeps_one_old_file() {
        let dir = std::env::temp_dir().join(format!("brocanth-gtag-rotate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("consent.jsonl");
        let log = ConsentLog::open(path.clone(), 100).expect("a log");
        for n in 0..5 {
            assert!(log.push(format!("{n}{}\n", "x".repeat(38))));
        }
        log.close();
        let old = std::fs::read_to_string(dir.join("consent.jsonl.1")).expect("the old file");
        let new = std::fs::read_to_string(&path).expect("the new file");
        assert_eq!(old.lines().count(), 2, "{old}");
        assert!(old.starts_with('2'), "the oldest file was replaced: {old}");
        assert_eq!(new.lines().count(), 1, "{new}");
        assert!(new.starts_with('4'), "{new}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_log_this_process_cannot_create_stops_the_start() {
        let file = std::env::temp_dir().join(format!("brocanth-gtag-file-{}", std::process::id()));
        std::fs::write(&file, "a file, not a directory").unwrap();
        let Err(error) = ConsentLog::open(file.join("consent.jsonl"), 100) else {
            panic!("a path under a file was accepted");
        };
        assert!(error.to_string().contains("help:"), "{error}");
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn a_report_from_another_site_is_cross_site() {
        let request = |headers: &[(&str, &str)]| {
            let mut builder = http::Request::builder().uri("/__consent");
            for (name, value) in headers {
                builder = builder.header(*name, *value);
            }
            builder.body(Vec::new()).unwrap()
        };
        assert!(!cross_site(&request(&[])));
        assert!(!cross_site(&request(&[("sec-fetch-site", "same-origin")])));
        assert!(cross_site(&request(&[("sec-fetch-site", "cross-site")])));
        assert!(cross_site(&request(&[("sec-fetch-site", "same-site")])));
        assert!(!cross_site(&request(&[
            ("host", "shop.example"),
            ("origin", "https://shop.example")
        ])));
        assert!(cross_site(&request(&[
            ("host", "shop.example"),
            ("origin", "https://evil.example")
        ])));
        assert!(cross_site(&request(&[
            ("host", "shop.example"),
            ("origin", "null")
        ])));
    }
}
