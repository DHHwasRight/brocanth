//! Which locale a request gets.
//!
//! Three places a client can say it, read in a fixed order:
//!
//! 1. the query parameter (`?locale=fr`), because a link that names a
//!    language has to win over a browser setting the visitor never chose;
//! 2. the cookie, because a choice the visitor made once has to last;
//! 3. `Accept-Language`, highest `q` first.
//!
//! Then the default. Each value is matched exactly, then with its last
//! subtag removed until one matches (`fr-CH` to `fr`), then by language
//! alone against a regional catalog (`en` to `en-GB` when there is no `en`).
//! A value that matches nothing is skipped, not an error: a stale cookie for
//! a locale the application dropped must not fail the request.

use std::fmt;
use std::sync::Arc;

/// The locales an application has catalogs for.
///
/// Implemented by the `Locale` enum the build script writes. One variant per
/// catalog file, so a locale with no catalog cannot be named.
pub trait Locales: Copy + Eq + fmt::Debug + Send + Sync + 'static {
    /// Every locale, the default first.
    const ALL: &'static [Self];
    /// The locale the build script was told is the default. Every key exists
    /// in it.
    const DEFAULT: Self;

    /// The BCP 47 tag, as the catalog file is named: `en`, `fr-CA`.
    fn tag(self) -> &'static str;

    /// The next locale to look in when this one lacks a key: `fr-CA` to `fr`.
    /// `None` means the default comes next.
    fn parent(self) -> Option<Self>;

    /// The locale with exactly this tag, ignoring ASCII case and taking `_`
    /// for `-`.
    fn from_tag(tag: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|l| same_tag(l.tag(), tag))
    }

    /// The text of a key with no placeholder and no plural, by its dotted
    /// name: `"title"`, `"gtag.banner.text"`. `None` for a key the catalog
    /// does not have, or one that takes an argument.
    ///
    /// This is the door for code that cannot name the application's
    /// `Locale` type: a hook in a library crate reads [`Current`] and asks
    /// it. A page names the key as a function, `t::title(locale)`, which
    /// the compiler checks. The build script writes this method; the
    /// default has no catalog.
    fn message(self, key: &str) -> Option<String> {
        let _ = key;
        None
    }
}

/// Where the query parameter and the cookie are looked for, and which
/// locales a request may get.
///
/// `None` turns a source off. The default reads `locale` in both, and says
/// so here rather than in a constant somewhere else.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "config",
    derive(serde::Serialize, serde::Deserialize),
    serde(default, deny_unknown_fields)
)]
pub struct NegotiatorConfig {
    /// The query parameter that names a locale: `?locale=fr`.
    pub query: Option<String>,
    /// The cookie that names a locale.
    pub cookie: Option<String>,
    /// The locale a request gets when it asks for none that is offered.
    /// `None`: the catalog's default, the locale the build script names.
    /// A static export sends no `Accept-Language`, so this is the language
    /// of every exported page.
    pub default: Option<String>,
    /// The locales a request may get, by tag. Empty: every catalog. A
    /// catalog left out still compiles and still backs a fallback, so a
    /// translation in progress ships without being offered.
    pub available: Vec<String>,
    /// A `?locale=` choice sets the cookie, so the next page keeps it. Only
    /// when both the query and the cookie are on. The cookie holds the
    /// choice the visitor made and nothing else, so it needs no consent.
    pub remember: bool,
    /// The locale is the path's first segment: `/fr/about`. The hook takes
    /// the segment off, so the page answers at `/about`, and sends a GET
    /// with no locale segment to the negotiated one. Off by default; the
    /// `i18n-routing` module turns it on.
    pub paths: bool,
    /// With `paths`: the prefixes that keep no locale segment and are
    /// never redirected. A path whose last segment has a `.` (a file, such
    /// as `/robots.txt`) is never redirected either, and neither is a
    /// socket or a stream. The default names the backoffice and the
    /// records API, where brocanth mounts them. A generated project adds
    /// the routes of other modules with
    /// [`keep_unprefixed`](Self::keep_unprefixed), after this list is read.
    pub unprefixed: Vec<String>,
}

