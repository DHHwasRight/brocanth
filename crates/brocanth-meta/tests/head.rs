//! The tags a page's head writes: escaping, the canonical URL, the share
//! image, and the `hreflang` alternates `i18n-routing` adds.

use brocanth_i18n::PathLocale;
use brocanth_meta::{Head, Meta, Site};

fn site() -> Site {
    Site::new("My Blog")
        .title_template("%s | My Blog")
        .base_url("https://blog.example")
}

fn page(set: impl FnOnce(&mut Meta)) -> Meta {
    let mut meta = Meta::default();
    set(&mut meta);
    meta
}

/// The value of the first tag whose name is `name`, as the HTML writes it.
fn content<'a>(tags: &'a str, name: &str) -> Option<&'a str> {
    tags.lines().find_map(|line| {
        let named = line.contains(&format!("=\"{name}\" content=\""))
            || line.contains(&format!("rel=\"{name}\" href=\""));
        named.then(|| {
            let start = line
                .find(" content=\"")
                .map(|i| i + 10)
                .or_else(|| line.find(" href=\"").map(|i| i + 7))?;
            line[start..].strip_suffix("\">")
        })?
    })
}

#[test]
fn quotes_and_markup_in_every_value_are_escaped() {
    let site = Site::new(r#"Tom's "Blog" <b>"#)
        .base_url("https://blog.example")
        .twitter(r#"@x"><script>"#)
        .image(r#"/og.png?a=1&b="2""#);
    let meta = page(|m| {
        m.title("</title><script>alert(1)</script>")
            .description(r#"it's "quoted" & <i>bold</i>"#)
            .canonical("/a?x=1&y='2'")
            .kind(r#"article" onload="x"#);
    });
    let head = Head::new(&site, Some(&meta)).alternate(r#"fr"><script>"#, r#"/fr" x="1"#);
    let tags = head.to_string();
    for line in tags.lines() {
        // Each tag is one element with only its own attributes: no value
        // closes its quote or opens an element.
        assert_eq!(line.matches('<').count(), 1, "{line}");
        assert_eq!(line.matches('>').count(), 1, "{line}");
        assert_eq!(line.matches('\'').count(), 0, "{line}");
    }
    assert!(!tags.contains("<script"), "{tags}");
    assert_eq!(
        content(&tags, "description"),
        Some("it&#39;s &quot;quoted&quot; &amp; &lt;i&gt;bold&lt;/i&gt;")
    );
    assert_eq!(
        content(&tags, "canonical"),
        Some("https://blog.example/a?x=1&amp;y=&#39;2&#39;")
    );
    assert_eq!(
        content(&tags, "og:site_name"),
        Some("Tom&#39;s &quot;Blog&quot; &lt;b&gt;")
    );
    assert_eq!(
        content(&tags, "og:image"),
        Some("https://blog.example/og.png?a=1&amp;b=&quot;2&quot;")
    );
    // The title the layout writes is the page's own text; the layout
    // escapes it as it escapes any value.
    assert_eq!(head.title(), "</title><script>alert(1)</script>");
}

#[test]
fn the_json_head_can_sit_in_a_script_element() {
    let meta = page(|m| {
        m.title("</script><script>alert(1)</script>")
            .description("line\nbreak \"q\" \\ \t\u{1}");
    });
    let json = Head::new(&site(), Some(&meta)).to_json();
    assert!(!json.contains("</script"), "{json}");
    assert!(!json.contains('\n'), "{json}");
    assert!(json.contains(r#"line\nbreak \"q\" \\ \t\u0001"#), "{json}");
    let title = r#"{"title":"\u003c/script>\u003cscript>alert(1)\u003c/script> | My Blog","#;
    assert!(json.starts_with(title), "{json}");
}

#[test]
fn the_html_and_the_json_list_the_same_tags() {
    let meta = page(|m| {
        m.title("About").description("d").image("/a.png").noindex();
    });
    let head = Head::new(&site().twitter("@b"), Some(&meta))
        .path("/fr/about")
        .lang("fr")
        .alternate("en", "/en/about");
    let html = head.to_string().lines().count();
    let json = head.to_json().matches("\"kind\":").count();
    assert_eq!(html, json);
}

#[test]
fn a_canonical_url_needs_a_base_url() {
    let plain = Site::new("x").description("d");
    let meta = page(|m| {
        m.canonical("/posts/1");
    });
    let tags = Head::new(&plain, Some(&meta))
        .path("/posts/1?page=2")
        .to_string();
    assert!(!tags.contains("canonical"), "{tags}");
    assert!(!tags.contains("og:url"), "{tags}");
    // A page that names a full URL has one: it needs no base.
    let meta = page(|m| {
        m.canonical("https://elsewhere.example/p/1");
    });
    let tags = Head::new(&plain, Some(&meta)).to_string();
    assert_eq!(
        content(&tags, "canonical"),
        Some("https://elsewhere.example/p/1")
    );
    assert_eq!(
        content(&tags, "og:url"),
        Some("https://elsewhere.example/p/1")
    );
}

#[test]
fn the_canonical_url_is_the_path_under_the_base_or_the_pages_own() {
    let site = Site::new("x").base_url("https://x.example/blog/");
    assert_eq!(site.base(), Some("https://x.example/blog"));
    let head = Head::new(&site, None).path("/about");
    assert_eq!(
        head.canonical().as_deref(),
        Some("https://x.example/blog/about")
    );
    let head = Head::new(&site, None);
    assert_eq!(head.canonical().as_deref(), Some("https://x.example/blog/"));
    let meta = page(|m| {
        m.canonical("posts");
    });
    let head = Head::new(&site, Some(&meta)).path("/posts?page=1");
    assert_eq!(
        head.canonical().as_deref(),
        Some("https://x.example/blog/posts")
    );
    // `base_url_or` does not replace a base the site set.
    let head = Head::new(&site, None)
        .path("/a")
        .base_url_or("http://localhost");
    assert_eq!(
        head.canonical().as_deref(),
        Some("https://x.example/blog/a")
    );
    let head = Head::new(&Site::new("x"), None).path("/a").base_url_or("");
    assert_eq!(head.canonical(), None, "an empty fallback is no base");
}

#[test]
fn the_share_image_is_absolute_when_the_base_allows_it() {
    let meta = page(|m| {
        m.image("img/about.png");
    });
    let tags = Head::new(&site(), Some(&meta)).to_string();
    assert_eq!(
        content(&tags, "og:image"),
        Some("https://blog.example/img/about.png")
    );
    assert_eq!(
        content(&tags, "twitter:image"),
        Some("https://blog.example/img/about.png")
    );
    // A full URL is kept as it is, on another host too.
    let meta = page(|m| {
        m.image("https://cdn.example/a.png");
    });
    let tags = Head::new(&site().image("/og.png"), Some(&meta)).to_string();
    assert_eq!(
        content(&tags, "og:image"),
        Some("https://cdn.example/a.png")
    );
    // Without a base the path is kept: a browser still resolves it.
    let tags = Head::new(&Site::new("x").image("/og.png"), None).to_string();
    assert_eq!(content(&tags, "og:image"), Some("/og.png"));
    assert_eq!(content(&tags, "twitter:card"), Some("summary_large_image"));
    // No image anywhere: a small card and no image tag.
    let tags = Head::new(&site(), None).to_string();
    assert_eq!(content(&tags, "og:image"), None);
    assert_eq!(content(&tags, "twitter:card"), Some("summary"));
}

#[test]
fn each_setting_reaches_its_tag() {
    let site = Site::new("Blog")
        .title_template("%s - Blog")
        .description("Site text")
        .image("/og.png")
        .twitter("@blog");
    let head = Head::new(&site, None);
    assert_eq!(head.title(), "Blog", "a page with no title is the name");
    let tags = head.to_string();
    assert_eq!(content(&tags, "description"), Some("Site text"));
    assert_eq!(content(&tags, "og:description"), Some("Site text"));
    assert_eq!(content(&tags, "twitter:description"), Some("Site text"));
    assert_eq!(content(&tags, "og:site_name"), Some("Blog"));
    assert_eq!(content(&tags, "og:title"), Some("Blog"));
    assert_eq!(content(&tags, "twitter:site"), Some("@blog"));
    assert_eq!(content(&tags, "og:type"), Some("website"));
    assert_eq!(content(&tags, "robots"), None);
    assert_eq!(content(&tags, "og:locale"), None);
    assert_eq!(head.lang_tag(), "en");

    // A page's own values win; the template wraps only `<title>`.
    let meta = page(|m| {
        m.title("About").description("Page text");
    });
    let head = Head::new(&site, Some(&meta));
    assert_eq!(head.title(), "About - Blog");
    let tags = head.to_string();
    assert_eq!(content(&tags, "og:title"), Some("About"));
    assert_eq!(content(&tags, "twitter:title"), Some("About"));
    assert_eq!(content(&tags, "description"), Some("Page text"));

    // No template: the page's title alone. No name: no `og:site_name`.
    let head = Head::new(&Site::new(""), Some(&meta));
    assert_eq!(head.title(), "About");
    assert!(!head.to_string().contains("og:site_name"));
    assert!(!head.to_string().contains("twitter:site"));
}

#[test]
fn a_wrong_base_url_or_template_is_refused_with_help() {
    for base in [
        "blog.example",
        "//blog.example",
        "ftp://blog.example",
        "javascript:alert(1)",
    ] {
        let why = Site::new("x").base_url(base).check().expect_err(base);
        assert!(why.contains("help:"), "{base}: {why}");
    }
    assert!(Site::new("x")
        .base_url("http://localhost:3000")
        .check()
        .is_ok());
    let why = Site::new("Blog")
        .title_template("Blog")
        .check()
        .unwrap_err();
    assert!(why.contains("\"%s | Blog\""), "{why}");
    assert!(Site::new("x").title_template("%s").check().is_ok());
    assert!(Site::new("x").check().is_ok(), "nothing set is fine");
}

/// What `ctx.head()` does with `i18n-routing`, as `src/pages/mod.rs`
/// writes it: the canonical URL keeps the segment, and every offered
/// locale is an alternate, then `x-default`.
fn routed_head(locale: &PathLocale, path: &str) -> Head {
    let mut head = Head::new(&site(), None)
        .path(&locale.href(path))
        .lang(locale.tag());
    for (tag, href) in locale.alternates(path) {
        head = head.alternate(tag, &href);
    }
    head
}

#[test]
fn hreflang_lists_every_locale_itself_included_then_x_default() {
    let locale = PathLocale::new("fr-CA", &["en", "fr", "fr-CA"]);
    let tags = routed_head(&locale, "/about").to_string();
    let alternates: Vec<&str> = tags
        .lines()
        .filter(|l| l.starts_with("<link rel=\"alternate\""))
        .collect();
    assert_eq!(
        alternates,
        [
            r#"<link rel="alternate" hreflang="en" href="https://blog.example/en/about">"#,
            r#"<link rel="alternate" hreflang="fr" href="https://blog.example/fr/about">"#,
            r#"<link rel="alternate" hreflang="fr-CA" href="https://blog.example/fr-CA/about">"#,
            r#"<link rel="alternate" hreflang="x-default" href="https://blog.example/about">"#,
        ]
    );
    assert_eq!(
        content(&tags, "canonical"),
        Some("https://blog.example/fr-CA/about")
    );
    assert_eq!(
        content(&tags, "og:url"),
        Some("https://blog.example/fr-CA/about")
    );
    assert_eq!(content(&tags, "og:locale"), Some("fr_CA"));
}

#[test]
fn the_home_page_of_a_locale_has_alternates_too() {
    let locale = PathLocale::new("en", &["en", "fr"]);
    let tags = routed_head(&locale, "/").to_string();
    assert!(
        tags.contains(r#"<link rel="alternate" hreflang="fr" href="https://blog.example/fr">"#),
        "{tags}"
    );
    assert!(
        tags.contains(
            r#"<link rel="alternate" hreflang="x-default" href="https://blog.example/">"#
        ),
        "{tags}"
    );
    assert_eq!(content(&tags, "canonical"), Some("https://blog.example/en"));
}
