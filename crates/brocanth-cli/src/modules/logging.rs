//! `logging` -- a line per request.

use brocanth_scaffold::snippet::{configurable_hooking, settings_derive, Hooking};
use brocanth_scaffold::{Category, Hook, Manifest, Module, Project, Scaffold, Setting, Slot};

/// A request logger, written out into the project rather than linked.
///
/// It has no crate of its own on purpose: the whole of it is a plugin with
/// one request hook, and a reader who can see it can change it. The core
/// assumes no log layer at all, which is the thing this choice makes visible.
///
/// It logs every response, from `after_dispatch`: a 404, a 405, a CORS
/// preflight and a 401 that a request hook answered included. It was a
/// layer mounted at `/{*rest}`, and a layer runs inside dispatch, so it saw
/// only the routes that matched. Static files never reach `handle`, so they
/// are not logged.
pub struct Logging;

impl Module for Logging {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "logging",
            "a plain-text line per response on stderr, for development; for production use observability-logs",
        )
        .category(Category::Middleware)
        .conflicts(
            "observability-logs",
            "both write a line per request; `observability-logs` writes JSON on stdout",
        )
        .asked_in(
            "foundations",
            "A text line per response, for development",
        )
        .hook(
            "line",
            "write one line per response on stderr",
            Hook::required().phase(brocanth_scaffold::Phase::Identify),
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/logging/mod.rs", plugin_rs(project));
        s.at(Slot::PluginsMod).line("pub mod logging;");
        // A request hook and not a mounted layer. A layer runs inside
        // dispatch, for a route that matched, so it never saw a 404, a 405,
        // a preflight or a 401 a request hook answered. `after_dispatch`
        // runs for every one of them.
        configurable_hooking(
            s,
            project,
            Hooking {
                key: "logging",
                section: "logging",
                ty: "crate::plugins::logging::Logging",
                config_ty: "crate::plugins::logging::Settings",
                prelude: String::new(),
                adjust: None,
            },
            // `debug` in the development file; `info` without one.
            vec![
                Setting::text(
                    "level",
                    "debug",
                    "error: 5xx only. warn: 4xx and 5xx. info or debug: every response.",
                ),
                Setting::text(
                    "format",
                    "line",
                    "line: `GET /path -> 200`. common: Apache Common Log Format. combined: common, then the Referer and the User-Agent.",
                ),
            ],
        );
    }
}

pub(crate) fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "logging");
    let mut out = format!(
        r##"//! Request logging.
//!
//! The core assumes no log layer at all -- not `log`, not `tracing`, not a
//! trait with a no-op default. This file is the whole of logging in this
//! binary: delete the registration in `src/plugins/mod.rs` and nothing logs,
//! delete this file and nothing about logging is compiled.
//!
//! One line per response, written by a request hook's `after_dispatch`. That
//! runs for every request `handle` sees: a page, a 404, a 405, a CORS
//! preflight, and a 401 or a 429 that another request hook answered. A
//! middleware layer would see only the requests that matched a route.
//!
//! Three shapes, chosen by `format`: `line`, the Apache Common Log Format
//! (`common`) and the Combined Log Format (`combined`). For a shape of your
//! own, change `Line::after_dispatch`: this file is yours.

use std::time::{{SystemTime, UNIX_EPOCH}};

use brocanth::plugin_api::{{Lifecycle, Plugin, PluginError}};
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{{
    HookRegistrar, HookSource, OnRequest, Request, RequestHook, RequestState, Response,
}};
use brocanth::plugin_order::{{Order, Phase}};

/// What this plugin is built from.
///
/// {note}
{derive}
pub struct Settings {{
    /// Which responses get a line: `error` writes only a 5xx, `warn` a 4xx
    /// and a 5xx, `info` and `debug` every response.
    pub level: String,
    /// The shape of a line: `line`, `common` or `combined`.
    pub format: String,
}}
"##
    );
    out.push_str(PLUGIN_BODY);
    out
}

/// The part of the plugin file that does not change with the project.
const PLUGIN_BODY: &str = r##"
impl Default for Settings {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            format: "line".to_string(),
        }
    }
}

/// The least severe line written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Level {
    Error,
    Warn,
    Info,
    Debug,
}