impl Default for NegotiatorConfig {
    fn default() -> Self {
        Self {
            query: Some("locale".to_string()),
            cookie: Some("locale".to_string()),
            default: None,
            available: Vec::new(),
            remember: true,
            paths: false,
            unprefixed: vec!["/admin".to_string(), "/api".to_string()],
        }
    }
}

impl NegotiatorConfig {
    /// Turns the locale path segment on or off: the `paths` field.
    #[must_use]
    pub fn with_paths(mut self, on: bool) -> Self {
        self.paths = on;
        self
    }

    /// Adds a prefix to [`unprefixed`](Self::unprefixed).
    ///
    /// For a route another module owns, read from its module file:
    /// `.keep_unprefixed(crate::modules::storage_routes::MODULE.path)`. A
    /// generated project calls it after the file and the flags were read,
    /// so a file that sets `unprefixed` cannot send that route through a
    /// redirect, and a moved route needs no edit here.
    #[must_use]
    pub fn keep_unprefixed(mut self, prefix: impl Into<String>) -> Self {
        let prefix = prefix.into();
        if !self.unprefixed.contains(&prefix) {
            self.unprefixed.push(prefix);
        }
        self
    }

    /// Refuses a name that cannot appear in a query string or a `Cookie`
    /// header. Called by the plugins at start-up, so a typo stops the
    /// application rather than silently never matching.
    ///
    /// # Errors
    ///
    /// A sentence naming the field and the value.
    pub fn check(&self) -> Result<(), String> {
        for (field, value) in [("query", &self.query), ("cookie", &self.cookie)] {
            if let Some(name) = value {
                let ok = !name.is_empty()
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
                if !ok {
                    return Err(format!(
                        "[i18n] {field} = {name:?}: a name is ASCII letters, digits, `_` and `-`, \
                         and not empty"
                    ));
                }
            }
        }
        for prefix in &self.unprefixed {
            if !prefix.starts_with('/') {
                return Err(format!(
                    "[i18n] unprefixed names {prefix:?}, which is not a path from the root. \
                     help: write it with a leading `/`: \"/{}\"",
                    prefix.trim_start_matches('/')
                ));
            }
        }
        Ok(())
    }
}

/// What one request says about its language.
///
/// Named fields, because the three are all `&str` and a positional call
/// that swapped two would compile.
///
/// ```
/// # use brocanth_i18n::Asked;
/// let asked = Asked {
///     query: "page=2&locale=fr",
///     cookie: Some("theme=dark; locale=en"),
///     accept_language: Some("de-CH, de;q=0.9, en;q=0.5"),
/// };
/// # let _ = asked;
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct Asked<'a> {
    /// The query string, without the `?`. Empty when there is none.
    pub query: &'a str,
    /// The `Cookie` header.
    pub cookie: Option<&'a str>,
    /// The `Accept-Language` header.
    pub accept_language: Option<&'a str>,
}

/// Which source decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    Query,
    Cookie,
    AcceptLanguage,
    /// The path's first segment: `/fr/about`. Only with
    /// [`NegotiatorConfig::paths`], and then it wins over every other
    /// source, because the URL is what a search engine indexes.
    Path,
    /// Nothing the client sent matched a catalog.
    Default,
}

/// The locale a request gets, and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Negotiated<L> {
    pub locale: L,
    pub source: Source,
}

