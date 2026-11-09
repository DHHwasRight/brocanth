//! A dynamic `ETag`, and `304 Not Modified` when the client already has the
//! page.
//!
//! [`ConditionalGet`] is one response hook, Django's
//! `ConditionalGetMiddleware` and Rails' `Rack::ConditionalGet` with
//! `Rack::ETag`. For a `GET` or `HEAD` that answered `200` with a buffered
//! body:
//!
//! 1. When the handler set no `ETag`, the hook sets one: a weak tag over
//!    the body's SHA-256.
//! 2. When the request has `If-None-Match`, the hook compares it with the
//!    `ETag` (weak comparison, RFC 9110 §13.1.2). A match is a `304`.
//! 3. When the request has no `If-None-Match` but has `If-Modified-Since`,
//!    and the handler set `Last-Modified`, a `Last-Modified` at or before
//!    that date is a `304`.
//!
//! A `304` has no body. The hook keeps every header but the four that
//! describe the body that is not sent: `Content-Type`, `Content-Length`,
//! `Content-Encoding` and `Content-Language`.
//!
//! The handler still runs. This hook saves the transfer, not the render.
//! A page that can answer cheaper than it renders compares the tag itself.
//! A dp-swap fragment that did not change costs a header round trip
//! instead of a body.
//!
//! # Why a weak tag
//!
//! A strong tag names exact bytes, and compression changes the bytes. The
//! tag here is computed over the identity body, before
//! [`Compression`](crate::Compression) runs, and a weak tag stays true
//! for the gzip and the brotli copy of the same page. Weak comparison is
//! what `If-None-Match` uses anyway, so nothing is lost for a `GET`.
//!
//! # What it leaves alone
//!
//! | Response | Why |
//! |---|---|
//! | a stream | there is no body to hash, and none will be in hand |
//! | a verb other than `GET` and `HEAD` | `If-None-Match` on a write is a precondition, not a cache check |
//! | a status other than `200` | an error or a redirect is not a page to revalidate |
//! | `Cache-Control: no-store` | no tag: the client must not keep the page |
//!
//! `If-Match` and `If-Unmodified-Since` are not read: they guard a write,
//! and a write handler checks them against the row it is about to change.
//!
//! # Order
//!
//! The hook runs after the route cache stores the page and before
//! compression. Its order says so with `route_cache` and `compression`,
//! when they are installed. A cache hit then gets a tag and a `304` like a
//! fresh page.

use base64::Engine as _;
use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{
    BodyShape, HookRegistrar, HookSource, OnResponse, Request, RequestState, ResponseHead,
    ResponseHook,
};
use brocanth::plugin_order::Order;
use http::header::{
    HeaderValue, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_LANGUAGE, CONTENT_LENGTH, CONTENT_TYPE,
    ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED,
};
use http::{Method, StatusCode};
use sha2::{Digest, Sha256};

/// What [`ConditionalGet`] does.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "config",
    derive(serde::Serialize, serde::Deserialize),
    serde(default, deny_unknown_fields)
)]
pub struct ConditionalGetConfig {
    /// Set a weak `ETag` over the body when the handler set none.
    pub etag: bool,
    /// Answer `If-Modified-Since` from a `Last-Modified` the handler set.
    pub last_modified: bool,
}

impl Default for ConditionalGetConfig {
    fn default() -> Self {
        Self {
            etag: true,
            last_modified: true,
        }
    }
}

/// `ETag` and `304`. Register it with `plugin_hooking`.
pub struct ConditionalGet {
    config: ConditionalGetConfig,
}

impl Plugin for ConditionalGet {
    type Config = ConditionalGetConfig;
    type Provides = NoContext;
    /// Also the configuration section: `[conditional_get]`.
    const NAME: &'static str = "conditional_get";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: Self::Config) -> Self {
        Self { config }
    }
}

