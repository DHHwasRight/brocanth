//! Questions about a page, answered without a substring search.
//!
//! # Why this is not a parsing crate
//!
//! §G6 exists because three rounds of tests in one afternoon asserted on CSS
//! class names. The fix is not a better string search, it is a level at which
//! a test can say *"there is a link to `/admin/post`"* instead of *"the bytes
//! `/admin/post` occur somewhere"*. The second passes when the path is in a
//! comment, in a `data-` attribute, or in the `<style>` block.
//!
//! Every question the tests in this repository actually ask is of one shape:
//! find the elements called `a`, read their `href`; find the `form`, read its
//! `method` and `action`; find the fields, read their `name`; take the inner
//! text of one element. Tag, attribute, inner text, and a region scoped by an
//! attribute. That is a scanner, not a parser, and
//! `brocanth-admin/tests/panels.rs` already has a twenty-line one
//! (`current_tabs`) because the need is real and the shape is small.
//!
//! A spec parser would buy CSS selectors and tree repair. The selectors are
//! worth something; the repair is a liability. `html5ever` inserts the
//! `<tbody>` the framework did not emit, closes the `<a>` a template left
//! open, and moves stray content out of `<table>` -- so a test passes against
//! a repaired tree while a browser shows something else, and the broken
//! markup is never reported. A scanner reads what was written, which is what
//! a test of a renderer wants.
//!
//! So: no dependency. The line to change if that stops being true is an
//! assertion that genuinely needs descendant combinators or `nth-child`.
//!
//! # What this does not do
//!
//! It builds no tree, runs no scripts, and infers no omitted end tag. Every
//! element it reports was spelled in the source. [`Html::within`] and
//! [`Element::text`] find a closing tag by counting the depth of that same tag
//! name, so an element whose end tag the source omits has no inner text here
//! -- which is the honest answer rather than a guess. Comments, `<script>` and
//! `<style>` are skipped, so markup quoted inside them is not found.
//!
//! Attribute values and text come back with `&amp;`, `&lt;`, `&gt;`,
//! `&quot;`, `&apos;`, `&nbsp;` and numeric references decoded, so a test
//! writes the URL a browser would follow -- `"?sort=views&dir=asc"` -- and not
//! the markup that encodes it.

use std::fmt::Write as _;

/// A page, or a region of one.
///
/// Owned, because a test holds it across assertions and nobody is counting
/// allocations here.
#[derive(Debug, Clone)]
pub struct Html {
    source: String,
}

/// One element the scanner found.
#[derive(Debug, Clone)]
pub struct Element {
    name: String,
    attrs: Vec<(String, String)>,
    inner: Option<String>,
}

impl Element {
    /// The tag name, lowercased.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// One attribute by name, decoded. `None` when the element does not carry
    /// it; `Some("")` when it carries it with no value, as `dp-boost` does.
    #[must_use]
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// Whether the element carries an attribute at all.
    #[must_use]
    pub fn has_attr(&self, name: &str) -> bool {
        self.attr(name).is_some()
    }

    /// Every attribute, in source order.
    #[must_use]
    pub fn attrs(&self) -> &[(String, String)] {
        &self.attrs
    }

    /// `href`, which is the attribute most assertions are about.
    #[must_use]
    pub fn href(&self) -> Option<&str> {
        self.attr("href")
    }

    /// The markup between this element's tags, if the source closed it.
    #[must_use]
    pub fn inner_html(&self) -> Option<&str> {
        self.inner.as_deref()
    }

    /// The text a reader would see inside this element: tags removed,
    /// entities decoded, runs of whitespace collapsed to one space.
    #[must_use]
    pub fn text(&self) -> String {
        self.inner.as_deref().map(text_of).unwrap_or_default()
    }

    /// The element's own contents as a region to ask further questions of.
    #[must_use]
    pub fn html(&self) -> Html {
        Html::new(self.inner.clone().unwrap_or_default())
    }
}