impl Level {
    fn parse(text: &str) -> Result<Self, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "error" => Ok(Self::Error),
            "warn" => Ok(Self::Warn),
            "info" => Ok(Self::Info),
            "debug" => Ok(Self::Debug),
            other => Err(format!(
                "[logging] level `{other}` is unknown\n\
                 help: set `level` under [logging] to error, warn, info or debug"
            )),
        }
    }

    /// Whether a response with this status gets a line.
    fn writes(self, status: u16) -> bool {
        match self {
            Self::Error => status >= 500,
            Self::Warn => status >= 400,
            Self::Info | Self::Debug => true,
        }
    }
}

/// The shape of a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    /// `GET /path -> 200`: short, for a terminal.
    Line,
    /// Apache's Common Log Format, which log tools read:
    /// `203.0.113.9 - - [10/Oct/2000:20:55:36 +0000] "GET /a HTTP/1.1" 200 2326`.
    Common,
    /// `Common`, then the quoted `Referer` and `User-Agent`.
    Combined,
}

impl Format {
    fn parse(text: &str) -> Result<Self, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "line" => Ok(Self::Line),
            "common" => Ok(Self::Common),
            "combined" => Ok(Self::Combined),
            other => Err(format!(
                "[logging] format `{other}` is unknown\n\
                 help: set `format` under [logging] to line, common or combined"
            )),
        }
    }
}

pub struct Logging {
    /// The level, or why the setting is not one. `Plugin::new` cannot
    /// fail, so a bad level fails `configure`, with the line to write.
    level: Result<Level, String>,
    /// The format, or why the setting is not one. Checked as the level is.
    format: Result<Format, String>,
}

impl Plugin for Logging {
    type Config = Settings;
    /// Nothing on the context: a logger writes, it does not hand values out.
    type Provides = NoContext;
    const NAME: &'static str = "logging";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: Settings) -> Self {
        Self {
            level: Level::parse(&config.level),
            format: Format::parse(&config.format),
        }
    }

    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        self.level.as_ref().map(drop).map_err(PluginError::new)?;
        self.format.as_ref().map(drop).map_err(PluginError::new)
    }

    fn before_start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        let instance = lc.instance();
        let level = format!("{:?}", self.level.as_ref().unwrap_or(&Level::Info)).to_lowercase();
        let format = format!("{:?}", self.format.as_ref().unwrap_or(&Format::Line)).to_lowercase();
        crate::plugins::say!(info, "[{instance}] level {level}, format {format}"; instance, %level, %format);
        Ok(())
    }
}

impl HookSource for Logging {
    type Families = OnRequest;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        let level = *self.level.as_ref().unwrap_or(&Level::Info);
        let format = *self.format.as_ref().unwrap_or(&Format::Line);
        // `Identify`: request hooks run `after_dispatch` in reverse, so this
        // line sees the response after every guard did.
        registrar.request(
            "line",
            Phase::Identify,
            Order::new(),
            Line { level, format },
        );
        Ok(())
    }
}

/// The hook that writes the line, for the responses `level` keeps.
struct Line {
    level: Level,
    format: Format,
}

impl RequestHook for Line {
    /// After the response is final, whatever answered: the response hooks
    /// have run, so the status is the one the client gets.
    ///
    /// A page can add to the line: put a value on the request state with
    /// `ctx.hook_insert`, and read it here from `state`.
    fn after_dispatch(&self, request: &Request, response: &Response, _state: &mut RequestState) {
        if !self.level.writes(response.status().as_u16()) {
            return;
        }
        let line = match self.format {
            Format::Line => format!(
                "{} {} -> {}",
                request.method(),
                request.uri().path(),
                response.status().as_u16()
            ),
            Format::Common => common(request, response, SystemTime::now()),
            Format::Combined => combined(request, response, SystemTime::now()),
        };
        eprintln!("{line}");
    }
}

/// `host ident user [time] "request" status bytes`.
///
/// The host is the socket's peer. Behind a proxy that is the proxy: read
/// `brocanth_middleware::ClientInfo` from the state here if you want the
/// client. Ident and user are `-`. The bytes are `-` for an empty body and
/// for a stream, whose length is not known when the line is written.
fn common(request: &Request, response: &Response, now: SystemTime) -> String {
    let host = request
        .extensions()
        .get::<std::net::SocketAddr>()
        .map_or_else(|| "-".to_string(), |peer| peer.ip().to_string());
    let target = request
        .uri()
        .path_and_query()
        .map_or("/", |target| target.as_str());
    let bytes = match response.body().len() {
        Some(0) | None => "-".to_string(),
        Some(n) => n.to_string(),
    };
    format!(
        "{host} - - [{}] \"{} {} {:?}\" {} {bytes}",
        clf_time(now),
        request.method(),
        escape(target.as_bytes()),
        request.version(),
        response.status().as_u16()
    )
}

