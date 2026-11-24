//! Markdown to HTML, then the HTML through an allow-list.
//!
//! **Why pulldown-cmark.** It implements CommonMark and the GFM tables,
//! footnotes, strikethrough and task lists; rustdoc and mdBook render with
//! it; it is MIT and pulls three small crates. `comrak` covers more of GFM
//! (autolinks without `<>`), but it is BSD-2-Clause and larger, and this
//! workspace takes MIT or Apache.
//!
//! **Why clean HTML that this crate wrote.** Markdown passes raw HTML
//! through, and a link may carry `javascript:`. The parser does not know
//! which tags a page allows, so every result goes through
//! `brocanth-sanitize`, the same allow-list a comment goes through, with
//! the few attributes Markdown itself writes.
//!
//! **Why no syntax highlighting by default.** A highlighter is a grammar per
//! language (megabytes for `syntect`) and writes either inline `style`,
//! which no policy here keeps, or classes that need a stylesheet. A code
//! block keeps `class="language-rust"`, which a client highlighter reads.
//! [`Markdown::highlight`] plugs in a server-side one, with classes that
//! start with `hl-`.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};

use brocanth_sanitize::{ammonia, Policy, SafeHtml};
use pulldown_cmark::{CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd};

/// What a code highlighter is: the language from the fence (empty when
/// none) and the code, to HTML. `None` leaves the block as plain text.
type Highlighter = dyn Fn(&str, &str) -> Option<String> + Send + Sync;

/// A Markdown renderer: parser options, heading ids and the policy the
/// HTML goes through.
///
/// Build one per use and keep it in a `static`: the policy is a set of
/// tables. [`render`] and [`render_user`] keep theirs.
pub struct Markdown {
    policy: Policy,
    heading_ids: bool,
    highlight: Option<Arc<Highlighter>>,
    user: bool,
}

/// The tags Markdown writes, and the inline tags an author writes in raw
/// HTML. No `script`, `style`, `iframe`, `form` or `object`.
const TAGS: &[&str] = &[
    "a",
    "abbr",
    "b",
    "blockquote",
    "br",
    "code",
    "dd",
    "del",
    "details",
    "div",
    "dl",
    "dt",
    "em",
    "figcaption",
    "figure",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "img",
    "input",
    "kbd",
    "li",
    "mark",
    "ol",
    "p",
    "pre",
    "q",
    "s",
    "span",
    "strong",
    "sub",
    "summary",
    "sup",
    "table",
    "tbody",
    "td",
    "th",
    "thead",
    "tr",
    "ul",
];

/// The classes Markdown writes on a footnote.
const FOOTNOTE_CLASSES: &[&str] = &[
    "footnote-reference",
    "footnote-definition",
    "footnote-definition-label",
];

/// The schemes a link or an image may use.
const SCHEMES: &[&str] = &["http", "https", "mailto"];

/// The prefix of every id in text a visitor wrote, as GitHub writes it, so
/// a heading cannot shadow a global name a script reads.
const USER_PREFIX: &str = "user-content-";

impl Markdown {
    /// Text the site's own authors wrote: a post, a page, a README.
    ///
    /// Headings get ids from their text (`## Getting started` is
    /// `id="getting-started"`), so `#getting-started` links work. A link
    /// gets `rel="noopener noreferrer"` and keeps its ranking.
    pub fn authored() -> Self {
        Self {
            policy: policy(false, false),
            heading_ids: true,
            highlight: None,
            user: false,
        }
    }

    /// Text a visitor wrote: a comment, a profile, an issue.
    ///
    /// Every id starts with `user-content-`, and a `#name` link is
    /// rewritten to match, so a heading cannot clobber a global name. A
    /// link gets `rel="nofollow ugc noopener noreferrer"`. No `h1`: the
    /// page owns its title.
    pub fn user() -> Self {
        Self {
            policy: policy(true, false),
            heading_ids: true,
            highlight: None,
            user: true,
        }
    }

    /// Without ids on headings.
    #[must_use]
    pub fn without_heading_ids(mut self) -> Self {
        self.heading_ids = false;
        self
    }

    /// A server-side highlighter for fenced code: `(language, code)` to the
    /// HTML of the code, or `None` for plain text.
    ///
    /// The result is cleaned like the rest. It keeps `<span class>` where
    /// every class starts with `hl-`; any other class goes. `syntect`'s
    /// `ClassedHTMLGenerator` with `ClassStyle::SpacedPrefixed { prefix:
    /// "hl-" }` writes that, and the stylesheet comes from its theme.
    #[must_use]
    pub fn highlight(
        mut self,
        highlighter: impl Fn(&str, &str) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        self.policy = policy(self.user, true);
        self.highlight = Some(Arc::new(highlighter));
        self
    }

