//! gzip and brotli on the responses a browser can decode.
//!
//! [`Compression`] is one response hook. It reads the request's
//! `Accept-Encoding`, picks `br` or `gzip`, and rewrites the buffered body.
//! An HTML page compresses five to ten times, so this is the largest saving
//! per line in the HTTP layer.
//!
//! # What it leaves alone
//!
//! | Response | Why |
//! |---|---|
//! | a stream: SSE, a download, a `101` upgrade | see below |
//! | a `Content-Encoding` already set | the handler compressed it, or it is a precompressed asset |
//! | status `1xx`, `204`, `205`, `206`, `304` | no body, or a byte range of the identity body |
//! | `Cache-Control: no-transform` | the handler forbids it (RFC 9111 §5.2.2.6) |
//! | a media type off the list | an image, a font or a zip is already compressed |
//! | a body under `min_bytes` | the frame costs more than it saves |
//! | a body that does not get smaller | the identity body is sent |
//!
//! **Why never a stream.** The core contract allows a compressing
//! [`Streaming::wrap`](brocanth::plugin_body::Streaming::wrap). This hook
//! does not use it, on purpose. An SSE event must reach the client when it
//! is written; a compressor holds bytes until its window fills, so every
//! event arrives late. A flush per chunk keeps the latency and loses most
//! of the ratio. A download that is a stream is usually a file that is
//! already compressed. And a `101` has no body. So a stream gets no header
//! and no wrap: nothing is announced that was not done.
//!
//! # Vary
//!
//! Every response the hook *could* compress gets `Vary: Accept-Encoding`,
//! also when this client did not ask for an encoding. Without it, a shared
//! cache stores the identity body for one client and the gzip body for the
//! next, and serves each to the other.
//!
//! # ETag
//!
//! A strong `ETag` names exact bytes. Compression changes the bytes, so a
//! strong tag on a compressed body becomes weak (`W/"…"`), as nginx does.
//! A weak tag compares equal across the two encodings, which is what an
//! `If-None-Match` from either client needs.
//! [`ConditionalGet`](crate::ConditionalGet) computes weak tags, so the two
//! hooks agree without knowing about each other.
//!
//! # Order
//!
//! Response hooks run in reverse declared order. Compression must run
//! **last**, after every hook that reads or rewrites the body. So its hook
//! is declared before the ones this workspace knows: `conditional_get`
//! (a 304 is decided over the identity body, and then nothing is
//! compressed), `route_cache` (the cache stores the identity body and a hit
//! is compressed again for its client), and `gtag` and `beacon` (both
//! insert markup into the HTML). Add an application hook that rewrites the
//! body to [`CompressionConfig::runs_after`].

use std::io::Write as _;

use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{
    BodyShape, HookRegistrar, HookSource, OnResponse, Request, RequestState, ResponseHead,
    ResponseHook,
};
use brocanth::plugin_order::Order;
use http::header::{
    HeaderValue, ACCEPT_ENCODING, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_TYPE, ETAG,
};

/// The hooks a compressing hook must run after, when they are installed.
///
/// Instance names, so every hook of each instance counts. Each one reads
/// or rewrites the identity body.
const RUNS_AFTER: [&str; 4] = ["conditional_get", "route_cache", "gtag", "beacon"];

