//! The collector: page views a static site sends, counted like the ones
//! this server answers.
//!
//! A static host never runs the hook, so a page exported with the `beacon`
//! module carries a few lines of script that tell this server "a page was
//! read": `navigator.sendBeacon`, and a 1×1 image when the browser has no
//! beacon. The collector is a request hook on one path, `/_a/v` by default.
//!
//! The same rules as the hook apply. The visitor is hashed from the beacon
//! request's own address and user agent, which are the visitor's, because
//! the browser sends the beacon. The page path comes from the payload and
//! is reduced by the same functions; the referrer keeps its origin.
//!
//! **Abuse.** Anybody can send a request to a public URL, so:
//!
//! - only an origin listed in `collector_origins` is counted, read from
//!   `Origin`, or from the origin of `Referer` when there is none (an image
//!   request sends no `Origin`);
//! - a query or a body over [`MAX_PAYLOAD`] bytes is ignored;
//! - bots are dropped, as in the hook;
//! - the path is fixed, so a rate-limit rule can name it.
//!
//! **The answer never varies.** Counted or not, the collector answers 204,
//! or a 1×1 GIF to a GET, with `Cache-Control: no-store`. Never a 5xx, never
//! a body that says why: a beacon cannot read it, and a prober learns
//! nothing from it.

use brocanth::plugin_api::PluginError;
use brocanth::plugin_body::Body;
use brocanth::plugin_hooks::{Flow, Request, RequestHook, RequestState, Response};

use crate::config::AnalyticsConfig;
use crate::plugin::{header, visit, Page};
use crate::recorder::Recorder;

/// The largest query or body the collector reads.
pub const MAX_PAYLOAD: usize = 2_048;

/// A transparent 1×1 GIF, 43 bytes.
const PIXEL: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff!\xf9\x04\x01\x00\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;";

/// The collector hook.
pub(crate) struct Collect {
    config: AnalyticsConfig,
    recorder: Recorder,
    origins: Vec<String>,
}

impl Collect {
    pub(crate) fn new(config: AnalyticsConfig, recorder: Recorder) -> Self {
        let origins = config
            .collector_origins
            .iter()
            .map(|o| o.trim().trim_end_matches('/').to_ascii_lowercase())
            .collect();
        Self {
            config,
            recorder,
            origins,
        }
    }

    /// Counts the beacon if it passes every check. Silent either way.
    fn take(&self, request: &Request) {
        let method = request.method();
        let payload = if method == http::Method::GET {
            request.uri().query().unwrap_or("").as_bytes()
        } else if method == http::Method::POST {
            request.body().as_slice()
        } else {
            return;
        };
        if payload.len() > MAX_PAYLOAD {
            return;
        }
        let Some(origin) = self.origin(request) else {
            return;
        };
        let Ok(payload) = std::str::from_utf8(payload) else {
            return;
        };
        let mut page = None;
        let mut referer = String::new();
        for pair in payload.split('&') {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            match name {
                "p" => page = decode(value),
                "r" => referer = decode(value).unwrap_or_default(),
                _ => {}
            }
        }
        let Some(page) = page.filter(|p| p.starts_with('/')) else {
            return;
        };
        let (path, query) = page.split_once('?').unwrap_or((&page, ""));
        // The static site is a site of its own on the dashboard: its rows
        // carry its host, and its visitors are hashed under it.
        let Some(host) = origin.split_once("://").map(|(_, host)| host) else {
            return;
        };
        let page = Page {
            site: host,
            path,
            query,
            referer: &referer,
            host: Some(host),
        };
        if let Some(pending) = visit(&self.config, &self.recorder, request, page) {
            self.recorder.record(pending);
        }
    }

    /// The sending origin, when it is one this collector accepts.
    fn origin(&self, request: &Request) -> Option<String> {
        let origin = match header(request, "origin") {
            "" | "null" => {
                let referer = header(request, "referer");
                let (scheme, rest) = referer.split_once("://")?;
                let host = rest.split(['/', '?', '#']).next()?;
                format!("{scheme}://{host}")
            }
            origin => origin.to_string(),
        }
        .to_ascii_lowercase();
        self.origins.contains(&origin).then_some(origin)
    }
}

impl RequestHook for Collect {
    fn before_dispatch(
        &self,
        request: &mut Request,
        _state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        if request.uri().path() != self.config.collector_path {
            return Ok(Flow::Continue);
        }
        self.take(request);
        Ok(Flow::respond(answer(request.method() == http::Method::GET)))
    }
}

/// 204, or the pixel for an image request.
fn answer(pixel: bool) -> Response {
    let builder = http::Response::builder()
        .header("cache-control", "no-store")
        .header("cross-origin-resource-policy", "cross-origin")
        .header("x-content-type-options", "nosniff");
    let built = if pixel {
        builder
            .status(200)
            .header("content-type", "image/gif")
            .body(Body::from(PIXEL.to_vec()))
    } else {
        builder.status(204).body(Body::from(Vec::new()))
    };
    built.unwrap_or_else(|_| http::Response::new(Body::from(Vec::new())))
}

/// Percent-decodes a form value, `+` as a space. `None` for invalid UTF-8.
fn decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' => {
                let hex = bytes
                    .get(i + 1..i + 3)
                    .and_then(|h| std::str::from_utf8(h).ok());
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            byte => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_form_value_is_percent_decoded() {
        assert_eq!(decode("%2Fdocs%3Fx%3D1").as_deref(), Some("/docs?x=1"));
        assert_eq!(decode("a+b").as_deref(), Some("a b"));
        assert_eq!(decode("100%").as_deref(), Some("100%"));
        assert_eq!(decode("%zz").as_deref(), Some("%zz"));
        assert_eq!(decode("%ff"), None);
    }

    #[test]
    fn the_pixel_is_a_gif() {
        assert!(PIXEL.starts_with(b"GIF89a"));
        assert_eq!(PIXEL.len(), 43);
    }
}