/// The common line, then `"referer" "user-agent"`, each `"-"` when absent.
fn combined(request: &Request, response: &Response, now: SystemTime) -> String {
    let header = |name: &str| {
        request
            .headers()
            .get(name)
            .map_or_else(|| "-".to_string(), |value| escape(value.as_bytes()))
    };
    format!(
        "{} \"{}\" \"{}\"",
        common(request, response, now),
        header("referer"),
        header("user-agent")
    )
}

/// A value safe inside the quotes of a line: `"` and `\` get a backslash,
/// and a byte outside printable ASCII becomes `\xhh`, as Apache writes it.
/// A client cannot start a new line or close the quotes.
fn escape(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &byte in bytes {
        match byte {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            0x20..=0x7e => out.push(char::from(byte)),
            _ => out.push_str(&format!("\\x{byte:02x}")),
        }
    }
    out
}

/// `10/Oct/2000:20:55:36 +0000`: the time in UTC, as the format spells it.
fn clf_time(now: SystemTime) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let secs = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm),
    // shifted so that a year starts on 1 March.
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    format!(
        "{day:02}/{}/{year}:{:02}:{:02}:{:02} +0000",
        MONTHS[(month - 1) as usize],
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use brocanth::plugin_body::Body;

    use super::*;

    /// Apache's own example instant, 10/Oct/2000:13:55:36 -0700, in UTC.
    fn apache_example() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(971_211_336)
    }

    fn request(target: &str) -> Request {
        let mut request = Request::new(Vec::new());
        *request.uri_mut() = target.parse().unwrap();
        request
            .extensions_mut()
            .insert(std::net::SocketAddr::from(([203, 0, 113, 9], 4000)));
        request
    }

    fn response(status: u16, body: &[u8]) -> Response {
        let mut response = Response::new(Body::Buffered(body.to_vec()));
        *response.status_mut() = status.try_into().unwrap();
        response
    }

    #[test]
    fn the_time_is_spelled_as_the_format_spells_it() {
        assert_eq!(clf_time(apache_example()), "10/Oct/2000:20:55:36 +0000");
        assert_eq!(clf_time(UNIX_EPOCH), "01/Jan/1970:00:00:00 +0000");
        // A leap day.
        let leap = UNIX_EPOCH + Duration::from_secs(1_709_164_800);
        assert_eq!(clf_time(leap), "29/Feb/2024:00:00:00 +0000");
    }

    #[test]
    fn a_common_line_has_the_seven_fields() {
        let line = common(
            &request("/a?b=1"),
            &response(200, b"hello"),
            apache_example(),
        );
        assert_eq!(
            line,
            "203.0.113.9 - - [10/Oct/2000:20:55:36 +0000] \"GET /a?b=1 HTTP/1.1\" 200 5"
        );
    }

    #[test]
    fn an_empty_body_is_a_dash() {
        let line = common(&request("/"), &response(304, b""), apache_example());
        assert!(line.ends_with("\" 304 -"), "{line}");
    }

    #[test]
    fn a_combined_line_quotes_the_referer_and_the_agent_and_escapes_them() {
        let mut request = request("/");
        request
            .headers_mut()
            .insert("user-agent", "a \"quoted\" agent".parse().unwrap());
        let line = combined(&request, &response(404, b"no"), apache_example());
        assert!(
            line.ends_with("\" 404 2 \"-\" \"a \\\"quoted\\\" agent\""),
            "{line}"
        );
    }

    #[test]
    fn a_control_byte_cannot_start_a_new_line() {
        assert_eq!(escape(b"a\nb\\"), "a\\x0ab\\\\");
    }

    #[test]
    fn an_unknown_format_says_what_to_write() {
        let error = Format::parse("apache").unwrap_err();
        assert!(error.contains("line, common or combined"), "{error}");
        assert_eq!(Format::parse(" Combined "), Ok(Format::Combined));
    }
}
"##;
