//! Structured logs on stdout: one line per event, JSON or text.
//!
//! A [`tracing_subscriber::Layer`], so the request line this crate writes and
//! every `tracing::info!` the application or a library writes go through one
//! formatter and one filter. A field on an open span is on every line written
//! inside it: the request span carries `request_id`, `method`, `path` and
//! `route`, and with the `otel` feature `trace_id` and `span_id`, so a line a
//! handler logs is joined to its request without the handler passing
//! anything.
//!
//! The JSON line:
//!
//! ```text
//! {"ts":"2026-10-30T12:00:00.123Z","level":"info","msg":"GET /users/7 -> 200","target":"brocanth::request",
//!  "request_id":"…","method":"GET","path":"/users/7","route":"/users/{id}","status":200,"duration_ms":1.234}
//! ```
//!
//! Written here rather than with `tracing-subscriber`'s `json` feature: that
//! format names its fields `timestamp`, `fields.message` and `span.*`, and
//! cannot be told otherwise; this one is two hundred lines and no `serde`.

use std::fmt::{self, Write as _};
use std::io::Write as _;
use std::sync::{Arc, Mutex, PoisonError};

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

use crate::config::LogFormat;
use crate::text::{json_f64, json_string, rfc3339_now};

/// Where finished lines go. One call per line, without the newline.
pub trait LogSink: Send + Sync + 'static {
    /// Writes one line.
    fn write_line(&self, line: &str);
}

/// Standard output, one locked write per line so two threads never
/// interleave inside a line.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stdout;

impl LogSink for Stdout {
    fn write_line(&self, line: &str) {
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(line.as_bytes());
        let _ = out.write_all(b"\n");
    }
}

/// Lines kept in memory, for a test that reads what was logged.
#[derive(Clone, Debug, Default)]
pub struct Memory(Arc<Mutex<Vec<String>>>);

impl Memory {
    /// An empty buffer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Every line so far.
    pub fn lines(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl LogSink for Memory {
    fn write_line(&self, line: &str) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(line.to_string());
    }
}

/// A field value, kept in its own type so JSON gets a number as a number.
#[derive(Clone, Debug)]
enum Value {
    Str(String),
    I64(i64),
    U64(u64),
    F64(f64),
    Bool(bool),
}

/// Fields as they were recorded, in order.
#[derive(Clone, Debug, Default)]
struct Fields(Vec<(&'static str, Value)>);

impl Fields {
    fn set(&mut self, name: &'static str, value: Value) {
        // `otel.*` fields steer the exporter (span name, kind, status) and
        // mean nothing in a log line.
        if name.starts_with("otel.") {
            return;
        }
        match self.0.iter_mut().find(|(held, _)| *held == name) {
            Some((_, held)) => *held = value,
            None => self.0.push((name, value)),
        }
    }
}

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.set(field.name(), Value::Str(format!("{value:?}")));
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.set(field.name(), Value::Str(value.to_string()));
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.set(field.name(), Value::I64(value));
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.set(field.name(), Value::U64(value));
    }
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.set(field.name(), Value::F64(value));
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.set(field.name(), Value::Bool(value));
    }
    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.set(field.name(), Value::Str(value.to_string()));
    }
}

/// The layer that formats and writes.
pub struct LogLayer {
    format: LogFormat,
    sink: Arc<dyn LogSink>,
}

impl LogLayer {
    /// Writes `format` lines to `sink`.
    pub fn new(format: LogFormat, sink: impl LogSink) -> Self {
        Self {
            format,
            sink: Arc::new(sink),
        }
    }
}

impl<S> Layer<S> for LogLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let mut fields = Fields::default();
        attrs.record(&mut fields);
        span.extensions_mut().insert(fields);
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let mut extensions = span.extensions_mut();
        if let Some(fields) = extensions.get_mut::<Fields>() {
            values.record(fields);
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut own = Fields::default();
        event.record(&mut own);
        let message = match own.0.iter().position(|(name, _)| *name == "message") {
            Some(at) => match own.0.remove(at).1 {
                Value::Str(s) => s,
                other => render_plain(&other),
            },
            None => String::new(),
        };

        // Root first, so a field set on an inner span wins over the outer
        // one, and the event's own fields win over both.
        let mut merged = Fields::default();
        if let Some(scope) = ctx.event_scope(event) {
            for span in scope.from_root() {
                if let Some(fields) = span.extensions().get::<Fields>() {
                    for (name, value) in &fields.0 {
                        merged.set(name, value.clone());
                    }
                }
            }
        }
        for (name, value) in own.0 {
            merged.set(name, value);
        }

        let meta = event.metadata();
        let line = match self.format {
            LogFormat::Json => json_line(*meta.level(), &message, meta.target(), &merged),
            LogFormat::Text => text_line(*meta.level(), &message, &merged),
        };
        self.sink.write_line(&line);
    }
}