/// The negotiated locale, for code that cannot name the application's
/// `Locale` type.
///
/// [`I18nHook`](crate::I18nHook) puts one in the request state beside the
/// typed [`Negotiated`]. A hook in a library crate reads it with
/// `state.get::<Current>()` and asks it for a key's text. A page has the
/// typed value and calls `t::key(locale)` instead, which the compiler
/// checks.
#[derive(Clone, Copy)]
pub struct Current {
    tag: &'static str,
    source: Source,
    message: fn(&'static str, &str) -> Option<String>,
}

impl Current {
    /// The type-erased form of a negotiated locale.
    pub fn of<L: Locales>(negotiated: Negotiated<L>) -> Self {
        Self {
            tag: negotiated.locale.tag(),
            source: negotiated.source,
            message: message_of::<L>,
        }
    }

    /// The BCP 47 tag: `en`, `fr-CA`.
    pub fn tag(&self) -> &'static str {
        self.tag
    }

    /// Which source decided.
    pub fn source(&self) -> Source {
        self.source
    }

    /// The text of a key with no placeholder, in this locale, with the
    /// catalog's fallback applied. `None` when the catalog has no such key.
    pub fn text(&self, key: &str) -> Option<String> {
        (self.message)(self.tag, key)
    }
}

impl fmt::Debug for Current {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Current")
            .field("tag", &self.tag)
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

fn message_of<L: Locales>(tag: &'static str, key: &str) -> Option<String> {
    L::from_tag(tag)?.message(key)
}

/// Picks a locale for a request. Cheap to clone: the configuration is
/// shared.
pub struct Negotiator<L> {
    config: Arc<NegotiatorConfig>,
    /// The locales a request may get. Never empty.
    offered: Arc<[L]>,
    /// The locale a request that asks for nothing offered gets.
    default: L,
}

impl<L: Copy> Clone for Negotiator<L> {
    fn clone(&self) -> Self {
        Self {
            config: Arc::clone(&self.config),
            offered: Arc::clone(&self.offered),
            default: self.default,
        }
    }
}

impl<L> fmt::Debug for Negotiator<L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Negotiator")
            .field("query", &self.config.query)
            .field("cookie", &self.config.cookie)
            .finish()
    }
}

impl<L: Locales> Negotiator<L> {
    /// A negotiator for this configuration. A tag in `default` or
    /// `available` that names no catalog is left out here, and
    /// [`Negotiator::check`] says so: the plugins call it at start-up.
    pub fn new(config: NegotiatorConfig) -> Self {
        let mut offered: Vec<L> = config
            .available
            .iter()
            .filter_map(|tag| L::from_tag(tag))
            .collect();
        if offered.is_empty() {
            offered = L::ALL.to_vec();
        }
        let fallback = if offered.contains(&L::DEFAULT) {
            L::DEFAULT
        } else {
            offered[0]
        };
        let default = config
            .default
            .as_deref()
            .and_then(L::from_tag)
            .filter(|l| offered.contains(l))
            .unwrap_or(fallback);
        Self {
            config: Arc::new(config),
            offered: offered.into(),
            default,
        }
    }

    pub fn config(&self) -> &NegotiatorConfig {
        &self.config
    }

    /// The locales a request may get, in catalog order.
    pub fn offered(&self) -> &[L] {
        &self.offered
    }

    /// The locale a request gets when it asks for nothing offered.
    pub fn default_locale(&self) -> L {
        self.default
    }

    /// Refuses a configuration that cannot run: a name no client can send,
    /// a tag with no catalog, a default that is not offered.
    ///
    /// # Errors
    ///
    /// A sentence naming the setting and the value, with a `help:` line.
    pub fn check(&self) -> Result<(), String> {
        self.config.check()?;
        let known = || {
            L::ALL
                .iter()
                .map(|l| format!("`{}`", l.tag()))
                .collect::<Vec<_>>()
                .join(", ")
        };
        for tag in &self.config.available {
            if L::from_tag(tag).is_none() {
                return Err(format!(
                    "[i18n] available names {tag:?}, and there is no catalog for it. \
                     help: add `locales/{tag}.toml`, or use one of {}",
                    known()
                ));
            }
        }
        if let Some(tag) = &self.config.default {
            match L::from_tag(tag) {
                None => {
                    return Err(format!(
                        "[i18n] default = {tag:?}, and there is no catalog for it. \
                         help: use one of {}",
                        known()
                    ));
                }
                Some(l) if !self.offered.contains(&l) => {
                    return Err(format!(
                        "[i18n] default = {tag:?}, and `available` leaves it out. \
                         help: add {tag:?} to `available`"
                    ));
                }
                Some(_) => {}
            }
        }
        Ok(())
    }

