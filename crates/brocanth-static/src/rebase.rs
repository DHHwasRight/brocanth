//! A base path, put in front of every root-relative URL a page names.
//!
//! A GitHub Pages project site is served under `/<repo>/`, but a brocanth
//! page names its links from the root: `/charts/bar`, `/js/app.js`. The
//! export rewrites them, so the application does not have to know where
//! it will be served.
//!
//! What counts as a URL is a fixed list: the attributes in [`ATTRIBUTES`],
//! the `url(...)` and `@import` of a style sheet, and the same two inside a
//! `<style>` element and a `style` attribute. The contents of a `<script>`
//! element are never touched: a JSON payload is data, and a URL in it is
//! the application's to prefix (it knows which strings are URLs; a scanner
//! does not).
//!
//! The scanner is not an HTML parser. It reads tags, attributes, comments
//! and the two raw-text elements that matter here (`script`, `style`), and
//! copies every other byte unchanged. That is enough for markup a
//! template engine wrote, and it keeps the crate free of a parser
//! dependency.

/// The attributes whose value is a URL, or (`dp-layout`) a path prefix that
/// dp-swap compares with `location.pathname`, which carries the base.
pub const ATTRIBUTES: &[&str] = &[
    "href",
    "src",
    "action",
    "formaction",
    "poster",
    "srcset",
    "dp-layout",
];

/// `"/d3set/"`, `"d3set"` and `"/d3set"` are the same base: `"/d3set"`.
/// `""` and `"/"` are no base.
///
/// # Errors
///
/// A base with a query, a fragment, an empty segment or a scheme is not a
/// path prefix, and the reason is returned.
pub fn normalize(base: &str) -> Result<String, String> {
    let trimmed = base.trim().trim_matches('/');
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if trimmed.contains(['?', '#', ':', '\\']) || trimmed.split('/').any(str::is_empty) {
        return Err(format!(
            "`{base}` is not a path prefix\n\
             help: pass the path the site is served under, such as `/d3set`"
        ));
    }
    Ok(format!("/{trimmed}"))
}

/// One URL: `/x` becomes `{base}/x`. A URL that is not root-relative
/// (`https://…`, `//cdn…`, `x.css`, `#top`, `?q`) is returned unchanged.
pub fn url(value: &str, base: &str) -> String {
    if base.is_empty() || !value.starts_with('/') || value.starts_with("//") {
        return value.to_string();
    }
    format!("{base}{value}")
}

