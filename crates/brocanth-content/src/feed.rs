//! An Atom feed of a collection.
//!
//! **Why Atom and not RSS 2.0.** Atom is an IETF standard (RFC 4287): each
//! entry has a required id and `updated` date, a date has one format
//! (RFC 3339), and HTML content says it is HTML. RSS 2.0 leaves each of
//! these to the reader. Every feed reader in use reads Atom, so one format
//! is one URL to announce.
//!
//! **Why absolute URLs.** A feed is read away from the site: a reader
//! resolves nothing against the page that linked it. Every link is the
//! site URL and the path, and each entry's content carries `xml:base`, so
//! a relative link inside a post resolves to the post.

use std::fmt::Write as _;

use crate::{Date, Entry, FrontMatter};

/// An Atom feed, built from a site URL and entries.
#[derive(Debug, Clone)]
pub struct Atom {
    site: String,
    path: String,
    title: String,
    author: String,
    entries: Vec<Item>,
}

#[derive(Debug, Clone)]
struct Item {
    url: String,
    title: String,
    published: Date,
    updated: Date,
    summary: Option<String>,
    html: String,
}

impl Atom {
    /// A feed for the site at `site_url`, served at `path` (`/feed.xml`).
    ///
    /// # Errors
    ///
    /// A site URL that is empty or not an absolute `http` or `https`
    /// address, with a `help:` line naming the setting.
    pub fn new(site_url: &str, path: &str, title: &str, author: &str) -> Result<Self, String> {
        let site = site_url.trim().trim_end_matches('/');
        if site.is_empty() {
            return Err(
                "`site_url` is not set, so the feed cannot write an absolute URL\n\
                 help: set `site_url` under [feed] to the published address, such as \
                 \"https://example.com\", or pass --feed.site_url=https://example.com"
                    .to_string(),
            );
        }
        let host = site
            .strip_prefix("https://")
            .or_else(|| site.strip_prefix("http://"));
        if host.is_none_or(|host| host.is_empty() || host.starts_with('/')) {
            return Err(format!(
                "`site_url` = {site_url:?} is not an absolute http(s) address\n\
                 help: set `site_url` under [feed] to the scheme and the host, such as \
                 \"https://example.com\""
            ));
        }
        Ok(Self {
            site: site.to_string(),
            path: path.to_string(),
            title: title.to_string(),
            author: author.to_string(),
            entries: Vec::new(),
        })
    }

    /// Adds the entries, in the order given, at most `limit` of them. An
    /// entry with no date is left out: Atom needs one.
    #[must_use]
    pub fn entries<'a, T: FrontMatter + 'a>(
        mut self,
        entries: impl IntoIterator<Item = &'a Entry<T>>,
        limit: usize,
    ) -> Self {
        for entry in entries
            .into_iter()
            .filter(|e| e.data.date().is_some())
            .take(limit)
        {
            let published = entry.data.date().expect("filtered on a date");
            self.entries.push(Item {
                url: self.url(&entry.path),
                title: entry.data.title().to_string(),
                published,
                updated: entry.data.updated().unwrap_or(published).max(published),
                summary: entry.data.description().map(str::to_string),
                html: entry.html.as_str().to_string(),
            });
        }
        self
    }

    /// `path` under the site URL.
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.site)
    }

    /// The feed's XML.
    pub fn render(&self) -> String {
        // The feed changed when its newest entry did. With no entry, a
        // fixed early date: Atom needs one, and "now" would change the feed
        // on every start.
        let updated = self
            .entries
            .iter()
            .map(|e| e.updated)
            .max()
            .unwrap_or(Date::new(2000, 1, 1).expect("a real day"));
        let mut xml = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
        xml.push_str("<feed xmlns=\"http://www.w3.org/2005/Atom\">\n");
        let _ = writeln!(xml, "  <title>{}</title>", escape(&self.title));
        let _ = writeln!(xml, "  <id>{}/</id>", escape(&self.site));
        let _ = writeln!(
            xml,
            "  <link rel=\"self\" type=\"application/atom+xml\" href=\"{}\"/>",
            escape(&self.url(&self.path))
        );
        let _ = writeln!(
            xml,
            "  <link rel=\"alternate\" type=\"text/html\" href=\"{}/\"/>",
            escape(&self.site)
        );
        let _ = writeln!(xml, "  <updated>{}</updated>", updated.rfc3339());
        let _ = writeln!(
            xml,
            "  <author><name>{}</name></author>",
            escape(&self.author)
        );
        for item in &self.entries {
            let url = escape(&item.url);
            xml.push_str("  <entry>\n");
            let _ = writeln!(xml, "    <title>{}</title>", escape(&item.title));
            let _ = writeln!(xml, "    <id>{url}</id>");
            let _ = writeln!(
                xml,
                "    <link rel=\"alternate\" type=\"text/html\" href=\"{url}\"/>"
            );
            let _ = writeln!(
                xml,
                "    <published>{}</published>",
                item.published.rfc3339()
            );
            let _ = writeln!(xml, "    <updated>{}</updated>", item.updated.rfc3339());
            if let Some(summary) = &item.summary {
                let _ = writeln!(xml, "    <summary>{}</summary>", escape(summary));
            }
            let _ = writeln!(
                xml,
                "    <content type=\"html\" xml:base=\"{url}\">{}</content>",
                escape(&item.html)
            );
            xml.push_str("  </entry>\n");
        }
        xml.push_str("</feed>\n");
        xml
    }
}