    /// Renders `text` and cleans the HTML.
    pub fn render(&self, text: &str) -> SafeHtml {
        let mut options = Options::empty();
        options.insert(Options::ENABLE_TABLES);
        options.insert(Options::ENABLE_FOOTNOTES);
        options.insert(Options::ENABLE_STRIKETHROUGH);
        options.insert(Options::ENABLE_TASKLISTS);
        let events: Vec<Event<'_>> = Parser::new_ext(text, options).collect();
        let events = if self.heading_ids {
            with_heading_ids(events)
        } else {
            events
        };
        let events = match &self.highlight {
            Some(highlight) => highlighted(events, highlight.as_ref()),
            None => events,
        };
        let mut html = String::with_capacity(text.len() * 3 / 2);
        pulldown_cmark::html::push_html(&mut html, events.into_iter());
        self.policy.clean(&html)
    }
}

static AUTHORED: LazyLock<Markdown> = LazyLock::new(Markdown::authored);
static USER: LazyLock<Markdown> = LazyLock::new(Markdown::user);

/// Renders text the site's authors wrote, with [`Markdown::authored`].
pub fn render(text: &str) -> SafeHtml {
    AUTHORED.render(text)
}

/// Renders text a visitor wrote, with [`Markdown::user`].
pub fn render_user(text: &str) -> SafeHtml {
    USER.render(text)
}

/// The allow-list. `user` prefixes ids and marks links `ugc`;
/// `highlighted` keeps `hl-` classes on `span`.
fn policy(user: bool, highlighted: bool) -> Policy {
    let mut builder = ammonia::Builder::empty();
    // `Builder::empty` keeps ammonia's default attributes (`lang`, `title`,
    // `cite`); name every one instead, as brocanth-sanitize does.
    builder
        .generic_attributes(HashSet::new())
        .tag_attributes(HashMap::new())
        .tag_attribute_values(HashMap::new())
        .add_tags(TAGS.iter().copied())
        .add_tag_attributes("a", ["href", "title"])
        .add_tag_attributes("img", ["src", "alt", "title", "width", "height"])
        .add_tag_attributes("ol", ["start"])
        .add_tag_attributes("td", ["colspan", "rowspan", "style"])
        .add_tag_attributes("th", ["colspan", "rowspan", "scope", "style"])
        .add_tag_attributes("abbr", ["title"])
        .add_tag_attributes("input", ["type", "checked", "disabled"])
        .add_tag_attributes("code", ["class"])
        .add_tag_attributes("sup", ["class", "id"])
        .add_tag_attributes("div", ["class", "id"])
        .add_tag_attributes("li", ["id"])
        .url_schemes(SCHEMES.iter().copied().collect())
        .strip_comments(true)
        .clean_content_tags(["script", "style"].into_iter().collect());
    for h in ["h1", "h2", "h3", "h4", "h5", "h6"] {
        builder.add_tag_attributes(h, ["id"]);
    }
    if highlighted {
        builder.add_tag_attributes("span", ["class"]);
    }
    if user {
        builder
            .rm_tags(["h1"])
            .link_rel(Some("nofollow ugc noopener noreferrer"));
    } else {
        builder.link_rel(Some("noopener noreferrer"));
    }
    builder
        .attribute_filter(move |element, attribute, value| filter(user, element, attribute, value));
    Policy::from_builder(builder)
}

/// The value an attribute keeps, or `None` to drop it.
///
/// Each attribute Markdown writes with a value from the text is held to the
/// shape Markdown writes: a class from a short list or with a known prefix,
/// a table cell's alignment and nothing else in its `style`, a checkbox.
fn filter<'u>(user: bool, element: &str, attribute: &str, value: &'u str) -> Option<Cow<'u, str>> {
    match (element, attribute) {
        ("code", "class") => language_class(value).then_some(Cow::Borrowed(value)),
        ("sup" | "div", "class") => FOOTNOTE_CLASSES
            .contains(&value)
            .then_some(Cow::Borrowed(value)),
        ("span", "class") => {
            let kept: Vec<&str> = value
                .split_ascii_whitespace()
                .filter(|class| class.starts_with("hl-") && plain(class))
                .collect();
            (!kept.is_empty()).then(|| Cow::Owned(kept.join(" ")))
        }
        ("td" | "th", "style") => matches!(
            value,
            "text-align: left" | "text-align: center" | "text-align: right"
        )
        .then_some(Cow::Borrowed(value)),
        ("input", "type") => (value == "checkbox").then_some(Cow::Borrowed(value)),
        (_, "id") if user => Some(prefixed(value)),
        ("a", "href") if user && value.starts_with('#') => {
            Some(Cow::Owned(format!("#{}", prefixed(&value[1..]))))
        }
        _ => Some(Cow::Borrowed(value)),
    }
}