impl HookSource for ConditionalGet {
    type Families = OnResponse;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        // Declared after compression and before the route cache, so on the
        // way out it runs after the cache stored the page and before the
        // body is compressed.
        let order = Order::new()
            .after_if_present("compression")
            .before_if_present("route_cache");
        registrar.response(
            "check",
            order,
            Check {
                config: self.config.clone(),
            },
        );
        Ok(())
    }
}

struct Check {
    config: ConditionalGetConfig,
}

impl ResponseHook for Check {
    fn on_headers(
        &self,
        _request: &Request,
        _head: &mut ResponseHead,
        body: BodyShape<'_>,
        _state: &mut RequestState,
    ) -> Result<(), PluginError> {
        match body {
            // The tag needs the bytes; they arrive in `on_body`.
            BodyShape::Buffered { .. } => Ok(()),
            // Unavailable: there is no body to hash, and there never will
            // be one in hand.
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
        if !matches!(*request.method(), Method::GET | Method::HEAD) || head.status != StatusCode::OK
        {
            return Ok(());
        }
        if has_directive(head, "no-store") {
            return Ok(());
        }
        if self.config.etag && !head.headers.contains_key(ETAG) {
            if let Ok(tag) = HeaderValue::from_str(&etag_for(body)) {
                head.headers.insert(ETAG, tag);
            }
        }
        if self.fresh(request, head) {
            not_modified(head, body);
        }
        Ok(())
    }
}

impl Check {
    /// Whether the client's copy is current. RFC 9110 §13.2.2: when
    /// `If-None-Match` is present, `If-Modified-Since` is ignored.
    fn fresh(&self, request: &Request, head: &ResponseHead) -> bool {
        let headers = request.headers();
        if headers.contains_key(IF_NONE_MATCH) {
            let Some(tag) = crate::text(&head.headers, ETAG) else {
                return false;
            };
            return headers
                .get_all(IF_NONE_MATCH)
                .iter()
                .filter_map(|v| v.to_str().ok())
                .any(|list| none_match_hits(list, tag));
        }
        if !self.config.last_modified {
            return false;
        }
        let (Some(since), Some(modified)) = (
            crate::text(headers, IF_MODIFIED_SINCE).and_then(http_date),
            crate::text(&head.headers, LAST_MODIFIED).and_then(http_date),
        ) else {
            return false;
        };
        modified <= since
    }
}

/// Whether `Cache-Control` on the response has `directive`.
fn has_directive(head: &ResponseHead, directive: &str) -> bool {
    head.headers.get_all(CACHE_CONTROL).iter().any(|value| {
        value.to_str().is_ok_and(|v| {
            v.split(',').any(|d| {
                d.split('=')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .eq_ignore_ascii_case(directive)
            })
        })
    })
}

/// The response becomes a `304`: no body, and none of the headers that
/// describe one.
fn not_modified(head: &mut ResponseHead, body: &mut Vec<u8>) {
    head.status = StatusCode::NOT_MODIFIED;
    body.clear();
    for name in [
        CONTENT_TYPE,
        CONTENT_LENGTH,
        CONTENT_ENCODING,
        CONTENT_LANGUAGE,
    ] {
        head.headers.remove(name);
    }
}

/// A weak tag over the body: 128 bits of its SHA-256, in URL-safe base64.
///
/// 128 bits, because a tag only has to tell two versions of one page apart,
/// and 22 characters is shorter on every request than 43.
pub fn etag_for(body: &[u8]) -> String {
    let digest = Sha256::digest(body);
    let short = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&digest[..16]);
    format!("W/\"{short}\"")
}