/// What [`Compression`] compresses, and how hard.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "config",
    derive(serde::Serialize, serde::Deserialize),
    serde(default, deny_unknown_fields)
)]
pub struct CompressionConfig {
    /// The smallest body that is compressed, in bytes. A gzip frame costs
    /// about 20 bytes, and a body under one TCP packet gains nothing on
    /// the wire.
    pub min_bytes: usize,
    /// Offer `gzip`. Every browser decodes it.
    pub gzip: bool,
    /// Offer `br`. Every current browser decodes it, over HTTPS only;
    /// a browser does not ask for it over plain HTTP.
    pub brotli: bool,
    /// The gzip level, 1 (fast) to 9 (small). 6 is zlib's default.
    pub gzip_level: u32,
    /// The brotli quality, 0 (fast) to 11 (small). 11 is for files
    /// compressed once at build time. A body compressed per request
    /// wants 4 or 5: near gzip's speed, and smaller.
    pub brotli_quality: u32,
    /// The media types that are compressed, without parameters. `text/*`
    /// takes every `text/` type. Matched without case.
    pub types: Vec<String>,
    /// More hooks whose response half runs before this one, when they are
    /// installed: an application hook that rewrites the body. An instance
    /// name or `instance.hook`. The four this workspace knows are always
    /// in the list.
    pub runs_after: Vec<String>,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            min_bytes: 1024,
            gzip: true,
            brotli: true,
            gzip_level: 6,
            brotli_quality: 4,
            types: [
                "text/*",
                "application/json",
                "application/javascript",
                "application/xml",
                "application/rss+xml",
                "application/atom+xml",
                "application/manifest+json",
                "image/svg+xml",
            ]
            .map(str::to_string)
            .to_vec(),
            runs_after: Vec::new(),
        }
    }
}

impl CompressionConfig {
    /// Checks the levels and the list. Each error names the setting and
    /// carries a `help:` line.
    pub fn check(&self) -> Result<(), String> {
        if !(1..=9).contains(&self.gzip_level) {
            return Err(format!(
                "`gzip_level` is {}, and gzip has levels 1 to 9. help: use 6, zlib's default",
                self.gzip_level
            ));
        }
        if self.brotli_quality > 11 {
            return Err(format!(
                "`brotli_quality` is {}, and brotli has qualities 0 to 11. help: use 4 for a \
                 body compressed per request",
                self.brotli_quality
            ));
        }
        if !self.gzip && !self.brotli {
            return Err(
                "`gzip` and `brotli` are both off, so the hook compresses nothing. \
                 help: turn one on, or remove the `Compression` registration"
                    .to_string(),
            );
        }
        for kind in &self.types {
            if kind
                .split_once('/')
                .is_none_or(|(a, b)| a.is_empty() || b.is_empty())
            {
                return Err(format!(
                    "`types` holds `{kind}`, which is not a media type. help: write \
                     `type/subtype`, as in `text/html`, or `text/*`"
                ));
            }
        }
        Ok(())
    }
}

/// gzip and brotli on buffered responses. Register it with
/// `plugin_hooking`.
pub struct Compression {
    config: CompressionConfig,
}

impl Plugin for Compression {
    type Config = CompressionConfig;
    type Provides = NoContext;
    /// Also the configuration section: `[compression]`.
    const NAME: &'static str = "compression";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: Self::Config) -> Self {
        Self { config }
    }
}

impl HookSource for Compression {
    type Families = OnResponse;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        self.config
            .check()
            .map_err(|why| PluginError::new(format!("compression: {why}")))?;
        let mut order = Order::new();
        for id in RUNS_AFTER
            .iter()
            .map(|id| id.to_string())
            .chain(self.config.runs_after.iter().cloned())
        {
            order = order.before_if_present(id);
        }
        registrar.response(
            "encode",
            order,
            Encode {
                config: self.config.clone(),
            },
        );
        Ok(())
    }
}

/// An encoding this hook writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Coding {
    Brotli,
    Gzip,
}

impl Coding {
    fn token(self) -> &'static str {
        match self {
            Self::Brotli => "br",
            Self::Gzip => "gzip",
        }
    }
}

struct Encode {
    config: CompressionConfig,
}

impl Encode {
    /// Whether this response could be compressed for some client. The
    /// answer does not depend on `Accept-Encoding`, so it also decides
    /// `Vary`.
    fn eligible(&self, head: &ResponseHead, len: usize) -> bool {
        let status = head.status.as_u16();
        if status < 200 || matches!(status, 204 | 205 | 206 | 304) {
            return false;
        }
        if head.headers.contains_key(CONTENT_ENCODING) {
            return false;
        }
        if len < self.config.min_bytes {
            return false;
        }
        let no_transform = head.headers.get_all(CACHE_CONTROL).iter().any(|value| {
            value.to_str().is_ok_and(|v| {
                v.split(',')
                    .any(|d| d.trim().eq_ignore_ascii_case("no-transform"))
            })
        });
        if no_transform {
            return false;
        }
        let Some(kind) = crate::text(&head.headers, CONTENT_TYPE) else {
            return false;
        };
        let kind = kind.split(';').next().unwrap_or(kind).trim();
        // An event stream sent whole is still an event stream: a client
        // reads it as it arrives, and a compressed one arrives at the end.
        if kind.eq_ignore_ascii_case("text/event-stream") {
            return false;
        }
        self.config
            .types
            .iter()
            .any(|allowed| media_matches(allowed, kind))
    }

