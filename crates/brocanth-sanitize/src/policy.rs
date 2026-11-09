//! The policies: one per use, each an allow-list.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use crate::SafeHtml;

/// The tags a comment keeps: emphasis, links, lists, quotes and code.
const COMMENT_TAGS: &[&str] = &[
    "a",
    "b",
    "blockquote",
    "br",
    "code",
    "del",
    "em",
    "i",
    "li",
    "ol",
    "p",
    "pre",
    "s",
    "strong",
    "ul",
];

/// What rich text adds: headings, images, tables and a few inline tags.
/// No `h1`: the page owns its title.
const RICH_TEXT_TAGS: &[&str] = &[
    "abbr",
    "cite",
    "figcaption",
    "figure",
    "h2",
    "h3",
    "h4",
    "hr",
    "img",
    "mark",
    "q",
    "sub",
    "sup",
    "table",
    "tbody",
    "td",
    "th",
    "thead",
    "tr",
];

/// The schemes a link or an image may use. No `javascript:`, no `data:`,
/// no `vbscript:`: each runs code or carries a document in some browser.
const SCHEMES: &[&str] = &["http", "https", "mailto"];

/// An allow-list of tags, attributes and URL schemes.
///
/// Build one per use, once, and keep it: a policy is a set of tables, and
/// [`comment`], [`rich_text`] and [`text`] keep theirs in a `static`.
///
/// What every policy here does, whatever the list:
///
/// - `<script>` and `<style>` go with their content, not only their tags.
/// - No `style`, `class` or `id` attribute: `style` is CSS injection,
///   `class` borrows the page's own styles to fake its interface, and `id`
///   can shadow a global name a script reads (DOM clobbering).
/// - No `on*` attribute, ever: ammonia has no way to allow one.
/// - A URL must be relative or use `http`, `https` or `mailto`.
/// - A link gets a `rel` the policy sets, which replaces the user's.
/// - Comments go.
pub struct Policy {
    builder: ammonia::Builder<'static>,
}

/// A builder that keeps nothing.
///
/// `ammonia::Builder::empty()` empties the tags only. It keeps ammonia's
/// default attributes: `lang` and `title` on every tag, and `cite` on
/// `blockquote`, `del`, `ins` and `q`. ammonia does not read `cite` as a
/// URL, so `<blockquote cite="javascript:..">` would pass the scheme check.
/// Every attribute a policy keeps is named in that policy.
fn bare() -> ammonia::Builder<'static> {
    let mut builder = ammonia::Builder::empty();
    builder
        .generic_attributes(HashSet::new())
        .tag_attributes(HashMap::new())
        .tag_attribute_values(HashMap::new());
    builder
}

impl Policy {
    /// A comment: `a`, `b`, `blockquote`, `br`, `code`, `del`, `em`, `i`,
    /// `li`, `ol`, `p`, `pre`, `s`, `strong`, `ul`.
    ///
    /// A link keeps `href` and `title`, and gets
    /// `rel="nofollow ugc noopener noreferrer"`: a link a visitor wrote
    /// passes no ranking (`ugc`, `nofollow`) and no window handle or
    /// referrer to the target.
    pub fn comment() -> Self {
        let mut builder = bare();
        builder
            .add_tags(COMMENT_TAGS.iter().copied())
            .add_tag_attributes("a", ["href", "title"])
            .url_schemes(SCHEMES.iter().copied().collect())
            .link_rel(Some("nofollow ugc noopener noreferrer"))
            .strip_comments(true)
            .clean_content_tags(["script", "style"].into_iter().collect());
        Self { builder }
    }