/// Whether an `If-None-Match` list names `tag`, by weak comparison.
///
/// `*` matches any current page. An entity tag is `[W/]"…"`, and the quoted
/// part may hold a comma, so the list is read tag by tag and not split.
fn none_match_hits(list: &str, tag: &str) -> bool {
    let wanted = opaque(tag);
    if list.trim() == "*" {
        return true;
    }
    let mut rest = list;
    loop {
        rest = rest.trim_start_matches(|c: char| c == ',' || c.is_ascii_whitespace());
        if rest.is_empty() {
            return false;
        }
        let tail = rest.strip_prefix("W/").unwrap_or(rest);
        let Some(inner) = tail.strip_prefix('"') else {
            // Not an entity tag: stop rather than guess where the next one
            // starts. A malformed header is a miss, never a 304.
            return false;
        };
        let Some(end) = inner.find('"') else {
            return false;
        };
        if Some(&inner[..end]) == wanted {
            return true;
        }
        rest = &inner[end + 1..];
    }
}

/// The quoted part of an entity tag, without `W/` and the quotes.
fn opaque(tag: &str) -> Option<&str> {
    let tag = tag.trim();
    let tag = tag.strip_prefix("W/").unwrap_or(tag);
    tag.strip_prefix('"')?.strip_suffix('"')
}

/// Seconds since 1970 from an IMF-fixdate: `Sun, 06 Nov 1994 08:49:37 GMT`.
///
/// RFC 9110 §5.6.7 makes this the one format a sender writes. The two
/// obsolete formats are read as no date at all, so the request is answered
/// in full: a date this hook cannot read is never a `304`.
pub fn http_date(text: &str) -> Option<i64> {
    let (_, rest) = text.trim().split_once(", ")?;
    let mut parts = rest.split(' ');
    let day: i64 = parts.next()?.parse().ok()?;
    let month = match parts.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year: i64 = parts.next()?.parse().ok()?;
    let mut clock = parts.next()?.split(':');
    let hour: i64 = clock.next()?.parse().ok()?;
    let minute: i64 = clock.next()?.parse().ok()?;
    let second: i64 = clock.next()?.parse().ok()?;
    if parts.next()? != "GMT" || parts.next().is_some() || clock.next().is_some() {
        return None;
    }
    if !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// Days from 1970-01-01 to a date in the proleptic Gregorian calendar.
/// Howard Hinnant's `days_from_civil`.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rfc_example_date_is_read() {
        assert_eq!(
            http_date("Sun, 06 Nov 1994 08:49:37 GMT"),
            Some(784_111_777)
        );
        assert_eq!(http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(
            http_date("Tue, 29 Feb 2028 12:00:00 GMT"),
            Some(1_835_438_400)
        );
    }

    #[test]
    fn an_obsolete_or_broken_date_is_no_date() {
        for text in [
            "Sunday, 06-Nov-94 08:49:37 GMT",
            "Sun Nov  6 08:49:37 1994",
            "Sun, 06 Nov 1994 08:49:37 UTC",
            "Sun, 06 Nov 1994 25:49:37 GMT",
            "Sun, 06 Nov 1994 08:49 GMT",
            "",
        ] {
            assert_eq!(http_date(text), None, "{text}");
        }
    }

    #[test]
    fn if_none_match_compares_weakly_and_reads_a_list() {
        assert!(none_match_hits("\"abc\"", "W/\"abc\""));
        assert!(none_match_hits("W/\"abc\"", "\"abc\""));
        assert!(none_match_hits("\"x\", W/\"abc\"", "W/\"abc\""));
        assert!(none_match_hits("\"a,b\", \"abc\"", "\"abc\""));
        assert!(none_match_hits("*", "\"abc\""));
        assert!(!none_match_hits("\"abcd\"", "\"abc\""));
        assert!(!none_match_hits("abc", "\"abc\""));
        assert!(!none_match_hits("\"abc", "\"abc\""));
    }

    #[test]
    fn the_tag_is_weak_short_and_stable() {
        let tag = etag_for(b"<p>hi</p>");
        assert!(tag.starts_with("W/\""), "{tag}");
        assert_eq!(tag.len(), 2 + 1 + 22 + 1);
        assert_eq!(tag, etag_for(b"<p>hi</p>"));
        assert_ne!(tag, etag_for(b"<p>ho</p>"));
    }
}
