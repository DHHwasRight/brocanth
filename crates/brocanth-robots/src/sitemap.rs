//! A sitemap.xml, written from a builder and checked before it is served.
//!
//! The format is the sitemaps.org protocol 0.9: one `<urlset>`, one `<url>`
//! per page, each with an absolute `<loc>` and an optional `<lastmod>`.
//! A crawler ignores a relative `<loc>`, and drops a whole file that holds
//! more than 50,000 URLs. [`Sitemap::render`] refuses both.
//!
//! ```
//! use brocanth_robots::sitemap::Sitemap;
//!
//! let xml = Sitemap::new("https://example.com")
//!     .path("/")
//!     .path_modified("/about", "2026-11-05")
//!     .render()
//!     .unwrap();
//! assert!(xml.contains("<loc>https://example.com/about</loc>"));
//! assert!(xml.contains("<lastmod>2026-11-05</lastmod>"));
//! ```

use std::collections::HashMap;
use std::fmt;

/// The most URLs one sitemap file may hold (sitemaps.org).
pub const MAX_URLS: usize = 50_000;

/// One `<url>` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    path: String,
    lastmod: Option<String>,
}

/// A sitemap.xml for one site.
///
/// The base URL is the scheme, the host and the base path, if the site is
/// served under one: `https://example.com` or
/// `https://user.github.io/repo`. Each path is written after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sitemap {
    base: String,
    entries: Vec<Entry>,
    /// Where each path is in `entries`, so a long list stays linear.
    index: HashMap<String, usize>,
}

impl Sitemap {
    /// An empty sitemap for the site at `base`. A trailing `/` is removed.
    pub fn new(base: impl Into<String>) -> Self {
        let mut base = base.into();
        while base.ends_with('/') {
            base.pop();
        }
        Self {
            base,
            entries: Vec::new(),
            index: HashMap::new(),
        }
    }

    /// One page, by its path from the root of the site: `/about`.
    /// A path given two times is written one time.
    pub fn path(self, path: impl Into<String>) -> Self {
        self.push(path.into(), None)
    }

    /// One page, with the date it last changed: `2026-11-05`, or a full
    /// W3C date and time such as `2026-11-05T10:00:00+00:00`.
    pub fn path_modified(self, path: impl Into<String>, lastmod: impl Into<String>) -> Self {
        self.push(path.into(), Some(lastmod.into()))
    }

    fn push(mut self, path: String, lastmod: Option<String>) -> Self {
        match self.index.get(&path) {
            Some(&at) => {
                if lastmod.is_some() {
                    self.entries[at].lastmod = lastmod;
                }
            }
            None => {
                self.index.insert(path.clone(), self.entries.len());
                self.entries.push(Entry { path, lastmod });
            }
        }
        self
    }

    /// The base URL, with no trailing `/`.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// The full URL of this file when it is served at `/sitemap.xml`. This
    /// is the value of the `Sitemap:` line in robots.txt.
    pub fn location(&self) -> String {
        format!("{}/sitemap.xml", self.base)
    }

    /// How many pages the sitemap lists.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the sitemap lists no page.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The XML, or the first value that would be wrong.
    ///
    /// # Errors
    ///
    /// [`SitemapError`] when the base is not an absolute `http(s)` URL or
    /// holds a query or a fragment, when a path does not start with `/`,
    /// when a date is not a W3C date, or when there are more than
    /// [`MAX_URLS`] pages.
    pub fn render(&self) -> Result<String, SitemapError> {
        check_base(&self.base)?;
        if self.entries.len() > MAX_URLS {
            return Err(SitemapError::TooMany(self.entries.len()));
        }
        let mut out = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n",
        );
        for entry in &self.entries {
            check_path(&entry.path)?;
            out.push_str("  <url>\n    <loc>");
            escape_into(&mut out, &self.base);
            escape_into(&mut out, &encode_path(&entry.path));
            out.push_str("</loc>\n");
            if let Some(lastmod) = &entry.lastmod {
                check_lastmod(lastmod)?;
                out.push_str("    <lastmod>");
                out.push_str(lastmod);
                out.push_str("</lastmod>\n");
            }
            out.push_str("  </url>\n");
        }
        out.push_str("</urlset>\n");
        Ok(out)
    }
}

/// Whether a route pattern belongs in a sitemap by default.
///
/// A pattern with a parameter (`/users/{id}`) is not a page: it is many,
/// and only the application knows which exist. A pattern whose last
/// segment has a dot (`/robots.txt`, `/openapi.json`) is a file, not a
/// page.
pub fn listed(pattern: &str) -> bool {
    let last = pattern.rsplit('/').next().unwrap_or_default();
    pattern.starts_with('/') && !pattern.contains(['{', '}', '*']) && !last.contains('.')
}