fn level_name(level: Level) -> &'static str {
    match level {
        Level::ERROR => "error",
        Level::WARN => "warn",
        Level::INFO => "info",
        Level::DEBUG => "debug",
        Level::TRACE => "trace",
    }
}

fn json_line(level: Level, message: &str, target: &str, fields: &Fields) -> String {
    let mut out = String::with_capacity(256);
    out.push_str("{\"ts\":\"");
    out.push_str(&rfc3339_now());
    out.push_str("\",\"level\":\"");
    out.push_str(level_name(level));
    out.push_str("\",\"msg\":");
    json_string(&mut out, message);
    out.push_str(",\"target\":");
    json_string(&mut out, target);
    for (name, value) in &fields.0 {
        out.push(',');
        json_string(&mut out, name);
        out.push(':');
        match value {
            Value::Str(s) => json_string(&mut out, s),
            Value::I64(n) => {
                let _ = write!(out, "{n}");
            }
            Value::U64(n) => {
                let _ = write!(out, "{n}");
            }
            Value::F64(n) => json_f64(&mut out, *n),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        }
    }
    out.push('}');
    out
}

fn text_line(level: Level, message: &str, fields: &Fields) -> String {
    let mut out = String::with_capacity(160);
    out.push_str(&rfc3339_now());
    let _ = write!(out, " {:>5} {message}", level.as_str());
    for (name, value) in &fields.0 {
        let rendered = render_plain(value);
        if rendered.is_empty() || rendered.contains([' ', '"', '=']) {
            let _ = write!(out, " {name}={rendered:?}");
        } else {
            let _ = write!(out, " {name}={rendered}");
        }
    }
    out
}

fn render_plain(value: &Value) -> String {
    match value {
        Value::Str(s) => s.clone(),
        Value::I64(n) => n.to_string(),
        Value::U64(n) => n.to_string(),
        Value::F64(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
    }
}

/// The level filter: `RUST_LOG` when it is set, else `spec`.
///
/// # Errors
///
/// A spec `tracing-subscriber` cannot read, with a `help:` line.
pub(crate) fn filter(spec: &str) -> Result<tracing_subscriber::filter::Targets, String> {
    let from_env = std::env::var("RUST_LOG")
        .ok()
        .filter(|v| !v.trim().is_empty());
    let (spec, source) = match &from_env {
        Some(env) => (env.as_str(), "RUST_LOG"),
        None => (spec, "the log level"),
    };
    spec.parse().map_err(|e| {
        format!(
            "{source} `{spec}` is not a level: {e}\nhelp: write a level such as `info`, \
             or per-target levels such as `info,sqlx=warn`"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    fn capture(format: LogFormat, f: impl FnOnce()) -> Vec<String> {
        let memory = Memory::new();
        let subscriber = tracing_subscriber::registry().with(LogLayer::new(format, memory.clone()));
        tracing::subscriber::with_default(subscriber, f);
        memory.lines()
    }

    #[test]
    fn a_json_line_carries_the_span_fields_and_typed_values() {
        let lines = capture(LogFormat::Json, || {
            let span = tracing::info_span!("request", request_id = "abc", method = "GET");
            let _in = span.enter();
            tracing::info!(status = 200u16, ok = true, ratio = 0.5, "done \"quoted\"");
        });
        assert_eq!(lines.len(), 1);
        let line = &lines[0];
        assert!(line.starts_with("{\"ts\":\""), "{line}");
        assert!(
            line.ends_with(
                r#""level":"info","msg":"done \"quoted\"","target":"brocanth_observe::logs::tests","request_id":"abc","method":"GET","status":200,"ok":true,"ratio":0.5}"#
            ),
            "{line}"
        );
    }

    #[test]
    fn a_text_line_is_level_message_then_pairs() {
        let lines = capture(LogFormat::Text, || {
            tracing::warn!(path = "/a b", status = 404u16, "not found");
        });
        let line = &lines[0];
        assert!(
            line.ends_with(r#"  WARN not found path="/a b" status=404"#),
            "{line}"
        );
    }

    #[test]
    fn a_bad_level_says_what_a_good_one_looks_like() {
        if std::env::var("RUST_LOG").is_ok() {
            return;
        }
        assert!(filter("info,sqlx=warn").is_ok());
        let error = filter("sqlx=loud").unwrap_err();
        assert!(error.contains("help: "), "{error}");
    }
}
