//! The three types: the site, the page, and the head they resolve to.

use std::fmt;
use std::sync::Arc;

use brocanth::plugin_context::ContextItem;

/// What every page shares: the `[meta]` settings.
///
/// Cheap to clone, because the context gets one per request: the parts are
/// shared.
///
/// ```
/// use brocanth_meta::Site;
///
/// let site = Site::new("My Blog")
///     .title_template("%s | My Blog")
///     .base_url("https://blog.example");
/// assert_eq!(site.name(), "My Blog");
/// assert!(site.check().is_ok());
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Site {
    parts: Arc<Parts>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Parts {
    name: String,
    template: Option<String>,
    base_url: Option<String>,
    description: Option<String>,
    image: Option<String>,
    twitter: Option<String>,
}

impl ContextItem for Site {}

impl Site {
    /// A site with this name. A page with no title of its own is titled
    /// with it.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            parts: Arc::new(Parts {
                name: name.into(),
                ..Parts::default()
            }),
        }
    }

    fn with(mut self, set: impl FnOnce(&mut Parts)) -> Self {
        set(Arc::make_mut(&mut self.parts));
        self
    }

    /// How a page's title becomes the document's: `%s` is the page's own.
    /// `"%s | My Blog"` titles `/about` "About | My Blog".
    #[must_use]
    pub fn title_template(self, template: impl Into<String>) -> Self {
        let template = template.into();
        self.with(|p| p.template = Some(template))
    }

    /// The site's address, scheme and host: `https://blog.example`. A
    /// canonical URL and `og:url` are absolute, so without it they are left
    /// out.
    #[must_use]
    pub fn base_url(self, url: impl Into<String>) -> Self {
        let url = url.into();
        self.with(|p| p.base_url = Some(url.trim_end_matches('/').to_string()))
    }

    /// The description of a page that gives none.
    #[must_use]
    pub fn description(self, text: impl Into<String>) -> Self {
        let text = text.into();
        self.with(|p| p.description = Some(text))
    }

    /// The `og:image` of a page that gives none: a path or a full URL.
    #[must_use]
    pub fn image(self, url: impl Into<String>) -> Self {
        let url = url.into();
        self.with(|p| p.image = Some(url))
    }

    /// The site's account, for `twitter:site`: `@blog`.
    #[must_use]
    pub fn twitter(self, handle: impl Into<String>) -> Self {
        let handle = handle.into();
        self.with(|p| p.twitter = Some(handle))
    }

    pub fn name(&self) -> &str {
        &self.parts.name
    }

    /// The base URL, with no trailing `/`.
    pub fn base(&self) -> Option<&str> {
        self.parts.base_url.as_deref()
    }

    /// Refuses a configuration that writes wrong tags.
    ///
    /// # Errors
    ///
    /// A sentence naming the setting, with a `help:` line.
    pub fn check(&self) -> Result<(), String> {
        if let Some(template) = &self.parts.template {
            if !template.contains("%s") {
                return Err(format!(
                    "title_template = {template:?} has no `%s`, so every page would have \
                     the same title. help: write the page's title as `%s`: \"%s | {}\"",
                    self.parts.name
                ));
            }
        }
        if let Some(base) = &self.parts.base_url {
            if !(base.starts_with("https://") || base.starts_with("http://")) {
                return Err(format!(
                    "base_url = {base:?} is not a full address, and a canonical URL must \
                     be one. help: write the scheme and the host: \"https://{}\"",
                    base.trim_start_matches('/')
                ));
            }
        }
        Ok(())
    }
}

/// What one page declares. Every field is optional: what a page leaves out
/// comes from the [`Site`].
///
/// A page sets it on the context before it renders, with the builder
/// methods, which chain:
///
/// ```
/// use brocanth_meta::Meta;
///
/// let mut meta = Meta::default();
/// meta.title("About").description("Who writes this blog.");
/// assert_eq!(meta.title.as_deref(), Some("About"));
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Meta {
    /// The page's own title. The site's template wraps it.
    pub title: Option<String>,
    pub description: Option<String>,
    /// A path or a full URL. `None`: the request's path.
    pub canonical: Option<String>,
    /// `og:image`: a path or a full URL.
    pub image: Option<String>,
    /// `og:type`. `None`: `website`.
    pub kind: Option<String>,
    /// `<meta name="robots">`: `noindex`.
    pub robots: Option<String>,
}