impl Html {
    /// A region from markup.
    #[must_use]
    pub fn new(source: impl Into<String>) -> Self {
        Html {
            source: source.into(),
        }
    }

    /// The markup this region holds.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The text a reader would see: tags removed, entities decoded, runs of
    /// whitespace collapsed to one space.
    #[must_use]
    pub fn text(&self) -> String {
        text_of(&self.source)
    }

    // -----------------------------------------------------------------
    // Finding elements
    // -----------------------------------------------------------------

    /// Every element with this tag name, in source order.
    #[must_use]
    pub fn tags(&self, name: &str) -> Vec<Element> {
        scan(&self.source, &|found: &str| {
            found.eq_ignore_ascii_case(name)
        })
    }

    /// Every `<a>`.
    #[must_use]
    pub fn links(&self) -> Vec<Element> {
        self.tags("a")
    }

    /// Every `<a>` carrying `attr="value"`.
    ///
    /// *"Which section am I in"* is `links_with("aria-current", "page")`,
    /// which is a fact about the page. The class name that styles it is a
    /// fact about the stylesheet, and asserting on one is the failure §G6
    /// names.
    #[must_use]
    pub fn links_with(&self, attr: &str, value: &str) -> Vec<Element> {
        self.links()
            .into_iter()
            .filter(|link| link.attr(attr) == Some(value))
            .collect()
    }

    /// Every `<form>`.
    #[must_use]
    pub fn forms(&self) -> Vec<Element> {
        self.tags("form")
    }

    /// Every `<input>`, `<select>` and `<textarea>` that carries a `name`.
    #[must_use]
    pub fn fields(&self) -> Vec<Element> {
        scan(&self.source, &|found: &str| {
            ["input", "select", "textarea"]
                .iter()
                .any(|kind| found.eq_ignore_ascii_case(kind))
        })
        .into_iter()
        .filter(|field| field.has_attr("name"))
        .collect()
    }

    /// Every `<button>`.
    #[must_use]
    pub fn buttons(&self) -> Vec<Element> {
        self.tags("button")
    }

    /// The first link whose `href` is exactly `href`.
    #[must_use]
    pub fn link_to(&self, href: &str) -> Option<Element> {
        self.links().into_iter().find(|a| a.href() == Some(href))
    }

    /// Whether any link points at `href`.
    #[must_use]
    pub fn has_link_to(&self, href: &str) -> bool {
        self.link_to(href).is_some()
    }

    /// The first form that submits to `action` with `method="post"`.
    ///
    /// A form with no `method` is a GET, which is what the HTML default says
    /// and what a browser does, so it is not reported here.
    #[must_use]
    pub fn form_posting_to(&self, action: &str) -> Option<Element> {
        self.forms().into_iter().find(|form| {
            form.attr("action") == Some(action)
                && form
                    .attr("method")
                    .is_some_and(|m| m.eq_ignore_ascii_case("post"))
        })
    }

    /// The first field called `name`.
    #[must_use]
    pub fn field(&self, name: &str) -> Option<Element> {
        self.fields()
            .into_iter()
            .find(|field| field.attr("name") == Some(name))
    }

    /// The first button whose text is `label`.
    #[must_use]
    pub fn button(&self, label: &str) -> Option<Element> {
        self.buttons()
            .into_iter()
            .find(|button| button.text() == label)
    }

    /// The contents of the first `<tag attr="value">`, as a region.
    ///
    /// This is how an assertion is scoped to the part of the page it is about:
    /// a pagination link also says `aria-current="page"`, and both are correct
    /// HTML, but only the one in `<nav aria-label="Sections">` answers *"which
    /// section am I in"*.
    ///
    /// # Panics
    ///
    /// When no such element is here, naming the elements of that tag that are.
    #[must_use]
    pub fn within(&self, tag: &str, attr: &str, value: &str) -> Html {
        match self.try_within(tag, attr, value) {
            Some(region) => region,
            None => panic!(
                "no <{tag} {attr}=\"{value}\"> here.\n{}",
                listing(
                    &format!("<{tag}>"),
                    self.tags(tag)
                        .iter()
                        .map(|e| match e.attr(attr) {
                            Some(v) => format!("{attr}=\"{v}\""),
                            None => format!("no {attr}"),
                        })
                        .collect(),
                )
            ),
        }
    }