    /// The coding this client asked for, among the ones this hook offers.
    fn chosen(&self, request: &Request) -> Option<Coding> {
        let accept = request
            .headers()
            .get_all(ACCEPT_ENCODING)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .collect::<Vec<_>>()
            .join(",");
        let mut offered = Vec::new();
        // Brotli first: on a tie of `q`, the smaller body wins.
        if self.config.brotli {
            offered.push(Coding::Brotli);
        }
        if self.config.gzip {
            offered.push(Coding::Gzip);
        }
        negotiate(&accept, &offered)
    }

    fn compress(&self, coding: Coding, body: &[u8]) -> Option<Vec<u8>> {
        match coding {
            Coding::Gzip => {
                let mut encoder = flate2::write::GzEncoder::new(
                    Vec::with_capacity(body.len() / 4),
                    flate2::Compression::new(self.config.gzip_level),
                );
                encoder.write_all(body).ok()?;
                encoder.finish().ok()
            }
            Coding::Brotli => {
                let params = brotli::enc::BrotliEncoderParams {
                    quality: self.config.brotli_quality as i32,
                    lgwin: 22,
                    size_hint: body.len(),
                    ..Default::default()
                };
                let mut out = Vec::with_capacity(body.len() / 4);
                brotli::BrotliCompress(&mut &body[..], &mut out, &params).ok()?;
                Some(out)
            }
        }
    }
}

impl ResponseHook for Encode {
    fn on_headers(
        &self,
        _request: &Request,
        _head: &mut ResponseHead,
        body: BodyShape<'_>,
        _state: &mut RequestState,
    ) -> Result<(), PluginError> {
        match body {
            // The decision needs the bytes; it is made in `on_body`.
            BodyShape::Buffered { .. } => Ok(()),
            // Never compressed, and nothing announced. The module header
            // says why: an SSE event must not wait for a full window.
            BodyShape::Streaming(_) => Ok(()),
        }
    }

    fn on_body(
        &self,
        request: &Request,
        head: &mut ResponseHead,
        body: &mut Vec<u8>,
        _state: &mut RequestState,
    ) -> Result<(), PluginError> {
        if !self.eligible(head, body.len()) {
            return Ok(());
        }
        crate::add_vary(&mut head.headers, "Accept-Encoding");
        let Some(coding) = self.chosen(request) else {
            return Ok(());
        };
        let Some(compressed) = self.compress(coding, body) else {
            return Ok(());
        };
        if compressed.len() >= body.len() {
            return Ok(());
        }
        *body = compressed;
        head.headers
            .insert(CONTENT_ENCODING, HeaderValue::from_static(coding.token()));
        weaken_etag(head);
        Ok(())
    }
}

/// A strong `ETag` becomes weak, because the bytes it named are gone.
fn weaken_etag(head: &mut ResponseHead) {
    let Some(tag) = crate::text(&head.headers, ETAG) else {
        return;
    };
    if tag.starts_with("W/") {
        return;
    }
    if let Ok(weak) = HeaderValue::from_str(&format!("W/{tag}")) {
        head.headers.insert(ETAG, weak);
    }
}

/// Whether the media type `kind` is on the list entry `allowed`, which may
/// be `type/*`.
fn media_matches(allowed: &str, kind: &str) -> bool {
    match allowed.strip_suffix("/*") {
        Some(major) => kind
            .split_once('/')
            .is_some_and(|(m, _)| m.eq_ignore_ascii_case(major)),
        None => allowed.eq_ignore_ascii_case(kind),
    }
}