impl Meta {
    pub fn title(&mut self, title: impl Into<String>) -> &mut Self {
        self.title = Some(title.into());
        self
    }

    pub fn description(&mut self, text: impl Into<String>) -> &mut Self {
        self.description = Some(text.into());
        self
    }

    /// The one URL this page is known by, when the request's path is not
    /// it: a list's first page, a post reachable at two paths.
    pub fn canonical(&mut self, url: impl Into<String>) -> &mut Self {
        self.canonical = Some(url.into());
        self
    }

    pub fn image(&mut self, url: impl Into<String>) -> &mut Self {
        self.image = Some(url.into());
        self
    }

    /// `og:type`: `article` for a post.
    pub fn kind(&mut self, kind: impl Into<String>) -> &mut Self {
        self.kind = Some(kind.into());
        self
    }

    /// Asks search engines to leave this page out.
    pub fn noindex(&mut self) -> &mut Self {
        self.robots = Some("noindex".to_string());
        self
    }
}

/// One `<link rel="alternate" hreflang>`: the same page in another
/// language. `x-default` names the page for a visitor no language matches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alternate {
    pub hreflang: String,
    /// A path or a full URL.
    pub href: String,
}

/// A page's head: the site and the page resolved. The root layout writes
/// [`Head::title`] in `<title>`, [`Head::lang`] in `<html lang>`, and the
/// head itself, unescaped (`{! meta }`), for the tags. Every value in the
/// tags is escaped.
///
/// ```
/// use brocanth_meta::{Head, Meta, Site};
///
/// let site = Site::new("My Blog")
///     .title_template("%s | My Blog")
///     .base_url("https://blog.example");
/// let mut page = Meta::default();
/// page.title("About");
///
/// let head = Head::new(&site, Some(&page)).path("/about");
/// assert_eq!(head.title(), "About | My Blog");
/// let tags = head.to_string();
/// assert!(tags.contains(r#"<link rel="canonical" href="https://blog.example/about">"#));
/// assert!(tags.contains(r#"<meta property="og:title" content="About">"#));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Head {
    site: Site,
    page: Meta,
    path: String,
    lang: Option<String>,
    base: Option<String>,
    alternates: Vec<Alternate>,
}

impl Head {
    /// The head of a page. `None` is a page that declared nothing.
    pub fn new(site: &Site, page: Option<&Meta>) -> Self {
        Self {
            site: site.clone(),
            page: page.cloned().unwrap_or_default(),
            path: "/".to_string(),
            lang: None,
            base: site.base().map(str::to_string),
            alternates: Vec::new(),
        }
    }

    /// The path this page answers at, for its canonical URL.
    #[must_use]
    pub fn path(mut self, path: &str) -> Self {
        self.path = path.to_string();
        self
    }

    /// The document's language: `fr`. `None` is `en`.
    #[must_use]
    pub fn lang(mut self, tag: &str) -> Self {
        self.lang = Some(tag.to_string());
        self
    }

    /// A base URL for a site whose `base_url` is unset: the sitemap's.
    #[must_use]
    pub fn base_url_or(mut self, url: &str) -> Self {
        if self.base.is_none() && !url.is_empty() {
            self.base = Some(url.trim_end_matches('/').to_string());
        }
        self
    }

    /// The same page in another language.
    #[must_use]
    pub fn alternate(mut self, hreflang: &str, href: &str) -> Self {
        self.alternates.push(Alternate {
            hreflang: hreflang.to_string(),
            href: href.to_string(),
        });
        self
    }

    /// The document's title: the page's, through the site's template; or
    /// the site's name for a page with none.
    pub fn title(&self) -> String {
        match (&self.page.title, &self.site.parts.template) {
            (Some(title), Some(template)) => template.replace("%s", title),
            (Some(title), None) => title.clone(),
            (None, _) => self.site.name().to_string(),
        }
    }

    /// The document's language, for `<html lang>`.
    pub fn lang_tag(&self) -> &str {
        self.lang.as_deref().unwrap_or("en")
    }

