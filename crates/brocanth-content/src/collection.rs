//! A collection, read once at start: every entry typed and rendered.

use brocanth_markdown::SafeHtml;

use crate::{Date, FrontMatter, Source};

/// One file of a collection, read.
#[derive(Debug, Clone)]
pub struct Entry<T> {
    /// The URL segment: the file name without `.md` and without a locale.
    pub slug: &'static str,
    /// The locale of a translation, `None` for the default language.
    pub locale: Option<&'static str>,
    /// The page's path: the collection's base and the slug,
    /// `/posts/hello`.
    pub path: String,
    /// The front matter, typed.
    pub data: T,
    /// The body, rendered and cleaned by `brocanth_markdown::render`.
    pub html: SafeHtml,
    /// The file it came from: `content/posts/hello.md`.
    pub source: &'static str,
}

/// Every entry of a collection, newest first.
///
/// Built once, from the files `build.rs` listed. Drafts are in it; a
/// [`View`] decides whether they are shown.
#[derive(Debug, Clone)]
pub struct Collection<T> {
    base: String,
    entries: Vec<Entry<T>>,
}

impl<T: FrontMatter> Collection<T> {
    /// Reads every source: the front matter into `T`, the body to HTML.
    /// `base` is the path the entries are served under, such as `/posts`.
    ///
    /// The order is newest first, by [`FrontMatter::date`]; an entry with
    /// no date comes last; ties go by slug.
    ///
    /// # Errors
    ///
    /// A file whose front matter does not read as `T`, with its path.
    /// `build.rs` checked the same files against the same type, so this
    /// fails only when the two were given different types.
    pub fn parse(base: &str, sources: &'static [Source]) -> Result<Self, String> {
        let base = base.trim_end_matches('/').to_string();
        let mut entries = Vec::with_capacity(sources.len());
        for source in sources {
            let doc = brocanth_markdown::front_matter::<T>(source.text)
                .map_err(|e| format!("{}: {e}", source.path))?;
            entries.push(Entry {
                slug: source.slug,
                locale: source.locale,
                path: format!("{base}/{}", source.slug),
                html: brocanth_markdown::render(doc.body),
                data: doc.data,
                source: source.path,
            });
        }
        entries.sort_by(|a, b| {
            b.data
                .date()
                .cmp(&a.data.date())
                .then_with(|| a.slug.cmp(b.slug))
                .then_with(|| a.locale.cmp(&b.locale))
        });
        Ok(Self { base, entries })
    }

    /// The path the entries are served under: `/posts`.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// Every entry, drafts and translations too, newest first.
    pub fn all(&self) -> &[Entry<T>] {
        &self.entries
    }

    /// The entries a reader may see, in the default language: no draft,
    /// no translation. What a feed and a static export list.
    pub fn published(&self) -> impl Iterator<Item = &Entry<T>> {
        self.entries
            .iter()
            .filter(|entry| entry.locale.is_none() && !entry.data.draft())
    }

    /// The paths a static export writes: the index, each published entry,
    /// the tag index and each tag page. As [`Collection::dated_paths`],
    /// without the dates.
    pub fn paths(&self) -> Vec<String> {
        self.dated_paths()
            .into_iter()
            .map(|(path, _)| path)
            .collect()
    }

    /// Every page of the published collection, with the day it last
    /// changed: what a sitemap lists. No draft and no translation, whatever
    /// a [`View`] shows.
    ///
    /// In order: the index (the newest entry's day), each published entry
    /// (its `updated`, else its `date`), the tag index (the newest tagged
    /// entry's day) and each tag page (its newest entry's day), most used
    /// tag first. `None` where no entry has a date.
    pub fn dated_paths(&self) -> Vec<(String, Option<Date>)> {
        let published: Vec<&Entry<T>> = self.published().collect();
        let newest = |entries: &[&Entry<T>]| entries.iter().filter_map(|e| modified(e)).max();
        let tagged: Vec<&Entry<T>> = published
            .iter()
            .copied()
            .filter(|entry| !entry.data.tags().is_empty())
            .collect();
        let mut out = vec![(self.base.clone(), newest(&published))];
        out.extend(
            published
                .iter()
                .map(|entry| (entry.path.clone(), modified(entry))),
        );
        out.push((self.tags_path(), newest(&tagged)));
        for (tag, _) in count_tags(published.iter().copied()) {
            let with: Vec<&Entry<T>> = tagged
                .iter()
                .copied()
                .filter(|entry| entry.data.tags().iter().any(|t| t == tag))
                .collect();
            out.push((self.tag_path(tag), newest(&with)));
        }
        out
    }

    /// The path of the tag index: `/posts/tags`.
    pub fn tags_path(&self) -> String {
        format!("{}/tags", self.base)
    }

    /// The path of the page that lists the entries with `tag`:
    /// `/posts/tags/rust`. `build.rs` refuses a tag that is not a slug, so
    /// it needs no encoding.
    pub fn tag_path(&self, tag: &str) -> String {
        format!("{}/tags/{tag}", self.base)
    }