/// The best coding in `offered` for an `Accept-Encoding` value.
///
/// RFC 9110 §12.5.3: each element is a coding and an optional `q`. `*`
/// stands for every coding not named. `q=0` refuses. The highest `q` wins;
/// on a tie, the earlier entry in `offered`. `x-gzip` is `gzip`.
fn negotiate(accept: &str, offered: &[Coding]) -> Option<Coding> {
    let mut named: Vec<(String, f32)> = Vec::new();
    for element in accept.split(',') {
        let mut parts = element.split(';');
        let coding = parts.next().unwrap_or("").trim().to_ascii_lowercase();
        if coding.is_empty() {
            continue;
        }
        let mut q = 1.0_f32;
        for param in parts {
            if let Some((key, value)) = param.split_once('=') {
                if key.trim().eq_ignore_ascii_case("q") {
                    q = value.trim().parse().unwrap_or(0.0);
                }
            }
        }
        let coding = if coding == "x-gzip" {
            "gzip".to_string()
        } else {
            coding
        };
        named.push((coding, q));
    }
    let weight = |coding: Coding| -> f32 {
        if let Some((_, q)) = named.iter().find(|(c, _)| c == coding.token()) {
            return *q;
        }
        named
            .iter()
            .find(|(c, _)| c == "*")
            .map_or(0.0, |(_, q)| *q)
    };
    let mut best: Option<(Coding, f32)> = None;
    for &coding in offered {
        let q = weight(coding);
        if q > 0.0 && best.is_none_or(|(_, b)| q > b) {
            best = Some((coding, q));
        }
    }
    best.map(|(coding, _)| coding)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: [Coding; 2] = [Coding::Brotli, Coding::Gzip];

    #[test]
    fn a_browser_header_picks_brotli() {
        assert_eq!(
            negotiate("gzip, deflate, br, zstd", &BOTH),
            Some(Coding::Brotli)
        );
    }

    #[test]
    fn q_values_decide_and_zero_refuses() {
        assert_eq!(negotiate("br;q=0.5, gzip;q=0.9", &BOTH), Some(Coding::Gzip));
        assert_eq!(negotiate("br;q=0, gzip", &BOTH), Some(Coding::Gzip));
        assert_eq!(negotiate("br;q=0, gzip;q=0", &BOTH), None);
        assert_eq!(negotiate("identity", &BOTH), None);
        assert_eq!(negotiate("", &BOTH), None);
    }

    #[test]
    fn a_star_stands_for_every_coding_not_named() {
        assert_eq!(negotiate("*", &BOTH), Some(Coding::Brotli));
        assert_eq!(negotiate("br;q=0, *", &BOTH), Some(Coding::Gzip));
        assert_eq!(negotiate("*;q=0", &BOTH), None);
    }

    #[test]
    fn only_offered_codings_are_chosen() {
        assert_eq!(negotiate("br", &[Coding::Gzip]), None);
        assert_eq!(negotiate("x-gzip", &[Coding::Gzip]), Some(Coding::Gzip));
        assert_eq!(negotiate("GZIP", &BOTH), Some(Coding::Gzip));
    }

    #[test]
    fn a_wildcard_type_takes_its_whole_family() {
        assert!(media_matches("text/*", "text/html"));
        assert!(media_matches("text/*", "TEXT/CSS"));
        assert!(!media_matches("text/*", "image/png"));
        assert!(media_matches("application/json", "Application/JSON"));
        assert!(!media_matches("application/json", "application/jsonx"));
    }

    #[test]
    fn a_bad_setting_is_refused_with_a_help_line() {
        let bad = [
            CompressionConfig {
                gzip_level: 0,
                ..Default::default()
            },
            CompressionConfig {
                brotli_quality: 12,
                ..Default::default()
            },
            CompressionConfig {
                gzip: false,
                brotli: false,
                ..Default::default()
            },
            CompressionConfig {
                types: vec!["html".to_string()],
                ..Default::default()
            },
        ];
        for config in bad {
            let why = config.check().unwrap_err();
            assert!(why.contains("help:"), "{why}");
        }
        CompressionConfig::default()
            .check()
            .expect("the default is valid");
    }
}