    /// The page's canonical URL. `None` without a base URL: a canonical URL
    /// must be absolute.
    pub fn canonical(&self) -> Option<String> {
        let path = self.page.canonical.as_deref().unwrap_or(&self.path);
        self.absolute(path)
    }

    fn description(&self) -> Option<&str> {
        self.page
            .description
            .as_deref()
            .or(self.site.parts.description.as_deref())
    }

    fn image(&self) -> Option<String> {
        let image = self
            .page
            .image
            .as_deref()
            .or(self.site.parts.image.as_deref())?;
        // A relative image still works in a browser; a crawler wants a full
        // URL, which needs the base.
        Some(self.absolute(image).unwrap_or_else(|| image.to_string()))
    }

    fn absolute(&self, url: &str) -> Option<String> {
        if url.starts_with("https://") || url.starts_with("http://") {
            return Some(url.to_string());
        }
        let base = self.base.as_deref()?;
        Some(if url.starts_with('/') {
            format!("{base}{url}")
        } else {
            format!("{base}/{url}")
        })
    }

    /// Every tag as `(kind, name, value)`: `("name", "description", ..)`,
    /// `("property", "og:title", ..)`, `("link", "canonical", ..)`. One list
    /// for the HTML and the JSON, so the two cannot disagree.
    fn tags(&self) -> Vec<(&'static str, String, String)> {
        let mut out = Vec::new();
        let own_title = self
            .page
            .title
            .clone()
            .unwrap_or_else(|| self.site.name().to_string());
        let description = self.description().map(str::to_string);
        let canonical = self.canonical();
        let image = self.image();
        if let Some(d) = &description {
            out.push(("name", "description".into(), d.clone()));
        }
        if let Some(c) = &canonical {
            out.push(("link", "canonical".into(), c.clone()));
        }
        if let Some(r) = &self.page.robots {
            out.push(("name", "robots".into(), r.clone()));
        }
        for alternate in &self.alternates {
            let href = self
                .absolute(&alternate.href)
                .unwrap_or_else(|| alternate.href.clone());
            out.push(("alternate", alternate.hreflang.clone(), href));
        }
        out.push(("property", "og:title".into(), own_title.clone()));
        if let Some(d) = &description {
            out.push(("property", "og:description".into(), d.clone()));
        }
        if let Some(c) = &canonical {
            out.push(("property", "og:url".into(), c.clone()));
        }
        if !self.site.name().is_empty() {
            out.push(("property", "og:site_name".into(), self.site.name().into()));
        }
        let kind = self.page.kind.as_deref().unwrap_or("website");
        out.push(("property", "og:type".into(), kind.into()));
        if let Some(i) = &image {
            out.push(("property", "og:image".into(), i.clone()));
        }
        if let Some(lang) = &self.lang {
            out.push(("property", "og:locale".into(), lang.replace('-', "_")));
        }
        let card = if image.is_some() {
            "summary_large_image"
        } else {
            "summary"
        };
        out.push(("name", "twitter:card".into(), card.into()));
        if let Some(t) = &self.site.parts.twitter {
            out.push(("name", "twitter:site".into(), t.clone()));
        }
        out.push(("name", "twitter:title".into(), own_title));
        if let Some(d) = description {
            out.push(("name", "twitter:description".into(), d));
        }
        if let Some(i) = image {
            out.push(("name", "twitter:image".into(), i));
        }
        out
    }

    /// The head as JSON, for a `.jt` template: `jt::Raw(&head.to_json())`.
    ///
    /// ```
    /// use brocanth_meta::{Head, Site};
    ///
    /// let json = Head::new(&Site::new("My Blog"), None).to_json();
    /// assert!(json.starts_with(r#"{"title":"My Blog","lang":"en","tags":["#));
    /// ```
    pub fn to_json(&self) -> String {
        let tags: Vec<String> = self
            .tags()
            .iter()
            .map(|(kind, name, value)| {
                format!(
                    "{{\"kind\":{},\"name\":{},\"value\":{}}}",
                    json(kind),
                    json(name),
                    json(value)
                )
            })
            .collect();
        format!(
            "{{\"title\":{},\"lang\":{},\"tags\":[{}]}}",
            json(&self.title()),
            json(self.lang_tag()),
            tags.join(",")
        )
    }
}