/// Text for XML: `&`, `<`, `>`, `"` and `'` become entities, and a
/// character XML 1.0 forbids (a control character) is dropped.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 || c == '\u{fffe}' || c == '\u{ffff}' => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;
    use crate::{Collection, Source};

    #[derive(Debug, Deserialize)]
    struct Post {
        title: String,
        date: Option<Date>,
        updated: Option<Date>,
        description: Option<String>,
        #[serde(default)]
        draft: bool,
    }

    impl FrontMatter for Post {
        fn title(&self) -> &str {
            &self.title
        }
        fn date(&self) -> Option<Date> {
            self.date
        }
        fn updated(&self) -> Option<Date> {
            self.updated
        }
        fn description(&self) -> Option<&str> {
            self.description.as_deref()
        }
        fn draft(&self) -> bool {
            self.draft
        }
    }

    static SOURCES: &[Source] = &[
        Source {
            path: "content/posts/hello.md",
            slug: "hello",
            locale: None,
            text: "+++\ntitle = \"Tom & Jerry\"\ndate = 2026-11-01\nupdated = 2026-11-20\n\
                   description = \"A <short> one\"\n+++\nSee [about](/about).\n",
        },
        Source {
            path: "content/posts/wip.md",
            slug: "wip",
            locale: None,
            text: "+++\ntitle = \"WIP\"\ndate = 2026-11-21\ndraft = true\n+++\n",
        },
        Source {
            path: "content/posts/undated.md",
            slug: "undated",
            locale: None,
            text: "+++\ntitle = \"Undated\"\n+++\n",
        },
    ];

    fn feed() -> String {
        let posts = Collection::<Post>::parse("/posts", SOURCES).unwrap();
        Atom::new("https://example.com/", "/feed.xml", "My Blog", "Ann")
            .unwrap()
            .entries(posts.published(), 20)
            .render()
    }

    #[test]
    fn every_url_is_absolute_and_every_value_escaped() {
        let xml = feed();
        assert!(xml.starts_with("<?xml"), "{xml}");
        assert!(
            xml.contains("<link rel=\"self\" type=\"application/atom+xml\" href=\"https://example.com/feed.xml\"/>"),
            "{xml}"
        );
        assert!(
            xml.contains("<id>https://example.com/posts/hello</id>"),
            "{xml}"
        );
        assert!(xml.contains("<title>Tom &amp; Jerry</title>"), "{xml}");
        assert!(
            xml.contains("<summary>A &lt;short&gt; one</summary>"),
            "{xml}"
        );
        assert!(
            xml.contains("xml:base=\"https://example.com/posts/hello\">&lt;p&gt;See"),
            "{xml}"
        );
    }

    #[test]
    fn dates_are_rfc3339_and_the_feed_is_as_new_as_its_newest_entry() {
        let xml = feed();
        assert!(
            xml.contains("<published>2026-11-01T00:00:00Z</published>"),
            "{xml}"
        );
        assert!(
            xml.contains("  <updated>2026-11-20T00:00:00Z</updated>\n  <author>"),
            "{xml}"
        );
    }

    #[test]
    fn drafts_and_undated_entries_stay_out() {
        let xml = feed();
        assert!(!xml.contains("WIP"), "{xml}");
        assert!(!xml.contains("Undated"), "{xml}");
        assert_eq!(xml.matches("<entry>").count(), 1, "{xml}");
    }

    #[test]
    fn a_missing_or_relative_site_url_is_refused_with_the_setting() {
        for bad in [
            "",
            "  ",
            "example.com",
            "/blog",
            "https://",
            "ftp://example.com",
        ] {
            let e = Atom::new(bad, "/feed.xml", "t", "a").unwrap_err();
            assert!(
                e.contains("help: set `site_url` under [feed]"),
                "{bad:?}: {e}"
            );
        }
    }

    #[test]
    fn a_control_character_does_not_break_the_xml() {
        assert_eq!(escape("a\u{1}b\tc"), "ab\tc");
    }
}