    /// The locale for one request.
    pub fn negotiate(&self, asked: Asked<'_>) -> Negotiated<L> {
        self.negotiate_all(
            asked.query,
            asked.cookie.into_iter(),
            asked.accept_language.into_iter(),
        )
    }

    /// The same, for a request that may repeat `Cookie` and
    /// `Accept-Language`, as HTTP/2 allows.
    pub(crate) fn negotiate_all<'a>(
        &self,
        query: &str,
        cookies: impl Iterator<Item = &'a str>,
        accept_language: impl Iterator<Item = &'a str>,
    ) -> Negotiated<L> {
        if let Some(name) = &self.config.query {
            if let Some(locale) = query_values(query, name).find_map(|t| self.best(t)) {
                return Negotiated {
                    locale,
                    source: Source::Query,
                };
            }
        }
        if let Some(name) = &self.config.cookie {
            let found = cookies
                .flat_map(|header| cookie_values(header, name))
                .find_map(|t| self.best(t));
            if let Some(locale) = found {
                return Negotiated {
                    locale,
                    source: Source::Cookie,
                };
            }
        }
        let mut ranges = Vec::new();
        for header in accept_language {
            ranges.extend(accept_language_ranges(header));
        }
        // Stable, so two ranges with the same weight keep the client's order.
        ranges.sort_by(|a, b| b.1.total_cmp(&a.1));
        if let Some(locale) = ranges.into_iter().find_map(|(tag, _)| self.best(tag)) {
            return Negotiated {
                locale,
                source: Source::AcceptLanguage,
            };
        }
        Negotiated {
            locale: self.default,
            source: Source::Default,
        }
    }

    fn best(&self, tag: &str) -> Option<L> {
        best_match(tag, &self.offered)
    }

    /// The offered locale a path's first segment names, and the path
    /// without it: `/fr/about` is `(fr, "/about")`, `/fr` is `(fr, "/")`.
    /// The segment must be an offered tag exactly, ignoring case: `/fr-CH`
    /// names no locale when only `fr` is offered, because one page must
    /// have one URL.
    pub fn split_path<'a>(&self, path: &'a str) -> Option<(L, &'a str)> {
        let rest = path.strip_prefix('/')?;
        let (first, tail) = match rest.find('/') {
            Some(cut) => rest.split_at(cut),
            None => (rest, ""),
        };
        let locale = self
            .offered
            .iter()
            .copied()
            .find(|l| same_tag(l.tag(), first))?;
        Some((locale, if tail.is_empty() { "/" } else { tail }))
    }

    /// Whether a GET to a path with no locale segment is sent to the
    /// negotiated one. Not a file (a last segment with a `.`), and not
    /// under a prefix of [`NegotiatorConfig::unprefixed`].
    pub fn redirects(&self, path: &str) -> bool {
        let file = path
            .rsplit('/')
            .next()
            .is_some_and(|last| last.contains('.'));
        let kept = self.config.unprefixed.iter().any(|prefix| {
            let prefix = prefix.trim_end_matches('/');
            prefix.is_empty()
                || path == prefix
                || path
                    .strip_prefix(prefix)
                    .is_some_and(|rest| rest.starts_with('/'))
        });
        !file && !kept
    }

    /// What a page is told about a locale its path named.
    pub fn path_locale(&self, locale: L) -> PathLocale {
        PathLocale {
            tag: locale.tag(),
            offered: self.offered.iter().map(|l| l.tag()).collect(),
        }
    }
}

