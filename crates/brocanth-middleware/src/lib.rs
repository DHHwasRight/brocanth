//! The middleware every application rewrites.
//!
//! Five hooks, taken from the Gorilla toolkit's `handlers` package because
//! that set has been argued over by a lot of people for a long time, and
//! because each one is a thing somebody writes badly in week two: trusting
//! `X-Forwarded-For` from anywhere, answering a preflight by hand, spelling
//! a canonical-host redirect as a 302 when it wanted a 301.
//!
//! | Hook | Gorilla | Answers |
//! |---|---|---|
//! | [`ProxyHeaders`] | `ProxyHeaders` | who the client is, behind a proxy |
//! | [`MethodOverride`] | `HTTPMethodOverrideHandler` | a `DELETE` sent from an HTML form |
//! | [`CanonicalHost`] | `CanonicalHost` | one host, so cookies and caches agree |
//! | [`ContentTypes`] | `ContentTypeHandler` | 415 before a handler parses nonsense |
//! | [`Cors`] | `CORS` | a browser on another origin |
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
//! Not one `MiddlewarePlugin` with five switches. An application that wants
//! a canonical host and nothing else registers one plugin, and
//! `brocanth routes` names that plugin rather than a flag inside another
//! one. The cost is five `Plugin` impls, which is fifteen lines each.
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
mod method_override;
mod proxy_headers;

#[cfg(feature = "scaffold")]
pub mod scaffold;

pub use canonical_host::{CanonicalHost, CanonicalHostConfig};
pub use content_types::{ContentTypes, ContentTypesConfig};
pub use cors::{Cors, CorsConfig, Origins};
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