/// A `srcset`: each candidate's URL, descriptors kept.
fn srcset(value: &str, base: &str) -> String {
    value
        .split(',')
        .map(|candidate| {
            let candidate = candidate.trim();
            match candidate.split_once(char::is_whitespace) {
                Some((at, descriptor)) => format!("{} {}", url(at, base), descriptor.trim()),
                None => url(candidate, base),
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A style sheet: `url(/x)`, `url("/x")`, `url('/x')`, `@import "/x"`.
pub fn css(text: &str, base: &str) -> String {
    if base.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 64);
    let mut rest = text;
    loop {
        let at_url = find_ascii_ci(rest, "url(");
        let at_import = find_ascii_ci(rest, "@import");
        let (at, skip) = match (at_url, at_import) {
            (Some(u), Some(i)) if i < u => (i, "@import".len()),
            (Some(u), _) => (u, "url(".len()),
            (None, Some(i)) => (i, "@import".len()),
            (None, None) => break,
        };
        let head = at + skip;
        out.push_str(&rest[..head]);
        rest = &rest[head..];
        // Whitespace, then an optional quote, then the URL itself.
        let blank = rest.len() - rest.trim_start().len();
        out.push_str(&rest[..blank]);
        rest = &rest[blank..];
        if let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') {
            out.push(quote);
            rest = &rest[1..];
        }
        if rest.starts_with('/') && !rest.starts_with("//") {
            out.push_str(base);
        }
    }
    out.push_str(rest);
    out
}

/// A document: every URL attribute, every `<style>` and `style`.
pub fn html(text: &str, base: &str) -> String {
    if base.is_empty() {
        return text.to_string();
    }
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len() + 256);
    let mut i = 0;
    while i < bytes.len() {
        let Some(lt) = text[i..].find('<').map(|n| i + n) else {
            out.push_str(&text[i..]);
            break;
        };
        out.push_str(&text[i..lt]);
        let after = &text[lt..];
        if after.starts_with("<!--") {
            let end = after.find("-->").map_or(after.len(), |n| n + 3);
            out.push_str(&after[..end]);
            i = lt + end;
        } else if bytes.get(lt + 1).is_some_and(u8::is_ascii_alphabetic) {
            let (tag, name, used) = start_tag(after, base);
            out.push_str(&tag);
            i = lt + used;
            // The raw-text elements: their content is not markup.
            if name == "script" || name == "style" {
                let close = format!("</{name}");
                let end = find_ascii_ci(&text[i..], &close).map_or(text.len(), |n| i + n);
                if name == "style" {
                    out.push_str(&css(&text[i..end], base));
                } else {
                    out.push_str(&text[i..end]);
                }
                i = end;
            }
        } else {
            // An end tag, a doctype, or a `<` in text: copied through `>`
            // when it is a tag, and as the one byte otherwise.
            if after.starts_with("</") || after.starts_with("<!") || after.starts_with("<?") {
                let end = after.find('>').map_or(after.len(), |n| n + 1);
                out.push_str(&after[..end]);
                i = lt + end;
            } else {
                out.push('<');
                i = lt + 1;
            }
        }
    }
    out
}

/// One start tag at the head of `text`: the rewritten tag, its lowercase
/// name, and how many bytes of `text` it used.
fn start_tag(text: &str, base: &str) -> (String, String, usize) {
    let bytes = text.as_bytes();
    let mut i = 1;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-') {
        i += 1;
    }
    let name = text[1..i].to_ascii_lowercase();
    let mut out = String::from(&text[..i]);
    loop {
        // Whitespace and stray slashes between attributes.
        let start = i;
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
            i += 1;
        }
        out.push_str(&text[start..i]);
        if i >= bytes.len() {
            return (out, name, i);
        }
        if bytes[i] == b'>' {
            out.push('>');
            return (out, name, i + 1);
        }
        // The attribute name.
        let at = i;
        while i < bytes.len()
            && !matches!(bytes[i], b'=' | b'>' | b'/')
            && !bytes[i].is_ascii_whitespace()
        {
            i += 1;
        }
        let attribute = text[at..i].to_ascii_lowercase();
        out.push_str(&text[at..i]);
        // `=`, with optional whitespace around it.
        let mut look = i;
        while look < bytes.len() && bytes[look].is_ascii_whitespace() {
            look += 1;
        }
        if bytes.get(look) != Some(&b'=') {
            continue;
        }
        look += 1;
        while look < bytes.len() && bytes[look].is_ascii_whitespace() {
            look += 1;
        }
        out.push_str(&text[i..look]);
        i = look;
        // The value: quoted, or up to whitespace or `>`.
        let (value, quote, used) = match bytes.get(i) {
            Some(&q @ (b'"' | b'\'')) => {
                let end = text[i + 1..]
                    .find(q as char)
                    .map_or(text.len(), |n| i + 1 + n);
                (
                    &text[i + 1..end],
                    Some(q as char),
                    end.min(text.len() - 1) + 1 - i,
                )
            }
            _ => {
                let end = text[i..]
                    .find(|c: char| c.is_ascii_whitespace() || c == '>')
                    .map_or(text.len(), |n| i + n);
                (&text[i..end], None, end - i)
            }
        };
        let rewritten = if attribute == "srcset" {
            srcset(value, base)
        } else if attribute == "style" {
            css(value, base)
        } else if ATTRIBUTES.contains(&attribute.as_str()) {
            url(value, base)
        } else {
            value.to_string()
        };
        match quote {
            Some(q) => {
                out.push(q);
                out.push_str(&rewritten);
                if text[i + 1..].contains(q) {
                    out.push(q);
                }
            }
            None => out.push_str(&rewritten),
        }
        i += used;
    }
}