/// Whether `path` is `prefix` or a path under it, segment by segment:
/// `/admin` holds `/admin/users` and not `/administrator`.
pub fn under(path: &str, prefix: &str) -> bool {
    let prefix = prefix.trim_end_matches('/');
    if prefix.is_empty() {
        return true;
    }
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn check_base(base: &str) -> Result<(), SitemapError> {
    let rest = base
        .strip_prefix("https://")
        .or_else(|| base.strip_prefix("http://"));
    let host = rest.and_then(|rest| rest.split('/').next());
    let bad = host.is_none_or(str::is_empty)
        || base.contains(['?', '#'])
        || base.chars().any(|c| c.is_whitespace() || c.is_control());
    if bad {
        return Err(SitemapError::Base(base.to_string()));
    }
    Ok(())
}

fn check_path(path: &str) -> Result<(), SitemapError> {
    if !path.starts_with('/') || path.starts_with("//") || path.chars().any(char::is_control) {
        return Err(SitemapError::Path(path.to_string()));
    }
    Ok(())
}

/// `YYYY-MM-DD`, optionally followed by `T` and a time.
fn check_lastmod(value: &str) -> Result<(), SitemapError> {
    let b = value.as_bytes();
    let date = b.len() >= 10
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[7] == b'-'
        && b[8..10].iter().all(u8::is_ascii_digit);
    let time = b.len() == 10
        || (b.len() > 11
            && b[10] == b'T'
            && b[11..]
                .iter()
                .all(|c| c.is_ascii_digit() || b":+-.Z".contains(c)));
    if !(date && time) {
        return Err(SitemapError::Lastmod(value.to_string()));
    }
    Ok(())
}

/// Percent-encodes every byte that is not printable ASCII, so a page whose
/// file name is not ASCII still has a valid `<loc>`.
fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        if byte.is_ascii_graphic() {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn escape_into(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
}

/// A value [`Sitemap::render`] refuses to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SitemapError {
    /// A base URL that is not absolute `http(s)`, or holds a query, a
    /// fragment or a space.
    Base(String),
    /// A path that does not start with a single `/`.
    Path(String),
    /// A date that is not `YYYY-MM-DD` or a W3C date and time.
    Lastmod(String),
    /// More pages than one file may hold.
    TooMany(usize),
}

impl fmt::Display for SitemapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Base(base) => write!(
                f,
                "sitemap base URL {base:?} is not an absolute URL, so crawlers ignore every entry\n\
                 help: give the scheme and the host, and the base path if the site has one, \
                 for example \"https://example.com\""
            ),
            Self::Path(path) => write!(
                f,
                "sitemap path {path:?} does not start with `/`\n\
                 help: write it from the root of the site, for example \"/{}\"",
                path.trim_start_matches(['.', '/'])
            ),
            Self::Lastmod(value) => write!(
                f,
                "sitemap date {value:?} is not a W3C date\n\
                 help: write it as \"2026-11-05\" or \"2026-11-05T10:00:00+00:00\""
            ),
            Self::TooMany(count) => write!(
                f,
                "the sitemap lists {count} pages, and one file may hold {MAX_URLS}\n\
                 help: exclude a path prefix, or split the site into several sitemaps"
            ),
        }
    }
}

impl std::error::Error for SitemapError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_sitemap_is_a_valid_urlset() {
        assert_eq!(
            Sitemap::new("https://example.com/").render().unwrap(),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n\
             </urlset>\n"
        );
    }

    #[test]
    fn each_path_is_written_once_after_the_base() {
        let sitemap = Sitemap::new("https://user.github.io/repo/")
            .path("/")
            .path("/about")
            .path_modified("/about", "2026-11-05");
        assert_eq!(sitemap.len(), 2);
        assert_eq!(
            sitemap.location(),
            "https://user.github.io/repo/sitemap.xml"
        );
        assert_eq!(
            sitemap.render().unwrap(),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n\
             \x20 <url>\n\
             \x20   <loc>https://user.github.io/repo/</loc>\n\
             \x20 </url>\n\
             \x20 <url>\n\
             \x20   <loc>https://user.github.io/repo/about</loc>\n\
             \x20   <lastmod>2026-11-05</lastmod>\n\
             \x20 </url>\n\
             </urlset>\n"
        );
    }

    #[test]
    fn markup_and_non_ascii_are_escaped() {
        let xml = Sitemap::new("https://example.com")
            .path("/a&b")
            .path("/café")
            .render()
            .unwrap();
        assert!(
            xml.contains("<loc>https://example.com/a&amp;b</loc>"),
            "{xml}"
        );
        assert!(
            xml.contains("<loc>https://example.com/caf%C3%A9</loc>"),
            "{xml}"
        );
    }

    #[test]
    fn a_relative_base_a_bad_path_and_a_bad_date_are_refused() {
        for base in [
            "example.com",
            "/",
            "https://",
            "https://a.com?x=1",
            "ftp://a.com",
        ] {
            assert!(
                matches!(Sitemap::new(base).render(), Err(SitemapError::Base(_))),
                "{base}"
            );
        }
        let error = Sitemap::new("https://a.com")
            .path("about")
            .render()
            .unwrap_err();
        assert_eq!(error, SitemapError::Path("about".to_string()));
        assert!(error.to_string().contains("help:"));
        for date in ["05/11/2026", "2026-11-5", "2026-11-05 10:00", "yesterday"] {
            assert!(
                matches!(
                    Sitemap::new("https://a.com")
                        .path_modified("/", date)
                        .render(),
                    Err(SitemapError::Lastmod(_))
                ),
                "{date}"
            );
        }
        assert!(Sitemap::new("https://a.com")
            .path_modified("/", "2026-11-05T10:00:00+00:00")
            .render()
            .is_ok());
    }

    #[test]
    fn more_than_fifty_thousand_pages_is_refused() {
        let mut sitemap = Sitemap::new("https://a.com");
        for n in 0..=MAX_URLS {
            sitemap = sitemap.path(format!("/p/{n}"));
        }
        assert_eq!(
            sitemap.render().unwrap_err(),
            SitemapError::TooMany(MAX_URLS + 1)
        );
    }

    #[test]
    fn listed_keeps_pages_and_drops_files_and_parameters() {
        assert!(listed("/"));
        assert!(listed("/about"));
        assert!(listed("/blog/archive"));
        assert!(!listed("/users/{id}"));
        assert!(!listed("/admin/{*rest}"));
        assert!(!listed("/robots.txt"));
        assert!(!listed("/openapi.json"));
    }

    #[test]
    fn under_matches_whole_segments() {
        assert!(under("/admin", "/admin"));
        assert!(under("/admin/users", "/admin/"));
        assert!(!under("/administrator", "/admin"));
        assert!(under("/anything", "/"));
    }
}
