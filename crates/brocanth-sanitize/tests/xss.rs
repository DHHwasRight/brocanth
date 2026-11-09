//! Known XSS vectors, through every policy.
//!
//! The check reads the output tag by tag rather than searching it for
//! words. A cleaned comment may well contain the *text* `onerror=alert(1)`,
//! escaped, and that is harmless; what must not survive is a tag or an
//! attribute. In the output, a literal `<` always starts a real tag, because
//! the serializer escapes every `<` in text. So every `<…>` is a tag the
//! browser will build, and each one is checked against the list.

use brocanth_sanitize::{comment, rich_text, text, Policy, SafeHtml};

/// The vectors. Most come from the OWASP XSS filter evasion cheat sheet,
/// the rest from published mutation-XSS reports against sanitizers.
const VECTORS: &[&str] = &[
    "<script>alert(1)</script>",
    "<SCRIPT SRC=//xss.example/x.js></SCRIPT>",
    "<scr<script>ipt>alert(1)</scr</script>ipt>",
    "<script",
    "<img src=x onerror=alert(1)>",
    "<img src=\"javascript:alert(1)\">",
    "<IMG SRC=JaVaScRiPt:alert(1)>",
    "<img src=\"jav&#x09;ascript:alert(1)\">",
    "<img src=x:alert(alt) onerror=eval(src) alt=0>",
    "<svg onload=alert(1)>",
    "<svg><script>alert(1)</script></svg>",
    "<svg><a xlink:href=\"javascript:alert(1)\"><text>x</text></a></svg>",
    "<math><mtext><table><mglyph><style><img src=x onerror=alert(1)>",
    "<noscript><p title=\"</noscript><img src=x onerror=alert(1)>\">",
    "<a href=\"javascript:alert(1)\">x</a>",
    "<a href=\" javascript:alert(1)\">x</a>",
    "<a href=\"&#106;avascript:alert(1)\">x</a>",
    "<a href=\"&#x6A;&#x61;&#x76;&#x61;script:alert(1)\">x</a>",
    "<a href=\"java\nscript:alert(1)\">x</a>",
    "<a href=\"vbscript:msgbox(1)\">x</a>",
    "<a href=\"data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==\">x</a>",
    "<a href=\"x\" onmouseover=\"alert(1)\">x</a>",
    "<a href=x\"onmouseover=\"alert(1)\">x</a>",
    "\"><script>alert(1)</script>",
    "'><img src=x onerror=alert(1)>",
    "<!--><script>alert(1)</script>-->",
    "<!--[if gte IE 4]><script>alert(1)</script><![endif]-->",
    "<iframe src=\"javascript:alert(1)\"></iframe>",
    "<iframe srcdoc=\"<script>alert(1)</script>\"></iframe>",
    "<object data=\"javascript:alert(1)\"></object>",
    "<embed src=\"javascript:alert(1)\">",
    "<form action=\"javascript:alert(1)\"><button>x</button></form>",
    "<input onfocus=alert(1) autofocus>",
    "<details open ontoggle=alert(1)>",
    "<base href=\"javascript:alert(1)//\">",
    "<meta http-equiv=\"refresh\" content=\"0;url=javascript:alert(1)\">",
    "<link rel=stylesheet href=\"javascript:alert(1)\">",
    "<style>@import 'javascript:alert(1)';</style>",
    "<div style=\"background:url(javascript:alert(1))\">x</div>",
    "<p style=\"width: expression(alert(1))\">x</p>",
    "<body onload=alert(1)>",
    "<video><source onerror=\"alert(1)\"></video>",
    "<marquee onstart=alert(1)>x</marquee>",
    "<table background=\"javascript:alert(1)\"><tr><td>x</td></tr></table>",
    "<td background=\"javascript:alert(1)\">x</td>",
    "<p id=\"location\" class=\"admin-button\" name=\"x\">x</p>",
    "<template><script>alert(1)</script></template>",
    "<a href=\"https://ok.example\" target=\"_blank\" rel=\"opener\">x</a>",
    "<q cite=\"javascript:alert(1)\">x</q>",
    "<blockquote cite=\"javascript:alert(1)\">x</blockquote>",
    "<del cite=\"javascript:alert(1)\" datetime=\"x\">x</del>",
    "<p title=\"x\" lang=\"en\">x</p>",
    "<img src=\"data:image/svg+xml;base64,PHN2Zz4=\">",
    "<xmp><p title=\"</xmp><img src=x onerror=alert(1)>\">",
    "<plaintext><img src=x onerror=alert(1)>",
    "<a href=\"//evil.example/%0a\">x</a>",
];