/// `value` with the user prefix, once.
fn prefixed(value: &str) -> Cow<'_, str> {
    if value.starts_with(USER_PREFIX) {
        Cow::Borrowed(value)
    } else {
        Cow::Owned(format!("{USER_PREFIX}{value}"))
    }
}

/// Whether `value` is the class a fenced block writes: `language-rust`.
fn language_class(value: &str) -> bool {
    value
        .strip_prefix("language-")
        .is_some_and(|lang| !lang.is_empty() && plain(lang))
}

/// Letters, digits, `-`, `_`, `+` and `#`: a language or a class name.
fn plain(text: &str) -> bool {
    text.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '#'))
}

/// The events, with an `id` on every heading that has none, from its text.
/// Two headings with one text get `-1`, `-2` after the first.
fn with_heading_ids(events: Vec<Event<'_>>) -> Vec<Event<'_>> {
    // The text of each heading, in order.
    let mut texts: Vec<String> = Vec::new();
    let mut inside: Option<String> = None;
    for event in &events {
        match event {
            Event::Start(Tag::Heading { .. }) => inside = Some(String::new()),
            Event::End(TagEnd::Heading(_)) => texts.extend(inside.take()),
            Event::Text(text) | Event::Code(text) => {
                if let Some(buffer) = inside.as_mut() {
                    buffer.push_str(text);
                }
            }
            _ => {}
        }
    }
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut ids = texts.iter().map(|text| {
        let base = slug(text);
        let count = seen.entry(base.clone()).or_insert(0);
        let id = if *count == 0 {
            base.clone()
        } else {
            format!("{base}-{count}")
        };
        *count += 1;
        id
    });
    events
        .into_iter()
        .map(|event| match event {
            Event::Start(Tag::Heading {
                level,
                id,
                classes,
                attrs,
            }) => {
                let next = ids.next();
                Event::Start(Tag::Heading {
                    level,
                    id: id.or_else(|| next.map(CowStr::from)),
                    classes,
                    attrs,
                })
            }
            other => other,
        })
        .collect()
}

/// An id from a heading's text: lower case, letters and digits kept,
/// every run of anything else one `-`. `section` when nothing is left.
fn slug(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut dash = false;
    for c in text.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "section".to_string()
    } else {
        out
    }
}

/// The events, with each fenced code block the highlighter takes replaced
/// by its HTML inside `<pre><code class="language-x">`.
fn highlighted<'a>(events: Vec<Event<'a>>, highlight: &Highlighter) -> Vec<Event<'a>> {
    let mut out = Vec::with_capacity(events.len());
    let mut block: Option<(String, String)> = None;
    for event in events {
        match (&mut block, event) {
            (None, Event::Start(Tag::CodeBlock(kind))) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split_ascii_whitespace()
                        .next()
                        .unwrap_or("")
                        .to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                block = Some((lang, String::new()));
            }
            (Some((_, code)), Event::Text(text)) => code.push_str(&text),
            (Some(_), Event::End(TagEnd::CodeBlock)) => {
                let (lang, code) = block.take().expect("inside a block");
                let class = if lang.is_empty() {
                    String::new()
                } else {
                    format!(" class=\"language-{}\"", escape(&lang))
                };
                let inner = highlight(&lang, &code).unwrap_or_else(|| escape(&code));
                out.push(Event::Html(CowStr::from(format!(
                    "<pre><code{class}>{inner}</code></pre>\n"
                ))));
            }
            (Some(_), _) => {}
            (None, other) => out.push(other),
        }
    }
    out
}

/// Text as HTML: `<`, `>`, `&` and `"` become entities.
fn escape(text: &str) -> String {
    SafeHtml::escape(text).into_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slug_keeps_letters_and_digits_and_joins_the_rest() {
        assert_eq!(slug("Getting started!"), "getting-started");
        assert_eq!(slug("  `fn main()` -- why?  "), "fn-main-why");
        assert_eq!(slug("Été 2026"), "été-2026");
        assert_eq!(slug("!!!"), "section");
    }

    #[test]
    fn a_language_class_is_a_prefix_and_a_plain_name() {
        assert!(language_class("language-rust"));
        assert!(language_class("language-c++"));
        assert!(!language_class("language-"));
        assert!(!language_class("language-x\" onclick=\"y"));
        assert!(!language_class("btn"));
    }
}