/// `needle` in `hay`, ASCII case-insensitively. `needle` is ASCII.
fn find_ascii_ci(hay: &str, needle: &str) -> Option<usize> {
    let (hay, needle) = (hay.as_bytes(), needle.as_bytes());
    if needle.len() > hay.len() {
        return None;
    }
    (0..=hay.len() - needle.len())
        .find(|&at| hay[at..at + needle.len()].eq_ignore_ascii_case(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: &str = "/d3set";

    #[test]
    fn a_base_has_one_spelling() {
        for spelling in ["/d3set", "d3set", "/d3set/", " /d3set/ "] {
            assert_eq!(normalize(spelling).unwrap(), "/d3set");
        }
        assert_eq!(normalize("").unwrap(), "");
        assert_eq!(normalize("/").unwrap(), "");
        assert_eq!(normalize("/a/b/").unwrap(), "/a/b");
    }

    #[test]
    fn a_base_that_is_not_a_path_is_refused_with_help() {
        for bad in ["/a?x", "/a#b", "https://x", "/a//b"] {
            let why = normalize(bad).unwrap_err();
            assert!(why.contains("help:"), "{why}");
        }
    }

    #[test]
    fn only_root_relative_urls_move() {
        assert_eq!(url("/charts/bar", B), "/d3set/charts/bar");
        assert_eq!(url("/", B), "/d3set/");
        for kept in [
            "https://x.dev/a",
            "//cdn.dev/a.js",
            "a.css",
            "#top",
            "?q=1",
            "mailto:a@b",
            "",
        ] {
            assert_eq!(url(kept, B), kept);
        }
        assert_eq!(url("/x", ""), "/x");
    }

    #[test]
    fn url_attributes_move_and_others_stay() {
        let page = r#"<a href="/charts/bar" title="/not-a-url" data-x="/y">x</a><img src='/i.png' alt=/z><form action=/go method=post></form>"#;
        assert_eq!(
            html(page, B),
            r#"<a href="/d3set/charts/bar" title="/not-a-url" data-x="/y">x</a><img src='/d3set/i.png' alt=/z><form action=/d3set/go method=post></form>"#
        );
    }

    #[test]
    fn layout_markers_carry_the_base() {
        let page = r#"<main id="app" dp-layout="/"><div dp-layout="/charts">x</div></main>"#;
        assert_eq!(
            html(page, B),
            r#"<main id="app" dp-layout="/d3set/"><div dp-layout="/d3set/charts">x</div></main>"#
        );
    }

    #[test]
    fn attribute_names_are_case_insensitive_and_spacing_is_kept() {
        assert_eq!(html(r#"<A HREF = "/a">"#, B), r#"<A HREF = "/d3set/a">"#);
        assert_eq!(
            html("<link\n  rel=stylesheet\n  href=\"/s.css\">", B),
            "<link\n  rel=stylesheet\n  href=\"/d3set/s.css\">"
        );
    }

    #[test]
    fn boolean_attributes_and_self_closing_tags_survive() {
        assert_eq!(
            html(
                r#"<input disabled src="/a.png"/><br/><script defer src="/m.js"></script>"#,
                B
            ),
            r#"<input disabled src="/d3set/a.png"/><br/><script defer src="/d3set/m.js"></script>"#
        );
    }

    #[test]
    fn script_contents_are_data_and_stay() {
        let page = r#"<script type="application/json">{"url":"/data/sales.json","x":"<a href=\"/q\">"}</script><a href="/b">"#;
        assert_eq!(
            html(page, B),
            r#"<script type="application/json">{"url":"/data/sales.json","x":"<a href=\"/q\">"}</script><a href="/d3set/b">"#
        );
    }

    #[test]
    fn comments_and_doctype_stay() {
        let page = r#"<!DOCTYPE html><!-- <a href="/x"> --><a href="/y">"#;
        assert_eq!(
            html(page, B),
            r#"<!DOCTYPE html><!-- <a href="/x"> --><a href="/d3set/y">"#
        );
    }

    #[test]
    fn a_less_than_in_text_is_text() {
        assert_eq!(html("<p>1 < 2 and a<b</p>", B), "<p>1 < 2 and a<b</p>");
    }

    #[test]
    fn srcset_candidates_move_one_by_one() {
        assert_eq!(
            html(
                r#"<img srcset="/a.png 1x, /b.png 2x,https://c/x.png 3x">"#,
                B
            ),
            r#"<img srcset="/d3set/a.png 1x, /d3set/b.png 2x, https://c/x.png 3x">"#
        );
    }

    #[test]
    fn style_sheets_move_their_urls() {
        let sheet = "@font-face{src:url(/f.woff2)} a{background:URL( \"/i.png\")} b{background:url('//cdn/x.png')} @import '/base.css'; c{background:url(x.png)}";
        assert_eq!(
            css(sheet, B),
            "@font-face{src:url(/d3set/f.woff2)} a{background:URL( \"/d3set/i.png\")} b{background:url('//cdn/x.png')} @import '/d3set/base.css'; c{background:url(x.png)}"
        );
    }

    #[test]
    fn style_elements_and_attributes_move_their_urls() {
        assert_eq!(
            html(
                r#"<style>a{b:url(/i.png)}</style><div style="background:url(/j.png)">"#,
                B
            ),
            r#"<style>a{b:url(/d3set/i.png)}</style><div style="background:url(/d3set/j.png)">"#
        );
    }

    #[test]
    fn no_base_is_the_identity() {
        let page = r#"<a href="/x"><style>a{b:url(/i)}</style>"#;
        assert_eq!(html(page, ""), page);
        assert_eq!(css("a{b:url(/i)}", ""), "a{b:url(/i)}");
    }

    #[test]
    fn a_truncated_tag_does_not_panic() {
        for cut in [
            r#"<a href="/x"#,
            "<a href=",
            "<a ",
            "<",
            "<!--",
            "<script>x",
            r#"<a href='/x"#,
        ] {
            let _ = html(cut, B);
        }
    }
}