    /// [`Html::within`], for a caller that would rather look than assert.
    #[must_use]
    pub fn try_within(&self, tag: &str, attr: &str, value: &str) -> Option<Html> {
        self.tags(tag)
            .into_iter()
            .find(|e| e.attr(attr) == Some(value))
            .map(|e| e.html())
    }

    // -----------------------------------------------------------------
    // Assertions
    // -----------------------------------------------------------------

    /// There is a link whose `href` is exactly this.
    ///
    /// # Panics
    ///
    /// When there is not, listing every link that is here.
    pub fn assert_link_to(&self, href: &str) -> &Self {
        assert!(
            self.has_link_to(href),
            "no link to `{href}`.\n{}",
            listing(
                "link",
                self.links()
                    .iter()
                    .map(|a| match a.href() {
                        Some(h) => format!("{h}  ({})", a.text()),
                        None => format!("<a> with no href  ({})", a.text()),
                    })
                    .collect(),
            )
        );
        self
    }

    /// There is no link to this.
    ///
    /// # Panics
    ///
    /// When there is one, quoting it.
    pub fn assert_no_link_to(&self, href: &str) -> &Self {
        if let Some(found) = self.link_to(href) {
            panic!("a link to `{href}` is here, and should not be: {found:?}");
        }
        self
    }

    /// There is a form that posts to this path.
    ///
    /// # Panics
    ///
    /// When there is not, listing the forms that are here with their methods.
    pub fn assert_form_posts_to(&self, action: &str) -> &Self {
        assert!(
            self.form_posting_to(action).is_some(),
            "no form posting to `{action}`.\n{}",
            listing(
                "form",
                self.forms()
                    .iter()
                    .map(|f| format!(
                        "{} {}",
                        f.attr("method").unwrap_or("GET").to_ascii_uppercase(),
                        f.attr("action").unwrap_or("(no action)")
                    ))
                    .collect(),
            )
        );
        self
    }

    /// There is an input, select or textarea called this.
    ///
    /// # Panics
    ///
    /// When there is not, listing the named fields that are here.
    pub fn assert_field(&self, name: &str) -> &Self {
        assert!(
            self.field(name).is_some(),
            "no field named `{name}`.\n{}",
            listing(
                "field",
                self.fields()
                    .iter()
                    .map(|f| format!("{} ({})", f.attr("name").unwrap_or_default(), f.name()))
                    .collect(),
            )
        );
        self
    }

    /// There is a button whose text is this.
    ///
    /// # Panics
    ///
    /// When there is not, listing the buttons that are here.
    pub fn assert_button(&self, label: &str) -> &Self {
        assert!(
            self.button(label).is_some(),
            "no button labelled `{label}`.\n{}",
            listing("button", self.buttons().iter().map(Element::text).collect())
        );
        self
    }

    /// There is no button with this text.
    ///
    /// *"A reader who may not act draws no button"* is this, and it is a
    /// stronger claim than the absence of a byte sequence: it holds however
    /// the button is attributed or styled.
    ///
    /// # Panics
    ///
    /// When there is one.
    pub fn assert_no_button(&self, label: &str) -> &Self {
        assert!(
            self.button(label).is_none(),
            "a button labelled `{label}` is here, and should not be"
        );
        self
    }

    /// The text a reader would see contains this.
    ///
    /// Not the markup: `assert_text("page 2")` holds whatever tags and
    /// indentation the template put around it, and does not hold when the
    /// bytes are only in an attribute.
    ///
    /// # Panics
    ///
    /// When it does not, quoting the text that is here.
    pub fn assert_text(&self, needle: &str) -> &Self {
        let text = self.text();
        assert!(
            text.contains(needle),
            "the text here does not contain `{needle}`.\nthe text is:\n  {text}"
        );
        self
    }