    /// Rich text from an editor: everything a comment has, and `h2` to
    /// `h4`, `hr`, `img`, `figure`, `figcaption`, tables, `abbr`, `cite`,
    /// `mark`, `q`, `sub` and `sup`.
    ///
    /// An image keeps `src`, `alt`, `title`, `width` and `height`. A table
    /// cell keeps `colspan`, `rowspan` and, on `th`, `scope`. `q` keeps no
    /// `cite`: ammonia checks the scheme of `href` and `src` but does not
    /// read `cite` as a URL, so a `javascript:` value would pass. A link gets
    /// `rel="noopener noreferrer"`: rich text is usually written by the
    /// site's own editors, so its links may pass ranking.
    pub fn rich_text() -> Self {
        let mut policy = Self::comment();
        policy
            .builder
            .add_tags(RICH_TEXT_TAGS.iter().copied())
            .add_tag_attributes("img", ["src", "alt", "title", "width", "height"])
            .add_tag_attributes("td", ["colspan", "rowspan"])
            .add_tag_attributes("th", ["colspan", "rowspan", "scope"])
            .add_tag_attributes("abbr", ["title"])
            .link_rel(Some("noopener noreferrer"));
        policy
    }

    /// No tags at all: the text stays, every tag goes, and the content of
    /// `<script>` and `<style>` goes with it. For a title or a name that
    /// came in as HTML.
    pub fn text() -> Self {
        let mut builder = bare();
        builder
            .strip_comments(true)
            .clean_content_tags(["script", "style"].into_iter().collect());
        Self { builder }
    }

    /// A policy from an ammonia builder, for a use the three do not fit.
    ///
    /// `ammonia::Builder::default()` is ammonia's own list, which is wider
    /// than [`Policy::rich_text`]. Start from [`Policy::comment`] or
    /// [`Policy::rich_text`] and add to it instead where you can.
    pub fn from_builder(builder: ammonia::Builder<'static>) -> Self {
        Self { builder }
    }

    /// Allows more tags.
    #[must_use]
    pub fn allow_tags(mut self, tags: impl IntoIterator<Item = &'static str>) -> Self {
        self.builder.add_tags(tags);
        self
    }

    /// Allows more attributes on one tag.
    #[must_use]
    pub fn allow_attributes(
        mut self,
        tag: &'static str,
        attributes: impl IntoIterator<Item = &'static str>,
    ) -> Self {
        self.builder.add_tag_attributes(tag, attributes);
        self
    }

    /// Removes tags from the list.
    #[must_use]
    pub fn deny_tags(mut self, tags: impl IntoIterator<Item = &'static str>) -> Self {
        self.builder.rm_tags(tags);
        self
    }

    /// The `rel` every kept link gets. `None` keeps no `rel` at all.
    #[must_use]
    pub fn link_rel(mut self, rel: Option<&'static str>) -> Self {
        self.builder.link_rel(rel);
        self
    }

    /// The URL schemes a link or an image may use, in place of `http`,
    /// `https` and `mailto`.
    #[must_use]
    pub fn url_schemes(mut self, schemes: impl IntoIterator<Item = &'static str>) -> Self {
        self.builder
            .url_schemes(schemes.into_iter().collect::<HashSet<_>>());
        self
    }

    /// Cleans `html`: what the list allows stays, the rest goes.
    pub fn clean(&self, html: &str) -> SafeHtml {
        SafeHtml::cleaned(self.builder.clean(html).to_string())
    }
}

static COMMENT: LazyLock<Policy> = LazyLock::new(Policy::comment);
static RICH_TEXT: LazyLock<Policy> = LazyLock::new(Policy::rich_text);
static TEXT: LazyLock<Policy> = LazyLock::new(Policy::text);

/// Cleans `html` with [`Policy::comment`].
pub fn comment(html: &str) -> SafeHtml {
    COMMENT.clean(html)
}

/// Cleans `html` with [`Policy::rich_text`].
pub fn rich_text(html: &str) -> SafeHtml {
    RICH_TEXT.clean(html)
}

/// Cleans `html` with [`Policy::text`].
pub fn text(html: &str) -> SafeHtml {
    TEXT.clean(html)
}
