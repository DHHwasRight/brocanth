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

/// More vectors, by family. Each family is a way a sanitizer has failed
/// before: an attribute it did not know, a URL it read differently from
/// the browser, a foreign-content tree (`svg`, `math`) that changes how the
/// next tags parse, an entity that decodes into markup, a tag that is
/// broken on purpose.
const MORE_VECTORS: &[&str] = &[
    // Attributes: every event handler, in every spelling the parser takes.
    "<p ONCLICK=alert(1)>x</p>",
    "<p onclick\n=alert(1)>x</p>",
    "<p/onclick=alert(1)>x</p>",
    "<img/src=x/onerror=alert(1)>",
    "<img src=\"x\" alt=\"\"onerror=\"alert(1)\">",
    "<abbr title=\"x\" onmouseover=\"alert(1)\">x</abbr>",
    "<td colspan=\"2\" onclick=\"alert(1)\">x</td>",
    "<a href=\"/x\"style=\"position:fixed;inset:0\">x</a>",
    "<a href=\"/x\" ping=\"https://evil.example/\">x</a>",
    "<a href=\"/x\" target=\"_blank\" download>x</a>",
    "<img src=\"/a.png\" srcset=\"javascript:alert(1) 1x\">",
    "<img src=\"/a.png\" usemap=\"#m\"><map name=\"m\"><area href=\"javascript:alert(1)\"></map>",
    "<button formaction=\"javascript:alert(1)\">x</button>",
    "<video poster=\"javascript:alert(1)\"></video>",
    "<img src=\"/a.png\" name=\"getElementById\">",
    "<a href=\"/x\" name=\"cookie\">x</a>",
    "<p xmlns:xlink=\"x\" xlink:href=\"javascript:alert(1)\">x</p>",
    "<img src=\"/a.png\" loading=\"lazy\" referrerpolicy=\"unsafe-url\">",
    "<a href=\"/x\" accesskey=\"x\" tabindex=\"1\" contenteditable>x</a>",
    // URLs: what the browser strips before it reads the scheme.
    "<a href=\"JAVASCRIPT:alert(1)\">x</a>",
    "<a href='javascript:alert(1)'>x</a>",
    "<a href=javascript:alert(1)>x</a>",
    "<a\nhref=\"javascript:alert(1)\">x</a>",
    "<a href=\"jav&#x0A;ascript:alert(1)\">x</a>",
    "<a href=\"jav&Tab;ascript:alert(1)\">x</a>",
    "<a href=\"javascript&colon;alert(1)\">x</a>",
    "<a href=\"&#0000106&#0000097&#0000118&#0000097&#0000115&#0000099&#0000114&#0000105&#0000112&#0000116&#0000058alert(1)\">x</a>",
    "<a href=\"&#14;javascript:alert(1)\">x</a>",
    "<a href=\"\u{1}javascript:alert(1)\">x</a>",
    "<a href=\"ja\u{0}vascript:alert(1)\">x</a>",
    "<a href=\"javascript://%0aalert(1)\">x</a>",
    "<a href=\"data:text/html,<script>alert(1)</script>\">x</a>",
    "<a href=\"DATA:text/html,x\">x</a>",
    "<img src=\"data:image/png;base64,iVBORw0KGgo=\">",
    "<a href=\"file:///etc/passwd\">x</a>",
    "<a href=\"blob:https://x.example/y\">x</a>",
    "<a href=\"livescript:alert(1)\">x</a>",
    "<a href=\"ftp://x.example/\">x</a>",
    "<img src=\"vbscript:msgbox(1)\">",
    // svg and math: foreign content, and the mutation-XSS family.
    "<svg><animate onbegin=alert(1) attributeName=x dur=1s>",
    "<svg><set attributeName=href to=javascript:alert(1)>",
    "<svg><animate attributeName=href values=javascript:alert(1) /><a><text>x</text></a></svg>",
    "<svg><use href=\"data:image/svg+xml,<svg id='x' xmlns='http://www.w3.org/2000/svg'><image href='1' onerror='alert(1)' /></svg>#x\"></use></svg>",
    "<svg><foreignObject><img src=x onerror=alert(1)></foreignObject></svg>",
    "<svg><style><img src=x onerror=alert(1)></style></svg>",
    "<svg></p><style><a id=\"</style><img src=1 onerror=alert(1)>\">",
    "<svg><desc><img src=x onerror=alert(1)></desc></svg>",
    "<svg><image href=\"javascript:alert(1)\"></image></svg>",
    "<math><maction actiontype=statusline xlink:href=javascript:alert(1)>x</maction></math>",
    "<math href=\"javascript:alert(1)\">x</math>",
    "<math><mi><mglyph><svg><mtext><textarea><path id=\"</textarea><img onerror=alert(1) src=1>\">",
    "<form><math><mtext></form><form><mglyph><style></math><img src onerror=alert(1)>",
    "<math><mtext><h1><a href=\"javascript:alert(1)\">x</a></h1></mtext></math>",
    "<math><annotation-xml encoding=\"text/html\"><img src=x onerror=alert(1)></annotation-xml></math>",
    // Raw-text and RCDATA elements: their content is text, until it is not.
    "<noscript><img src=x onerror=alert(1)></noscript>",
    "<noembed><img title=\"</noembed><img src onerror=alert(1)>\"></noembed>",
    "<noframes><img title=\"</noframes><img src onerror=alert(1)>\">",
    "<iframe><img title=\"</iframe><img src onerror=alert(1)>\"></iframe>",
    "<textarea><img title=\"</textarea><img src onerror=alert(1)>\">",
    "<title><img title=\"</title><img src onerror=alert(1)>\">",
    "<style><img title=\"</style><img src onerror=alert(1)>\">",
    "<img src=\"/a.png\" alt=\"</p><script>alert(1)</script>\">",
    "<img src=\"/a.png\" title=\"<img src=x onerror=alert(1)>\">",
    // Entities: text that decodes into markup must stay text.
    "&lt;script&gt;alert(1)&lt;/script&gt;",
    "&#60;script&#62;alert(1)&#60;/script&#62;",
    "&#x3C;img src=x onerror=alert(1)&#x3E;",
    "<img src=x onerror=&#97;lert(1)>",
    "<a href=\"&#x26;#x6A;avascript:alert(1)\">x</a>",
    "<a href=\"&amp;#106;avascript:alert(1)\">x</a>",
    "<p>&#0;&#xD800;&#x110000;</p>",
    "\u{FF1C}script\u{FF1E}alert(1)\u{FF1C}/script\u{FF1E}",
    // Nested and malformed tags.
    "<<script>script>alert(1)<</script>/script>",
    "<scr\u{0}ipt>alert(1)</script>",
    "<p <script>alert(1)</script>>x</p>",
    "<a href=\"/x\"<script>alert(1)</script>>x</a>",
    "<b><i><a href=javascript:alert(1)>x</b></i>",
    "</p><script>alert(1)</script><p>",
    "<p>unclosed <b>bold <a href=\"/x\">link",
    "<!DOCTYPE html><html><head><script>alert(1)</script></head><body onload=alert(1)>x</body></html>",
    "<![CDATA[<script>alert(1)</script>]]>",
    "<?xml-stylesheet href=\"javascript:alert(1)\"?>",
    "<a href=\"/x\"><a href=\"javascript:alert(1)\">x</a></a>",
    "<table><a href=\"javascript:alert(1)\">x</a><tr><td>y</td></tr></table>",
    "<select><option><img src=x onerror=alert(1)></option></select>",
    "<!-- --!><img src=x onerror=alert(1)> -->",
    "<!--<img src=\"--><img src=x onerror=alert(1)>\">",
    "<img src=\"x\"\u{0}onerror=\"alert(1)\">",
    "<img src=`x`onerror=alert(1)>",
    "<a href=\"/x\" title=\"\"\"><img src=x onerror=alert(1)>",
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
        // The first `>` outside a quoted value: the serializer does not
        // escape `>` inside an attribute, so `title="a>b"` is one tag.
        let mut quoted = false;
        let end = after
            .char_indices()
            .find(|&(_, c)| {
                if c == '"' {
                    quoted = !quoted;
                }
                c == '>' && !quoted
            })
            .map(|(i, _)| i)
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

/// What a browser would see as the scheme of a URL, or `None` for a
/// relative URL.
///
/// The WHATWG URL parser strips C0 controls and spaces at the ends and tabs
/// and newlines anywhere. This removes every control and space, which is
/// stricter. A scheme is then an ASCII letter and letters, digits, `+`,
/// `-` or `.` up to a `:`. Anything else before the first `:` (a `/`, a
/// `U+FFFD` the parser put in place of a NUL) makes the URL relative.
fn scheme(url: &str) -> Option<String> {
    let cleaned: String = url
        .chars()
        .filter(|c| !c.is_ascii_control() && !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    let (head, _) = cleaned.split_once(':')?;
    let mut chars = head.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    valid.then(|| head.to_string())
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
    for input in VECTORS.iter().chain(MORE_VECTORS) {
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

#[test]
fn the_checker_reads_a_scheme_as_the_browser_does() {
    // Controls and spaces do not hide a scheme.
    assert_eq!(
        scheme("\u{1} Java\tScript:x").as_deref(),
        Some("javascript")
    );
    assert_eq!(scheme("data:text/html,x").as_deref(), Some("data"));
    // A character a scheme cannot hold makes the URL relative: the browser
    // resolves `ja\u{FFFD}vascript:x` against the page and runs nothing.
    assert_eq!(scheme("ja\u{FFFD}vascript:alert(1)"), None);
    assert_eq!(scheme("/a:b"), None);
    assert_eq!(scheme("1http:x"), None);
    assert_eq!(scheme("relative"), None);
}

#[test]
fn an_entity_in_text_stays_text() {
    for input in [
        "&lt;script&gt;alert(1)&lt;/script&gt;",
        "&#60;script&#62;alert(1)&#60;/script&#62;",
    ] {
        assert_eq!(
            comment(input).as_str(),
            "&lt;script&gt;alert(1)&lt;/script&gt;"
        );
        assert_eq!(
            text(input).as_str(),
            "&lt;script&gt;alert(1)&lt;/script&gt;"
        );
    }
}

#[test]
fn an_encoded_javascript_url_loses_its_href() {
    for input in [
        "<a href=\"&#106;avascript:alert(1)\">x</a>",
        "<a href=\"jav&#x09;ascript:alert(1)\">x</a>",
        "<a href=\"javascript&colon;alert(1)\">x</a>",
        "<a href=\"\u{1}javascript:alert(1)\">x</a>",
        "<a href=\"data:text/html,x\">x</a>",
    ] {
        assert_eq!(
            comment(input).as_str(),
            "<a rel=\"nofollow ugc noopener noreferrer\">x</a>",
            "{input}"
        );
    }
}

#[test]
fn a_data_image_loses_its_src() {
    assert_eq!(
        rich_text("<img src=\"data:image/png;base64,iVBORw0KGgo=\" alt=\"a\">").as_str(),
        "<img alt=\"a\">"
    );
}

#[test]
fn svg_and_math_go_with_their_content() {
    // ammonia drops an element of a foreign namespace with its content.
    assert_eq!(
        rich_text("<svg onload=alert(1)><text>hi</text></svg>").as_str(),
        ""
    );
    assert_eq!(
        rich_text("<math><mtext><img src=x onerror=alert(1)></mtext></math>").as_str(),
        ""
    );
}

#[test]
fn url_schemes_replaces_the_list() {
    let policy = Policy::comment().url_schemes(["https"]);
    assert_eq!(
        policy
            .clean("<a href=\"https://a.example/\">a</a><a href=\"http://b.example/\">b</a><a href=\"mailto:c@example.com\">c</a>")
            .as_str(),
        "<a href=\"https://a.example/\" rel=\"nofollow ugc noopener noreferrer\">a</a>\
         <a rel=\"nofollow ugc noopener noreferrer\">b</a>\
         <a rel=\"nofollow ugc noopener noreferrer\">c</a>"
    );
}

#[test]
fn a_comment_and_a_cdata_section_go() {
    assert_eq!(comment("a<!-- secret -->b").as_str(), "ab");
    // Outside svg, `<![CDATA[` opens a bogus comment that ends at the
    // first `>`: the rest is text, escaped.
    assert_eq!(
        comment("<![CDATA[<script>alert(1)</script>]]>x").as_str(),
        "alert(1)]]&gt;x"
    );
}

#[test]
fn the_user_rel_is_replaced_not_kept() {
    assert_eq!(
        comment("<a href=\"/x\" rel=\"opener\" target=\"_blank\">x</a>").as_str(),
        "<a href=\"/x\" rel=\"nofollow ugc noopener noreferrer\">x</a>"
    );
    assert_eq!(
        rich_text("<a href=\"/x\" rel=\"opener\">x</a>").as_str(),
        "<a href=\"/x\" rel=\"noopener noreferrer\">x</a>"
    );
}

#[test]
fn malformed_markup_is_closed() {
    assert_eq!(
        comment("<p>unclosed <b>bold <a href=\"/x\">link").as_str(),
        "<p>unclosed <b>bold <a href=\"/x\" rel=\"nofollow ugc noopener noreferrer\">link</a></b></p>"
    );
    assert_eq!(
        comment("<<script>script>alert(1)<</script>/script>").as_str(),
        "&lt;/script&gt;"
    );
}