/// The locale a path named, for a page: `/fr/about` answers with `fr`, and
/// the page sees `/about`.
///
/// The `i18n` hook puts one in the request state when
/// [`NegotiatorConfig::paths`] is on and the path had a locale segment. A
/// page reads it with `ctx.hooked::<PathLocale>()` to write its links and
/// its `hreflang` alternates. Cheap to clone: the offered tags are shared.
///
/// ```
/// use brocanth_i18n::PathLocale;
///
/// let fr = PathLocale::new("fr", &["en", "fr"]);
/// assert_eq!(fr.href("/about"), "/fr/about");
/// assert_eq!(fr.href("/"), "/fr");
/// assert_eq!(
///     fr.alternates("/about"),
///     [("en", "/en/about".to_string()), ("fr", "/fr/about".to_string()), ("x-default", "/about".to_string())]
/// );
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathLocale {
    tag: &'static str,
    offered: Arc<[&'static str]>,
}

impl PathLocale {
    /// A path locale, for a test or a page built by hand.
    pub fn new(tag: &'static str, offered: &[&'static str]) -> Self {
        Self {
            tag,
            offered: offered.into(),
        }
    }

    /// The tag the path named: `fr`.
    pub fn tag(&self) -> &'static str {
        self.tag
    }

    /// Every offered tag, in catalog order.
    pub fn offered(&self) -> &[&'static str] {
        &self.offered
    }

    /// A path of this site in this locale: `/about` is `/fr/about`.
    pub fn href(&self, path: &str) -> String {
        prefixed(self.tag, path)
    }

    /// The same page in every offered locale, then `x-default`: the path
    /// with no segment, which sends a visitor to the locale they ask for.
    pub fn alternates(&self, path: &str) -> Vec<(&'static str, String)> {
        self.offered
            .iter()
            .map(|tag| (*tag, prefixed(tag, path)))
            .chain(std::iter::once(("x-default", path.to_string())))
            .collect()
    }
}

/// `path` under the segment `tag`: `/about` is `/fr/about`, and `/` is
/// `/fr`.
pub fn prefixed(tag: &str, path: &str) -> String {
    if path == "/" || path.is_empty() {
        format!("/{tag}")
    } else if path.starts_with('/') {
        format!("/{tag}{path}")
    } else {
        format!("/{tag}/{path}")
    }
}

/// `fr-CH` → `fr-CH`, then `fr`; then `fr` against `fr-CA` by language.
/// Only among `offered`.
pub(crate) fn best_match<L: Locales>(tag: &str, offered: &[L]) -> Option<L> {
    let tag = tag.trim();
    if tag.is_empty() || tag == "*" {
        return None;
    }
    let mut candidate = tag;
    loop {
        let exact = offered
            .iter()
            .copied()
            .find(|l| same_tag(l.tag(), candidate));
        if exact.is_some() {
            return exact;
        }
        match candidate.rfind(['-', '_']) {
            Some(cut) => candidate = &candidate[..cut],
            None => break,
        }
    }
    // `candidate` is the language subtag now. A catalog for one region of it
    // is a better answer than the default in another language.
    offered
        .iter()
        .copied()
        .find(|l| language(l.tag()).eq_ignore_ascii_case(candidate))
}

fn language(tag: &str) -> &str {
    tag.split(['-', '_']).next().unwrap_or(tag)
}

fn same_tag(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes().zip(b.bytes()).all(|(x, y)| {
            let norm = |c: u8| {
                if c == b'_' {
                    b'-'
                } else {
                    c.to_ascii_lowercase()
                }
            };
            norm(x) == norm(y)
        })
}

fn query_values<'a>(query: &'a str, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    query.split('&').filter_map(move |pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then_some(value)
    })
}

