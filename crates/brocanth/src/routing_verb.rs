//! The seven verbs, and the `Allow` set that makes 405 useful.
//!
//! The Crystal prototype registered every page with `@router.get(route)`.
//! There was no other call site, so a POST to any route in a framework
//! whose stated purpose is HTMX answered 404. Here a verb is a closed enum
//! and a route carries one slot per verb, so "which verbs does this path
//! answer" is a fact the router holds rather than a thing the author
//! remembers.
//!
//! # File convention
//!
//! The prototype already dispatched `handle_json` by content type. The verb
//! convention extends the same shape: a page answers a verb when it defines
//! the matching function.
//!
//! | Function in the page file | Verb |
//! | ------------------------- | ---- |
//! | `handle`                  | GET |
//! | `handle_head`             | HEAD |
//! | `handle_post`             | POST |
//! | `handle_put`              | PUT |
//! | `handle_patch`            | PATCH |
//! | `handle_delete`           | DELETE |
//! | `handle_options`          | OPTIONS |
//!
//! `handle` keeps its bare name because GET is what a page is for. Content
//! negotiation stays orthogonal: `handle_json` is still chosen by `Accept`,
//! inside whichever verb matched.

use core::fmt;

/// An HTTP method brocanth routes on.
///
/// The list is closed. TRACE and CONNECT are a proxy's business, not an
/// application's, so a route cannot be registered for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Verb {
    /// `GET`
    Get,
    /// `HEAD`
    Head,
    /// `POST`
    Post,
    /// `PUT`
    Put,
    /// `PATCH`
    Patch,
    /// `DELETE`
    Delete,
    /// `OPTIONS`
    Options,
}

impl Verb {
    /// Every verb, in the order an `Allow` header lists them.
    pub const ALL: [Self; 7] = [
        Self::Get,
        Self::Head,
        Self::Post,
        Self::Put,
        Self::Patch,
        Self::Delete,
        Self::Options,
    ];

    /// Index into a per-route verb table. Matches the order of
    /// [`Verb::ALL`].
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Get => 0,
            Self::Head => 1,
            Self::Post => 2,
            Self::Put => 3,
            Self::Patch => 4,
            Self::Delete => 5,
            Self::Options => 6,
        }
    }

    /// The uppercase method name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Head => "HEAD",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
            Self::Options => "OPTIONS",
        }
    }

    /// The page function that declares this verb.
    #[must_use]
    pub const fn handler_fn(self) -> &'static str {
        match self {
            Self::Get => "handle",
            Self::Head => "handle_head",
            Self::Post => "handle_post",
            Self::Put => "handle_put",
            Self::Patch => "handle_patch",
            Self::Delete => "handle_delete",
            Self::Options => "handle_options",
        }
    }

    /// The verb a page function declares, or [`None`] for any other name.
    ///
    /// `handle_json` is a content-negotiation hook, not a verb, so it
    /// returns [`None`].
    #[must_use]
    pub fn from_handler_fn(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.handler_fn() == name)
    }

    /// Parse a method name. Case-sensitive, as HTTP requires.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.as_str() == name)
    }

    /// The matching [`http::Method`], or [`None`] for TRACE, CONNECT, and
    /// any extension method.
    #[must_use]
    pub fn from_http(method: &http::Method) -> Option<Self> {
        Self::from_name(method.as_str())
    }

    /// As an [`http::Method`].
    #[must_use]
    pub fn to_http(self) -> http::Method {
        match self {
            Self::Get => http::Method::GET,
            Self::Head => http::Method::HEAD,
            Self::Post => http::Method::POST,
            Self::Put => http::Method::PUT,
            Self::Patch => http::Method::PATCH,
            Self::Delete => http::Method::DELETE,
            Self::Options => http::Method::OPTIONS,
        }
    }
}

impl fmt::Display for Verb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<Verb> for http::Method {
    fn from(verb: Verb) -> Self {
        verb.to_http()
    }
}

/// The set of verbs a matched route answers.
///
/// This is the body of the `Allow` header that must accompany a 405. A 404
/// in its place hides the single most useful fact: the path is right and
/// the method is wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Allow {
    bits: u8,
}