    /// The text a reader would see does not contain this.
    ///
    /// # Panics
    ///
    /// When it does.
    pub fn assert_no_text(&self, needle: &str) -> &Self {
        let text = self.text();
        assert!(
            !text.contains(needle),
            "the text here contains `{needle}`, and should not.\nthe text is:\n  {text}"
        );
        self
    }
}

/// `no link to X. the 3 links here are: ...`, or `there are no links here.`
fn listing(what: &str, found: Vec<String>) -> String {
    if found.is_empty() {
        return format!("there are no {what}s here.");
    }
    let mut out = format!("the {} {what}s here are:", found.len());
    for item in found {
        let _ = write!(out, "\n  {item}");
    }
    out
}

// ---------------------------------------------------------------------------
// The scanner
// ---------------------------------------------------------------------------

/// Elements whose end tag HTML does not have.
const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Elements whose contents are not markup, and are skipped whole.
const RAW: &[&str] = &["script", "style"];

fn is_void(name: &str) -> bool {
    VOID.iter().any(|v| name.eq_ignore_ascii_case(v))
}

fn is_raw(name: &str) -> bool {
    RAW.iter().any(|v| name.eq_ignore_ascii_case(v))
}

/// Every element whose name `want` accepts, in source order.
fn scan(source: &str, want: &dyn Fn(&str) -> bool) -> Vec<Element> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        let rest = &source[i..];
        if rest.starts_with("<!--") {
            match rest.find("-->") {
                Some(end) => i += end + 3,
                None => break,
            }
            continue;
        }
        // A doctype, a processing instruction, or a closing tag: nothing here
        // opens an element, so skip to the end of it.
        if rest.starts_with("<!") || rest.starts_with("</") || rest.starts_with("<?") {
            match rest.find('>') {
                Some(end) => i += end + 1,
                None => break,
            }
            continue;
        }
        let Some(name_end) = tag_name_end(bytes, i + 1) else {
            i += 1;
            continue;
        };
        let name = &source[i + 1..name_end];
        let Some((gt, self_closing)) = open_tag_end(bytes, name_end) else {
            break;
        };

        if is_raw(name) {
            // The contents are CSS or JavaScript, and markup quoted inside
            // them is not markup.
            i = match find_close(source, name, gt + 1) {
                Some((_, after)) => after,
                None => break,
            };
            continue;
        }

        if want(name) {
            let inner = if self_closing || is_void(name) {
                None
            } else {
                find_close(source, name, gt + 1).map(|(end, _)| source[gt + 1..end].to_string())
            };
            out.push(Element {
                name: name.to_ascii_lowercase(),
                attrs: parse_attrs(&source[name_end..gt]),
                inner,
            });
        }
        i = gt + 1;
    }

    out
}

/// The index just past the tag name starting at `at`, or `None` when what is
/// there is not a tag name.
fn tag_name_end(bytes: &[u8], at: usize) -> Option<usize> {
    if at >= bytes.len() || !bytes[at].is_ascii_alphabetic() {
        return None;
    }
    let mut i = at;
    while i < bytes.len()
        && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-' || bytes[i] == b':')
    {
        i += 1;
    }
    Some(i)
}

/// The index of the `>` that closes an open tag, and whether it closed itself.
///
/// Quote-aware, because `title="a > b"` is legal and a naive search for `>`
/// would stop inside it.
fn open_tag_end(bytes: &[u8], mut i: usize) -> Option<(usize, bool)> {
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        let c = bytes[i];
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                b'"' | b'\'' => quote = Some(c),
                b'>' => {
                    let slash =
                        bytes[..i].iter().rev().find(|b| !b.is_ascii_whitespace()) == Some(&b'/');
                    return Some((i, slash));
                }
                _ => {}
            },
        }
        i += 1;
    }
    None
}