    /// The collection as a page sees it: with drafts, or without.
    pub fn view(&self, drafts: bool) -> View<'_, T> {
        View {
            collection: self,
            drafts,
        }
    }
}

/// A collection with one drafts rule, which a page reads.
///
/// A list holds one entry per slug: the translation into the locale asked
/// for when there is one, else the file in the default language.
#[derive(Debug)]
pub struct View<'a, T> {
    collection: &'a Collection<T>,
    drafts: bool,
}

// By hand: a derive would ask `T: Clone`, and a view is two words.
impl<T> Clone for View<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for View<'_, T> {}

impl<'a, T: FrontMatter> View<'a, T> {
    /// Whether this view shows drafts.
    pub fn drafts(&self) -> bool {
        self.drafts
    }

    /// The collection behind it.
    pub fn collection(&self) -> &'a Collection<T> {
        self.collection
    }

    fn shown(&self, entry: &Entry<T>) -> bool {
        self.drafts || !entry.data.draft()
    }

    /// The entries, newest first, one per slug, in `locale` where a
    /// translation exists. `None` lists the default language.
    pub fn list(&self, locale: Option<&str>) -> Vec<&'a Entry<T>> {
        self.listed(locale, |_| true)
    }

    /// The shown entries in the default language that `keep` admits, each
    /// in `locale` where a translation exists.
    fn listed(&self, locale: Option<&str>, keep: impl Fn(&Entry<T>) -> bool) -> Vec<&'a Entry<T>> {
        self.collection
            .entries
            .iter()
            .filter(|entry| entry.locale.is_none() && self.shown(entry) && keep(entry))
            .map(|default| {
                locale
                    .and_then(|wanted| self.translation(default.slug, wanted))
                    .unwrap_or(default)
            })
            .collect()
    }

    /// The entry at `slug`, in `locale` where a translation exists. `None`
    /// when there is none, or when it is a draft this view hides.
    pub fn get(&self, slug: &str, locale: Option<&str>) -> Option<&'a Entry<T>> {
        let default = self
            .collection
            .entries
            .iter()
            .find(|entry| entry.slug == slug && entry.locale.is_none())
            .filter(|entry| self.shown(entry))?;
        Some(
            locale
                .and_then(|wanted| self.translation(slug, wanted))
                .unwrap_or(default),
        )
    }

    /// The entries with `tag`, as [`View::list`] gives them.
    ///
    /// The file in the default language decides: a tag belongs to the
    /// entry, so a tag page lists the same entries in every locale, and
    /// shows each one's translation where there is one.
    pub fn tagged(&self, tag: &str, locale: Option<&str>) -> Vec<&'a Entry<T>> {
        self.listed(locale, |entry| entry.data.tags().iter().any(|t| t == tag))
    }

    /// Every tag of the listed entries, with how many entries carry it,
    /// most used first.
    pub fn tags(&self) -> Vec<(&'a str, usize)> {
        count_tags(self.list(None))
    }

    fn translation(&self, slug: &str, locale: &str) -> Option<&'a Entry<T>> {
        self.collection
            .entries
            .iter()
            .find(|entry| entry.slug == slug && entry.locale == Some(locale))
            .filter(|entry| self.shown(entry))
    }
}

/// The day an entry last changed: `updated`, else `date`.
fn modified<T: FrontMatter>(entry: &Entry<T>) -> Option<Date> {
    entry.data.updated().or_else(|| entry.data.date())
}

