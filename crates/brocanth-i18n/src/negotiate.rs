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
use std::marker::PhantomData;
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
}

/// Where the query parameter and the cookie are looked for.
///
/// `None` turns a source off. The default reads `locale` in both, and says
/// so here rather than in a constant somewhere else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NegotiatorConfig {
    /// The query parameter that names a locale: `?locale=fr`.
    pub query: Option<String>,
    /// The cookie that names a locale.
    pub cookie: Option<String>,
}

impl Default for NegotiatorConfig {
    fn default() -> Self {
        Self {
            query: Some("locale".to_string()),
            cookie: Some("locale".to_string()),
        }
    }
}

impl NegotiatorConfig {
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
    /// Nothing the client sent matched a catalog.
    Default,
}

/// The locale a request gets, and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Negotiated<L> {
    pub locale: L,
    pub source: Source,
}

/// Picks a locale for a request. Cheap to clone: the configuration is
/// shared.
pub struct Negotiator<L> {
    config: Arc<NegotiatorConfig>,
    _locales: PhantomData<fn() -> L>,
}

impl<L> Clone for Negotiator<L> {
    fn clone(&self) -> Self {
        Self {
            config: Arc::clone(&self.config),
            _locales: PhantomData,
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
    pub fn new(config: NegotiatorConfig) -> Self {
        Self {
            config: Arc::new(config),
            _locales: PhantomData,
        }
    }

    pub fn config(&self) -> &NegotiatorConfig {
        &self.config
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
            if let Some(locale) = query_values(query, name).find_map(best_match::<L>) {
                return Negotiated {
                    locale,
                    source: Source::Query,
                };
            }
        }
        if let Some(name) = &self.config.cookie {
            let found = cookies
                .flat_map(|header| cookie_values(header, name))
                .find_map(best_match::<L>);
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
        if let Some(locale) = ranges.into_iter().find_map(|(tag, _)| best_match::<L>(tag)) {
            return Negotiated {
                locale,
                source: Source::AcceptLanguage,
            };
        }
        Negotiated {
            locale: L::DEFAULT,
            source: Source::Default,
        }
    }
}

/// `fr-CH` → `fr-CH`, then `fr`; then `fr` against `fr-CA` by language.
pub(crate) fn best_match<L: Locales>(tag: &str) -> Option<L> {
    let tag = tag.trim();
    if tag.is_empty() || tag == "*" {
        return None;
    }
    let mut candidate = tag;
    loop {
        if let Some(found) = L::from_tag(candidate) {
            return Some(found);
        }
        match candidate.rfind(['-', '_']) {
            Some(cut) => candidate = &candidate[..cut],
            None => break,
        }
    }
    // `candidate` is the language subtag now. A catalog for one region of it
    // is a better answer than the default in another language.
    L::ALL
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
        };
        assert!(bad.check().is_err());
        assert!(NegotiatorConfig::default().check().is_ok());
    }
}