/// The tags, one per line, every value escaped. The `<title>` is not here:
/// the layout writes it, so a page without this crate keeps one.
impl fmt::Display for Head {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, (kind, name, value)) in self.tags().iter().enumerate() {
            if i > 0 {
                f.write_str("\n")?;
            }
            let (name, value) = (escape(name), escape(value));
            match *kind {
                "link" => write!(f, r#"<link rel="{name}" href="{value}">"#)?,
                "alternate" => write!(
                    f,
                    r#"<link rel="alternate" hreflang="{name}" href="{value}">"#
                )?,
                _ => write!(f, r#"<meta {kind}="{name}" content="{value}">"#)?,
            }
        }
        Ok(())
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn json(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // `<` too, so the JSON can sit in a `<script>` element.
            '<' => out.push_str("\\u003c"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site() -> Site {
        Site::new("My Blog")
            .title_template("%s | My Blog")
            .base_url("https://blog.example/")
            .description("Notes.")
            .twitter("@blog")
    }

    #[test]
    fn a_page_with_no_metadata_takes_the_site() {
        let head = Head::new(&site(), None).path("/");
        assert_eq!(head.title(), "My Blog");
        let tags = head.to_string();
        assert!(
            tags.contains(r#"<meta name="description" content="Notes.">"#),
            "{tags}"
        );
        assert!(tags.contains(r#"href="https://blog.example/">"#), "{tags}");
        assert!(
            tags.contains(r#"<meta name="twitter:card" content="summary">"#),
            "{tags}"
        );
        assert!(
            tags.contains(r#"<meta name="twitter:site" content="@blog">"#),
            "{tags}"
        );
    }

    #[test]
    fn every_value_is_escaped() {
        let mut page = Meta::default();
        page.title(r#""><script>"#).description("a & b");
        let tags = Head::new(&site(), Some(&page)).to_string();
        assert!(!tags.contains("<script>"), "{tags}");
        assert!(tags.contains("&quot;&gt;&lt;script&gt;"), "{tags}");
        assert!(tags.contains("a &amp; b"), "{tags}");
        let json = Head::new(&site(), Some(&page)).to_json();
        assert!(!json.contains("<script>"), "{json}");
    }

    #[test]
    fn without_a_base_url_there_is_no_canonical() {
        let head = Head::new(&Site::new("x"), None).path("/a");
        assert_eq!(head.canonical(), None);
        let head = head.base_url_or("http://localhost:3000");
        assert_eq!(head.canonical().as_deref(), Some("http://localhost:3000/a"));
    }

    #[test]
    fn an_alternate_and_an_image_are_made_absolute() {
        let mut page = Meta::default();
        page.image("/og.png").noindex().kind("article");
        let tags = Head::new(&site(), Some(&page))
            .path("/fr/about")
            .lang("fr-CA")
            .alternate("en", "/en/about")
            .alternate("x-default", "/about")
            .to_string();
        assert!(
            tags.contains(
                r#"<link rel="alternate" hreflang="en" href="https://blog.example/en/about">"#
            ),
            "{tags}"
        );
        assert!(tags.contains(r#"hreflang="x-default""#), "{tags}");
        assert!(
            tags.contains(r#"content="https://blog.example/og.png""#),
            "{tags}"
        );
        assert!(tags.contains(r#"content="summary_large_image""#), "{tags}");
        assert!(
            tags.contains(r#"<meta name="robots" content="noindex">"#),
            "{tags}"
        );
        assert!(tags.contains(r#"content="fr_CA""#), "{tags}");
        assert!(
            tags.contains(r#"<meta property="og:type" content="article">"#),
            "{tags}"
        );
    }

    #[test]
    fn a_template_with_no_hole_and_a_relative_base_are_refused() {
        let why = Site::new("x").title_template("x").check().unwrap_err();
        assert!(why.contains("help:"), "{why}");
        let why = Site::new("x").base_url("blog.example").check().unwrap_err();
        assert!(why.contains("https://blog.example"), "{why}");
    }
}