/// The tags and attributes a policy may let through, for the check.
struct Allowed {
    tags: &'static [&'static str],
    attributes: &'static [(&'static str, &'static [&'static str])],
}

const COMMENT: Allowed = Allowed {
    tags: &[
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
    ],
    attributes: &[("a", &["href", "title", "rel"])],
};

const RICH_TEXT: Allowed = Allowed {
    tags: &[
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
    ],
    attributes: &[
        ("a", &["href", "title", "rel"]),
        ("img", &["src", "alt", "title", "width", "height"]),
        ("td", &["colspan", "rowspan"]),
        ("th", &["colspan", "rowspan", "scope"]),
        ("abbr", &["title"]),
    ],
};

const TEXT: Allowed = Allowed {
    tags: &[],
    attributes: &[],
};

/// One tag in the output: its name and its attributes, values decoded of
/// the entities the serializer writes.
struct Tag {
    name: String,
    attributes: Vec<(String, String)>,
}

/// Every tag in serialized HTML. The input is the sanitizer's own output,
/// so the grammar is the serializer's: `<name attr="value">` or `</name>`,
/// double quotes, `&quot;` and `&amp;` inside.
fn tags(html: &str) -> Vec<Tag> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        let after = &rest[start + 1..];
        let end = after
            .find('>')
            .unwrap_or_else(|| panic!("an open tag in {html:?}"));
        let inside = after[..end].trim_start_matches('/').trim_end_matches('/');
        let (name, mut attrs) = inside.split_once(' ').unwrap_or((inside, ""));
        let mut attributes = Vec::new();
        while !attrs.trim().is_empty() {
            attrs = attrs.trim_start();
            let (key, tail) = attrs.split_once('=').unwrap_or((attrs, ""));
            if tail.is_empty() {
                attributes.push((key.trim().to_string(), String::new()));
                break;
            }
            let tail = tail.strip_prefix('"').expect("a quoted value");
            let close = tail.find('"').expect("a closing quote");
            let value = tail[..close].replace("&quot;", "\"").replace("&amp;", "&");
            attributes.push((key.trim().to_ascii_lowercase(), value));
            attrs = &tail[close + 1..];
        }
        out.push(Tag {
            name: name.to_ascii_lowercase(),
            attributes,
        });
        rest = &after[end + 1..];
    }
    out
}

