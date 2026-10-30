//! The middleware every application rewrites.
//!
//! Six hooks, taken from the Gorilla toolkit because that set has been
//! argued over by a lot of people for a long time, and because each one is
//! a thing somebody writes badly in week two: trusting `X-Forwarded-For`
//! from anywhere, answering a preflight by hand, spelling a canonical-host
//! redirect as a 302 when it wanted a 301.
//!
//! | Hook | Gorilla | Answers |
//! |---|---|---|
//! | [`ProxyHeaders`] | `ProxyHeaders` | who the client is, behind a proxy |
//! | [`MethodOverride`] | `HTTPMethodOverrideHandler` | a `DELETE` sent from an HTML form |
//! | [`CanonicalHost`] | `CanonicalHost` | one host, so cookies and caches agree |
//! | [`ContentTypes`] | `ContentTypeHandler` | 415 before a handler parses nonsense |
//! | [`Cors`] | `CORS` | a browser on another origin |
//! | [`Csrf`] | `gorilla/csrf` | a form post this application handed out |
//!
//! And one that is not Gorilla's: [`DpSwap`], with the rest of [`dp`], is
//! the server half of dp-swap's mutating verbs. It reads the `DP-*`
//! request headers, sets `Vary`, and gives a handler typed response
//! headers and out-of-band fragments.
//!
//! # The one that is on by default
//!
//! Five of the six encode a fact about somebody else's network: which
//! proxy to trust, which host is canonical, which origins may call. The
//! scaffold writes those commented out, at the place somebody who knows
//! the fact uncomments one.
//!
//! [`Csrf`] is not one of those. It encodes a fact about browsers -- any
//! page anywhere can post a form here, and the cookies ride along -- and
//! that fact is the same in every deployment. "Unprotected until you opt
//! in" is the wrong default for it, so it is a choice of its own that
//! arrives ticked, with the configuration a generated project needs
//! already written. `scaffold::Csrf` says what is still in the way of the
//! line being live rather than commented.
//!
//! # Why hooks and not layers
//!
//! `brocanth::routing_middleware` is a pattern-scoped chain that looks like
//! the place for this. It is not: nothing mounts a layer, and a layer that
//! awaits does not compile, for reasons its own header sets out in full.
//! `docs/middleware.md` §1 has the comparison. Everything here is a
//! [`RequestHook`] or a [`ResponseHook`].
//!
//! # Each one is its own plugin
//!
//! Not one `MiddlewarePlugin` with six switches. An application that wants
//! a canonical host and nothing else registers one plugin, and
//! `brocanth routes` names that plugin rather than a flag inside another
//! one. The cost is six `Plugin` impls, which is fifteen lines each.
//!
//! # What is not here
//!
//! Compression, because gzip is a dependency and this crate has none yet;
//! the three logging handlers, because `logging` already writes a line and
//! Common Log Format is a format choice on it rather than a second hook;
//! and recovery, because catching a panic around the application's handler
//! is a change to the core rather than a hook. `docs/middleware.md` §2 says
//! what each of those costs.
//!
//! [`RequestHook`]: brocanth::plugin_hooks::RequestHook
//! [`ResponseHook`]: brocanth::plugin_hooks::ResponseHook

mod canonical_host;
mod content_types;
mod cors;
mod csrf;
pub mod dp;
mod method_override;
mod proxy_headers;

#[cfg(feature = "scaffold")]
pub mod scaffold;

pub use canonical_host::{CanonicalHost, CanonicalHostConfig};
pub use content_types::{ContentTypes, ContentTypesConfig};
pub use cors::{Cors, CorsConfig, Origins};
pub use csrf::{Csrf, CsrfConfig, CsrfToken};
pub use dp::{Directive, DpRequest, DpSwap, Oob};
pub use method_override::{MethodOverride, MethodOverrideConfig};
pub use proxy_headers::{ClientInfo, ProxyHeaders, ProxyHeadersConfig};

/// Reads one header as UTF-8, or `None` when it is absent or not text.
///
/// A header value is bytes. Every header this crate reads is a token or a
/// host name, so one that is not UTF-8 is malformed rather than exotic, and
/// treating it as absent is the same answer with less code than refusing it.
fn text(headers: &http::HeaderMap, name: http::header::HeaderName) -> Option<&str> {
    headers.get(name)?.to_str().ok()
}

/// The first comma-separated element of a header, trimmed.
fn first(value: &str) -> &str {
    value.split(',').next().unwrap_or(value).trim()
}

/// One field out of an `application/x-www-form-urlencoded` body.
///
/// `None` for a body of any other media type, which is the answer a
/// `multipart/form-data` upload gets: reading a field out of one means
/// parsing an upload, in a hook, on every request. Two hooks read a form
/// field -- the method override and the CSRF check -- and both refuse
/// multipart for that reason, so the rule is written once here.
fn form_field(request: &brocanth::plugin_hooks::Request, field: &str) -> Option<String> {
    let kind = text(request.headers(), http::header::CONTENT_TYPE)?;
    // `application/x-www-form-urlencoded; charset=utf-8` is the same type.
    if !kind
        .split(';')
        .next()
        .unwrap_or(kind)
        .trim()
        .eq_ignore_ascii_case("application/x-www-form-urlencoded")
    {
        return None;
    }
    let body = std::str::from_utf8(request.body()).ok()?;
    body.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (decode(key) == field).then(|| decode(value))
    })
}

/// Percent and plus decoding, enough for a field name and a short value.
///
/// A full form decoder belongs in the crate that fills a struct from a
/// body. This reads a field name and a token beside it, and a dependency
/// for that would be the whole cost of this crate.
fn decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(raw.len());
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'+' => {
                out.push(' ');
                at += 1;
            }
            b'%' if at + 2 < bytes.len() => {
                let hex = &raw[at + 1..at + 3];
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte as char);
                        at += 3;
                    }
                    Err(_) => {
                        out.push('%');
                        at += 1;
                    }
                }
            }
            _ => {
                let ch = raw[at..].chars().next().expect("at a boundary");
                out.push(ch);
                at += ch.len_utf8();
            }
        }
    }
    out
}