impl Allow {
    /// An empty set.
    #[must_use]
    pub const fn empty() -> Self {
        Self { bits: 0 }
    }

    /// Add a verb.
    #[must_use]
    pub const fn with(self, verb: Verb) -> Self {
        Self {
            bits: self.bits | (1 << verb.index()),
        }
    }

    /// Whether the set contains `verb`.
    #[must_use]
    pub const fn contains(self, verb: Verb) -> bool {
        self.bits & (1 << verb.index()) != 0
    }

    /// Whether no verb is allowed.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.bits == 0
    }

    /// How many verbs are allowed.
    #[must_use]
    pub const fn len(self) -> usize {
        self.bits.count_ones() as usize
    }

    /// The verbs, in [`Verb::ALL`] order.
    pub fn iter(self) -> impl Iterator<Item = Verb> {
        Verb::ALL.into_iter().filter(move |v| self.contains(*v))
    }

    /// The header value, for example `GET, HEAD, POST`.
    ///
    /// # Panics
    ///
    /// Never: every verb name is a valid header value.
    #[must_use]
    pub fn header_value(self) -> http::HeaderValue {
        http::HeaderValue::from_str(&self.to_string())
            .expect("verb names contain only visible ASCII")
    }
}

impl FromIterator<Verb> for Allow {
    fn from_iter<I: IntoIterator<Item = Verb>>(iter: I) -> Self {
        iter.into_iter().fold(Self::empty(), Self::with)
    }
}

impl fmt::Display for Allow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for verb in self.iter() {
            if !first {
                f.write_str(", ")?;
            }
            f.write_str(verb.as_str())?;
            first = false;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_seven_verbs_are_present_and_distinct() {
        assert_eq!(Verb::ALL.len(), 7);
        let mut indices: Vec<usize> = Verb::ALL.iter().map(|v| v.index()).collect();
        indices.sort_unstable();
        assert_eq!(indices, [0, 1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn the_file_convention_maps_both_ways() {
        let cases = [
            (Verb::Get, "handle"),
            (Verb::Head, "handle_head"),
            (Verb::Post, "handle_post"),
            (Verb::Put, "handle_put"),
            (Verb::Patch, "handle_patch"),
            (Verb::Delete, "handle_delete"),
            (Verb::Options, "handle_options"),
        ];
        for (verb, name) in cases {
            assert_eq!(verb.handler_fn(), name);
            assert_eq!(Verb::from_handler_fn(name), Some(verb));
        }
    }

    #[test]
    fn content_negotiation_hooks_are_not_verbs() {
        assert_eq!(Verb::from_handler_fn("handle_json"), None);
        assert_eq!(Verb::from_handler_fn("render"), None);
        assert_eq!(Verb::from_handler_fn("handle_trace"), None);
    }

    #[test]
    fn http_methods_round_trip() {
        for verb in Verb::ALL {
            assert_eq!(Verb::from_http(&verb.to_http()), Some(verb));
        }
    }

    #[test]
    fn proxy_methods_are_not_routable() {
        assert_eq!(Verb::from_http(&http::Method::TRACE), None);
        assert_eq!(Verb::from_http(&http::Method::CONNECT), None);
    }

    #[test]
    fn method_names_are_case_sensitive() {
        assert_eq!(Verb::from_name("GET"), Some(Verb::Get));
        assert_eq!(Verb::from_name("get"), None);
    }

    #[test]
    fn allow_lists_verbs_in_canonical_order() {
        let allow: Allow = [Verb::Delete, Verb::Get, Verb::Post].into_iter().collect();
        assert_eq!(allow.to_string(), "GET, POST, DELETE");
        assert_eq!(allow.len(), 3);
        assert!(allow.contains(Verb::Post));
        assert!(!allow.contains(Verb::Put));
    }

    #[test]
    fn allow_becomes_a_header_value() {
        let allow: Allow = [Verb::Get, Verb::Head].into_iter().collect();
        assert_eq!(allow.header_value(), "GET, HEAD");
    }

    #[test]
    fn an_empty_allow_prints_nothing() {
        assert!(Allow::empty().is_empty());
        assert_eq!(Allow::empty().to_string(), "");
    }
}