/// What a browser would see as the scheme of a URL: control characters and
/// spaces removed, lower case, up to the first `:`.
fn scheme(url: &str) -> Option<String> {
    let cleaned: String = url
        .chars()
        .filter(|c| !c.is_ascii_control() && !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    let (head, _) = cleaned.split_once(':')?;
    (!head.contains('/') && !head.contains('?') && !head.contains('#')).then(|| head.to_string())
}

fn assert_safe(policy: &str, allowed: &Allowed, input: &str, output: &SafeHtml) {
    for tag in tags(output.as_str()) {
        assert!(
            allowed.tags.contains(&tag.name.as_str()),
            "{policy}: <{}> survived\n  input:  {input}\n  output: {output}",
            tag.name
        );
        let names = allowed
            .attributes
            .iter()
            .find(|(t, _)| *t == tag.name)
            .map_or(&[][..], |(_, a)| *a);
        for (key, value) in &tag.attributes {
            assert!(
                names.contains(&key.as_str()),
                "{policy}: {key}= on <{}> survived\n  input:  {input}\n  output: {output}",
                tag.name
            );
            if matches!(key.as_str(), "href" | "src" | "cite") {
                if let Some(scheme) = scheme(value) {
                    assert!(
                        ["http", "https", "mailto"].contains(&scheme.as_str()),
                        "{policy}: a {scheme}: URL survived\n  input:  {input}\n  output: {output}"
                    );
                }
            }
        }
    }
}

#[test]
fn no_vector_survives_any_policy() {
    for input in VECTORS {
        assert_safe("comment", &COMMENT, input, &comment(input));
        assert_safe("rich_text", &RICH_TEXT, input, &rich_text(input));
        assert_safe("text", &TEXT, input, &text(input));
    }
}

#[test]
fn the_checker_catches_what_it_is_for() {
    // A check that passes everything proves nothing. Feed it the raw
    // vectors, as if a page had skipped the policy.
    let unsafe_outputs = [
        "<script>alert(1)</script>",
        "<img src=\"x\" onerror=\"alert(1)\">",
        "<a href=\"javascript:alert(1)\">x</a>",
        "<a href=\"java\nscript:alert(1)\">x</a>",
        "<p style=\"color:red\">x</p>",
    ];
    for raw in unsafe_outputs {
        let caught = std::panic::catch_unwind(|| {
            assert_safe(
                "none",
                &RICH_TEXT,
                raw,
                &SafeHtml::from_static(Box::leak(raw.to_string().into_boxed_str())),
            );
        });
        assert!(caught.is_err(), "the checker let {raw:?} through");
    }
}

#[test]
fn script_and_style_go_with_their_content() {
    assert_eq!(comment("a<script>alert(1)</script>b").as_str(), "ab");
    assert_eq!(rich_text("a<style>p{}</style>b").as_str(), "ab");
    assert_eq!(text("a<script>alert(1)</script>b").as_str(), "ab");
}

#[test]
fn ordinary_markup_stays() {
    let input = "<p>Hello <strong>you</strong>, see <a href=\"https://example.com/a?b=1&amp;c=2\" title=\"t\">this</a>.</p>\
                 <ul><li>one</li></ul><pre><code>let x = 1 &lt; 2;</code></pre>";
    assert_eq!(
        comment(input).as_str(),
        "<p>Hello <strong>you</strong>, see <a href=\"https://example.com/a?b=1&amp;c=2\" \
         title=\"t\" rel=\"nofollow ugc noopener noreferrer\">this</a>.</p>\
         <ul><li>one</li></ul><pre><code>let x = 1 &lt; 2;</code></pre>"
    );
}

#[test]
fn a_relative_link_and_a_mailto_stay() {
    assert_eq!(
        comment("<a href=\"/posts/2\">next</a> <a href=\"mailto:a@example.com\">mail</a>").as_str(),
        "<a href=\"/posts/2\" rel=\"nofollow ugc noopener noreferrer\">next</a> \
         <a href=\"mailto:a@example.com\" rel=\"nofollow ugc noopener noreferrer\">mail</a>"
    );
}

#[test]
fn rich_text_keeps_a_table_and_an_image() {
    let input = "<h2>T</h2><table><thead><tr><th scope=\"col\">a</th></tr></thead>\
                 <tbody><tr><td colspan=\"2\">b</td></tr></tbody></table>\
                 <figure><img src=\"/a.png\" alt=\"a\" width=\"10\"><figcaption>c</figcaption></figure>";
    assert_eq!(rich_text(input).as_str(), input);
    // A comment keeps the text and drops the structure it does not allow.
    assert_eq!(comment("<h2>T</h2><img src=\"/a.png\">").as_str(), "T");
}

#[test]
fn a_tag_that_is_not_allowed_keeps_its_text() {
    assert_eq!(comment("<span>kept</span>").as_str(), "kept");
    assert_eq!(
        text("<b>Bold</b> &amp; <i>brave</i>").as_str(),
        "Bold &amp; brave"
    );
}

#[test]
fn a_custom_policy_starts_from_a_given_one() {
    let policy = Policy::comment()
        .deny_tags(["a"])
        .allow_tags(["h3"])
        .allow_attributes("p", ["lang"]);
    assert_eq!(
        policy
            .clean("<h3>x</h3><p lang=\"fr\">y</p><a href=\"https://e.example\">z</a>")
            .as_str(),
        "<h3>x</h3><p lang=\"fr\">y</p>z"
    );
}

#[test]
fn a_policy_without_rel_keeps_none() {
    let policy = Policy::rich_text().link_rel(None);
    assert_eq!(
        policy.clean("<a href=\"/x\" rel=\"opener\">x</a>").as_str(),
        "<a href=\"/x\">x</a>"
    );
}

#[test]
fn escape_turns_text_into_html_that_shows_it() {
    assert_eq!(
        SafeHtml::escape("<b> & \"q\" 'a'").as_str(),
        "&lt;b&gt; &amp; &quot;q&quot; &#39;a&#39;"
    );
    assert!(SafeHtml::default().is_empty());
    assert_eq!(
        SafeHtml::from_static("<br>")
            .concat(&SafeHtml::escape("<"))
            .into_string(),
        "<br>&lt;"
    );
}

/// The template side: a `SafeHtml` is written as markup, a `String` in the
/// same place is escaped.
#[test]
fn rsx_writes_safe_html_as_markup() {
    use rsx::Template as _;

    let cleaned = comment("<p>hi</p><script>x</script>");
    assert_eq!((&cleaned).render_to_string().expect("renders"), "<p>hi</p>");
    assert_eq!(cleaned.render_to_string().expect("renders"), "<p>hi</p>");
}