/// The end tag that matches an open tag of `name` whose contents start at
/// `from`, as (where the contents end, where the end tag ends).
///
/// Counts the depth of that one tag name. An element the source never closed
/// has no match here, and the caller reports no inner text rather than
/// guessing one.
fn find_close(source: &str, name: &str, from: usize) -> Option<(usize, usize)> {
    let bytes = source.as_bytes();
    let mut depth = 1usize;
    let mut i = from;

    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        let rest = &source[i..];
        if rest.starts_with("<!--") {
            i += rest.find("-->")? + 3;
            continue;
        }
        if rest.starts_with("</") {
            let after = i + 2;
            if name_at(bytes, after, name) {
                let gt = rest.find('>')? + i;
                depth -= 1;
                if depth == 0 {
                    return Some((i, gt + 1));
                }
                i = gt + 1;
                continue;
            }
            i += 2;
            continue;
        }
        if name_at(bytes, i + 1, name) {
            let (gt, self_closing) = open_tag_end(bytes, i + 1 + name.len())?;
            if !self_closing && !is_void(name) {
                depth += 1;
            }
            i = gt + 1;
            continue;
        }
        i += 1;
    }
    None
}

/// Whether the tag name at `at` is `name` and ends there.
fn name_at(bytes: &[u8], at: usize, name: &str) -> bool {
    let end = at + name.len();
    if end > bytes.len() || !bytes[at..end].eq_ignore_ascii_case(name.as_bytes()) {
        return false;
    }
    match bytes.get(end) {
        None => true,
        Some(c) => c.is_ascii_whitespace() || *c == b'>' || *c == b'/',
    }
}

/// The attributes written between a tag name and its `>`.
fn parse_attrs(source: &str) -> Vec<(String, String)> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
            i += 1;
        }
        let start = i;
        while i < bytes.len()
            && !bytes[i].is_ascii_whitespace()
            && bytes[i] != b'='
            && bytes[i] != b'/'
        {
            i += 1;
        }
        if i == start {
            // Nothing readable here; a malformed tag must not loop forever.
            i += 1;
            continue;
        }
        let name = source[start..i].to_ascii_lowercase();
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if bytes.get(i) != Some(&b'=') {
            // A bare attribute, like `dp-boost`. Present, with no value.
            out.push((name, String::new()));
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let value_start;
        let value_end;
        match bytes.get(i) {
            Some(q @ (b'"' | b'\'')) => {
                let quote = *q;
                i += 1;
                value_start = i;
                while i < bytes.len() && bytes[i] != quote {
                    i += 1;
                }
                value_end = i;
                i = (i + 1).min(bytes.len());
            }
            _ => {
                value_start = i;
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                value_end = i;
            }
        }
        out.push((name, decode(&source[value_start..value_end])));
    }

    out
}

/// Markup into the text a reader would see.
fn text_of(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut raw = String::with_capacity(source.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] != b'<' {
            let start = i;
            while i < bytes.len() && bytes[i] != b'<' {
                i += 1;
            }
            raw.push_str(&source[start..i]);
            continue;
        }
        let rest = &source[i..];
        if rest.starts_with("<!--") {
            match rest.find("-->") {
                Some(end) => i += end + 3,
                None => break,
            }
            continue;
        }
        if let Some(name_end) = tag_name_end(bytes, i + 1) {
            let name = &source[i + 1..name_end];
            if is_raw(name) {
                if let Some((_, after)) = open_tag_end(bytes, name_end)
                    .and_then(|(gt, _)| find_close(source, name, gt + 1))
                {
                    i = after;
                    continue;
                }
            }
        }
        match rest.find('>') {
            Some(end) => i += end + 1,
            None => break,
        }
    }

    collapse(&decode(&raw))
}

/// Runs of whitespace to one space, and trimmed, which is what a browser
/// shows and what a template's indentation should not change.
fn collapse(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
    }
    out
}