/// Every tag of `entries`, with how many carry it, most used first, then by
/// name.
fn count_tags<'a, T: FrontMatter + 'a>(
    entries: impl IntoIterator<Item = &'a Entry<T>>,
) -> Vec<(&'a str, usize)> {
    let mut counts: Vec<(&'a str, usize)> = Vec::new();
    for entry in entries {
        for tag in entry.data.tags() {
            match counts.iter_mut().find(|(t, _)| *t == tag.as_str()) {
                Some((_, n)) => *n += 1,
                None => counts.push((tag.as_str(), 1)),
            }
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    counts
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;
    use crate::Date;

    #[derive(Debug, Deserialize)]
    struct Post {
        title: String,
        date: Option<Date>,
        updated: Option<Date>,
        #[serde(default)]
        tags: Vec<String>,
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
        fn tags(&self) -> &[String] {
            &self.tags
        }
        fn draft(&self) -> bool {
            self.draft
        }
    }

    const fn source(
        slug: &'static str,
        locale: Option<&'static str>,
        text: &'static str,
    ) -> Source {
        Source {
            path: "content/posts/x.md",
            slug,
            locale,
            text,
        }
    }

    static SOURCES: &[Source] = &[
        source(
            "old",
            None,
            "+++\ntitle = \"Old\"\ndate = 2025-01-01\nupdated = 2026-03-01\ntags = [\"rust\"]\n+++\nOld *body*\n",
        ),
        source(
            "new",
            None,
            "+++\ntitle = \"New\"\ndate = 2026-01-01\ntags = [\"rust\", \"web\"]\n+++\n",
        ),
        source(
            "new",
            Some("fr"),
            "+++\ntitle = \"Nouveau\"\ndate = 2026-01-01\n+++\n",
        ),
        source(
            "wip",
            None,
            "+++\ntitle = \"WIP\"\ndate = 2026-06-01\ntags = [\"draft-only\"]\ndraft = true\n+++\n",
        ),
        source("undated", None, "+++\ntitle = \"Undated\"\n+++\n"),
    ];

    fn posts() -> Collection<Post> {
        Collection::parse("/posts/", SOURCES).unwrap()
    }

    fn titles(entries: &[&Entry<Post>]) -> Vec<String> {
        entries.iter().map(|e| e.data.title.clone()).collect()
    }

    #[test]
    fn the_list_is_newest_first_and_hides_drafts_unless_asked() {
        let posts = posts();
        assert_eq!(
            titles(&posts.view(false).list(None)),
            ["New", "Old", "Undated"]
        );
        assert_eq!(
            titles(&posts.view(true).list(None)),
            ["WIP", "New", "Old", "Undated"]
        );
        assert!(posts.view(false).get("wip", None).is_none());
        assert!(posts.view(true).get("wip", None).is_some());
    }

    #[test]
    fn a_locale_takes_its_translation_and_falls_back_to_the_default() {
        let posts = posts();
        let view = posts.view(false);
        assert_eq!(
            titles(&view.list(Some("fr"))),
            ["Nouveau", "Old", "Undated"]
        );
        assert_eq!(view.get("new", Some("de")).unwrap().data.title, "New");
        assert_eq!(view.get("new", Some("fr")).unwrap().data.title, "Nouveau");
    }

    #[test]
    fn an_entry_has_its_path_and_its_rendered_body() {
        let posts = posts();
        let old = posts.view(false).get("old", None).unwrap();
        assert_eq!(old.path, "/posts/old");
        assert_eq!(old.html.as_str(), "<p>Old <em>body</em></p>\n");
    }

    #[test]
    fn published_and_paths_leave_out_drafts_and_translations() {
        let posts = posts();
        assert_eq!(
            posts.paths(),
            [
                "/posts",
                "/posts/new",
                "/posts/old",
                "/posts/undated",
                "/posts/tags",
                "/posts/tags/rust",
                "/posts/tags/web",
            ]
        );
    }

    #[test]
    fn each_published_page_carries_the_day_it_last_changed() {
        let posts = posts();
        let day = |text: &str| Some(text.parse::<Date>().unwrap());
        let dated = posts.dated_paths();
        let of = |path: &str| {
            dated
                .iter()
                .find(|(p, _)| p == path)
                .map(|(_, d)| *d)
                .unwrap_or_else(|| panic!("{path} in {dated:?}"))
        };
        // `updated` wins over `date`; an undated entry has no day.
        assert_eq!(of("/posts/old"), day("2026-03-01"));
        assert_eq!(of("/posts/new"), day("2026-01-01"));
        assert_eq!(of("/posts/undated"), None);
        // An index takes its newest entry's day; the draft counts for none.
        assert_eq!(of("/posts"), day("2026-03-01"));
        assert_eq!(of("/posts/tags"), day("2026-03-01"));
        assert_eq!(of("/posts/tags/rust"), day("2026-03-01"));
        assert_eq!(of("/posts/tags/web"), day("2026-01-01"));
        assert!(dated.iter().all(|(p, _)| !p.contains("draft-only")));
    }

    #[test]
    fn tags_count_the_listed_entries() {
        let posts = posts();
        let view = posts.view(false);
        assert_eq!(view.tags(), [("rust", 2), ("web", 1)]);
        assert_eq!(titles(&view.tagged("web", None)), ["New"]);
        // A draft's tag shows only where drafts do.
        assert!(view.tagged("draft-only", None).is_empty());
        assert_eq!(
            titles(&posts.view(true).tagged("draft-only", None)),
            ["WIP"]
        );
        assert_eq!(posts.tag_path("rust"), "/posts/tags/rust");
        assert_eq!(posts.tags_path(), "/posts/tags");
    }

    #[test]
    fn a_tag_page_lists_the_translation_of_each_tagged_entry() {
        let posts = posts();
        // `new.fr.md` has no tags; `new.md` decides.
        assert_eq!(
            titles(&posts.view(false).tagged("web", Some("fr"))),
            ["Nouveau"]
        );
    }

    #[test]
    fn a_front_matter_the_type_refuses_names_the_file() {
        static BAD: &[Source] = &[source("bad", None, "+++\ndate = 2026-01-01\n+++\n")];
        let e = Collection::<Post>::parse("/posts", BAD).unwrap_err();
        assert!(e.starts_with("content/posts/x.md: "), "{e}");
        assert!(e.contains("help: "), "{e}");
    }
}