fn cookie_values<'a>(header: &'a str, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    header.split(';').filter_map(move |pair| {
        let (key, value) = pair.trim().split_once('=')?;
        (key.trim() == name).then_some(value.trim().trim_matches('"'))
    })
}

/// `de-CH, de;q=0.9, en;q=0.5` → `[(de-CH, 1.0), (de, 0.9), (en, 0.5)]`.
///
/// A range with `q=0` is the client saying "not this", so it is dropped,
/// and so is one whose `q` does not parse: guessing a weight is worse than
/// ignoring the range.
fn accept_language_ranges(header: &str) -> impl Iterator<Item = (&str, f32)> {
    header.split(',').filter_map(|item| {
        let mut parts = item.split(';');
        let tag = parts.next()?.trim();
        let mut weight = 1.0_f32;
        for param in parts {
            let (key, value) = param.trim().split_once('=')?;
            if key.trim().eq_ignore_ascii_case("q") {
                weight = value.trim().parse().ok()?;
            }
        }
        (!tag.is_empty() && weight > 0.0).then_some((tag, weight))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum L {
        En,
        Fr,
        FrCa,
        PtBr,
    }

    impl Locales for L {
        const ALL: &'static [Self] = &[L::En, L::Fr, L::FrCa, L::PtBr];
        const DEFAULT: Self = L::En;
        fn tag(self) -> &'static str {
            match self {
                L::En => "en",
                L::Fr => "fr",
                L::FrCa => "fr-CA",
                L::PtBr => "pt-BR",
            }
        }
        fn parent(self) -> Option<Self> {
            match self {
                L::FrCa => Some(L::Fr),
                _ => None,
            }
        }
    }

    fn pick(asked: Asked<'_>) -> Negotiated<L> {
        Negotiator::<L>::new(NegotiatorConfig::default()).negotiate(asked)
    }

    #[test]
    fn nothing_sent_gives_the_default() {
        let got = pick(Asked::default());
        assert_eq!(
            got,
            Negotiated {
                locale: L::En,
                source: Source::Default
            }
        );
    }

    #[test]
    fn accept_language_is_read_by_weight_not_by_position() {
        let got = pick(Asked {
            accept_language: Some("de;q=0.9, fr;q=0.95, en;q=0.1"),
            ..Asked::default()
        });
        assert_eq!(
            got,
            Negotiated {
                locale: L::Fr,
                source: Source::AcceptLanguage
            }
        );
    }

    #[test]
    fn a_range_nobody_has_is_skipped_for_the_next_one() {
        let got = pick(Asked {
            accept_language: Some("de-DE, de;q=0.9, fr;q=0.5"),
            ..Asked::default()
        });
        assert_eq!(got.locale, L::Fr);
    }

    #[test]
    fn a_region_we_lack_falls_back_to_its_language() {
        let got = pick(Asked {
            accept_language: Some("fr-CH"),
            ..Asked::default()
        });
        assert_eq!(got.locale, L::Fr);
    }

    #[test]
    fn a_region_we_have_is_taken_exactly_and_case_does_not_matter() {
        let got = pick(Asked {
            accept_language: Some("FR-ca"),
            ..Asked::default()
        });
        assert_eq!(got.locale, L::FrCa);
    }

    #[test]
    fn a_language_alone_finds_its_only_regional_catalog() {
        let got = pick(Asked {
            accept_language: Some("pt"),
            ..Asked::default()
        });
        assert_eq!(got.locale, L::PtBr);
    }

    #[test]
    fn q_zero_means_not_this_one() {
        let got = pick(Asked {
            accept_language: Some("fr;q=0, en;q=0.1"),
            ..Asked::default()
        });
        assert_eq!(got.locale, L::En);
        assert_eq!(got.source, Source::AcceptLanguage);
    }

    #[test]
    fn the_cookie_beats_the_browser_and_the_query_beats_the_cookie() {
        let browser = Some("en");
        let got = pick(Asked {
            cookie: Some("theme=dark; locale=fr"),
            accept_language: browser,
            ..Asked::default()
        });
        assert_eq!(
            got,
            Negotiated {
                locale: L::Fr,
                source: Source::Cookie
            }
        );

        let got = pick(Asked {
            query: "page=2&locale=fr-CA",
            cookie: Some("locale=en"),
            accept_language: browser,
        });
        assert_eq!(
            got,
            Negotiated {
                locale: L::FrCa,
                source: Source::Query
            }
        );
    }

    #[test]
    fn a_stale_cookie_is_ignored_rather_than_an_error() {
        let got = pick(Asked {
            cookie: Some("locale=xx"),
            accept_language: Some("fr"),
            ..Asked::default()
        });
        assert_eq!(
            got,
            Negotiated {
                locale: L::Fr,
                source: Source::AcceptLanguage
            }
        );
    }

    #[test]
    fn a_source_turned_off_is_not_read() {
        let n = Negotiator::<L>::new(NegotiatorConfig {
            query: None,
            cookie: Some("lang".into()),
            ..NegotiatorConfig::default()
        });
        let got = n.negotiate(Asked {
            query: "locale=fr",
            cookie: Some("locale=fr"),
            accept_language: None,
        });
        assert_eq!(got.source, Source::Default);
    }

    #[test]
    fn a_name_that_cannot_be_a_cookie_is_refused() {
        let bad = NegotiatorConfig {
            query: None,
            cookie: Some("a b".into()),
            ..NegotiatorConfig::default()
        };
        assert!(bad.check().is_err());
        assert!(NegotiatorConfig::default().check().is_ok());
    }

    fn with(default: Option<&str>, available: &[&str]) -> Negotiator<L> {
        Negotiator::new(NegotiatorConfig {
            default: default.map(String::from),
            available: available.iter().map(|t| t.to_string()).collect(),
            ..NegotiatorConfig::default()
        })
    }

    #[test]
    fn the_default_setting_answers_a_request_that_asks_for_nothing() {
        let n = with(Some("fr"), &[]);
        assert!(n.check().is_ok());
        let got = n.negotiate(Asked::default());
        assert_eq!((got.locale, got.source), (L::Fr, Source::Default));
        let got = n.negotiate(Asked {
            accept_language: Some("de"),
            ..Asked::default()
        });
        assert_eq!(got.locale, L::Fr, "a language nobody has");
    }

    #[test]
    fn a_locale_left_out_of_available_is_never_given() {
        let n = with(None, &["en", "fr"]);
        assert!(n.check().is_ok());
        let got = n.negotiate(Asked {
            accept_language: Some("fr-CA, pt;q=0.9"),
            ..Asked::default()
        });
        assert_eq!(got.locale, L::Fr, "fr-CA is compiled, not offered");
        let got = n.negotiate(Asked {
            query: "locale=pt-BR",
            ..Asked::default()
        });
        assert_eq!(got.source, Source::Default);
    }

    #[test]
    fn a_tag_with_no_catalog_or_a_default_not_offered_stops_the_start() {
        let unknown = with(Some("de"), &[]).check().unwrap_err();
        assert!(
            unknown.contains("help:") && unknown.contains("`fr-CA`"),
            "{unknown}"
        );
        assert!(with(None, &["xx"]).check().is_err());
        let hidden = with(Some("pt-BR"), &["en"]).check().unwrap_err();
        assert!(hidden.contains("available"), "{hidden}");
    }

    #[test]
    fn current_reads_the_catalog_without_naming_the_locale_type() {
        let current = Current::of(Negotiated {
            locale: L::Fr,
            source: Source::Cookie,
        });
        assert_eq!(current.tag(), "fr");
        assert_eq!(current.source(), Source::Cookie);
        // This test type has no catalog, so the trait's default answers.
        assert_eq!(current.text("title"), None);
    }
}