/// The character references a server-rendered page actually emits.
fn decode(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let Some(semi) = tail[..tail.len().min(12)].find(';') else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => numeric(entity),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &tail[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `#39` and `#x27`.
fn numeric(entity: &str) -> Option<char> {
    let digits = entity.strip_prefix('#')?;
    let code = match digits.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => digits.parse().ok()?,
    };
    char::from_u32(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_is_found_by_its_href_and_not_by_its_bytes() {
        let page =
            Html::new(r#"<p data-trace="/admin/post">see</p><a href="/admin/user">Users</a>"#);
        assert!(page.has_link_to("/admin/user"));
        assert!(
            !page.has_link_to("/admin/post"),
            "a path in a data attribute is not a link, and `contains` cannot tell"
        );
    }

    #[test]
    fn an_attribute_comes_back_the_way_a_browser_would_read_it() {
        let page = Html::new(r#"<a href="/post?sort=views&amp;dir=asc">Views</a>"#);
        page.assert_link_to("/post?sort=views&dir=asc");
    }

    #[test]
    fn an_attribute_may_span_lines_and_may_hold_a_greater_than() {
        let page = Html::new("<a\n  href=\"/x\"\n  title=\"a > b\"\n  dp-boost>Go</a>");
        let link = page.link_to("/x").expect("the link");
        assert_eq!(link.attr("title"), Some("a > b"));
        assert_eq!(
            link.attr("dp-boost"),
            Some(""),
            "a bare attribute is present"
        );
        assert_eq!(link.text(), "Go");
    }

    #[test]
    fn a_region_is_scoped_by_an_attribute_and_closes_at_its_own_depth() {
        let page = Html::new(
            r#"<nav aria-label="Sections"><nav><a href="/in">in</a></nav></nav>
               <nav aria-label="Pagination"><a href="/out">out</a></nav>"#,
        );
        let rail = page.within("nav", "aria-label", "Sections");
        assert!(rail.has_link_to("/in"));
        assert!(
            !rail.has_link_to("/out"),
            "the nested nav did not end the rail"
        );
    }

    #[test]
    fn markup_inside_a_style_block_is_not_markup() {
        let page = Html::new(r#"<style>a[href="/admin"] { color: red }</style><a href="/x">x</a>"#);
        assert_eq!(page.links().len(), 1);
        assert_eq!(page.text(), "x");
    }

    #[test]
    fn text_is_what_a_reader_sees_rather_than_what_a_template_indented() {
        let page = Html::new("<div>\n  <p>page 2</p>\n  <!-- hidden -->\n</div>");
        assert_eq!(page.text(), "page 2");
    }

    #[test]
    fn a_form_reports_the_verb_it_actually_uses() {
        let page = Html::new(
            r#"<form action="/a"><button>Search</button></form>
               <form method="post" action="/b"><button>Delete</button></form>"#,
        );
        assert!(page.form_posting_to("/b").is_some());
        assert!(
            page.form_posting_to("/a").is_none(),
            "a form with no method is a GET, and a GET must not delete"
        );
        page.assert_button("Delete");
    }

    #[test]
    fn fields_are_the_named_inputs_whatever_tag_they_are() {
        let page = Html::new(
            r#"<input type="hidden"><input name="email" type="email">
               <select name="author"><option value="1">Ada</option></select>
               <textarea name="bio"></textarea>"#,
        );
        let fields = page.fields();
        let names: Vec<&str> = fields.iter().filter_map(|f| f.attr("name")).collect();
        assert_eq!(names, ["email", "author", "bio"]);
        page.assert_field("bio");
    }

    #[test]
    fn an_unclosed_element_reports_no_inner_text_rather_than_a_guess() {
        let page = Html::new(r#"<a href="/x">open"#);
        let link = page.link_to("/x").expect("the link");
        assert_eq!(link.inner_html(), None);
    }

    #[test]
    #[should_panic(expected = "the 2 links here are")]
    fn a_missing_link_says_which_links_are_there() {
        Html::new(r#"<a href="/a">A</a><a href="/b">B</a>"#).assert_link_to("/c");
    }
}
