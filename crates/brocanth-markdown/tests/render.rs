//! What a page gets back from Markdown: the features an author uses, and
//! nothing a script could run.

use brocanth_markdown::{render, render_user, Markdown};

#[test]
fn commonmark_and_the_gfm_extensions_render() {
    let html = render(
        "## Results\n\nSome *emphasis*, ~~struck~~ and `code`.\n\n\
         | a | b |\n|:--|--:|\n| 1 | 2 |\n\n- [x] done\n- [ ] todo\n\n\
         A claim.[^src]\n\n[^src]: The source.\n",
    );
    let html = html.as_str();
    assert!(html.contains("<h2 id=\"results\">Results</h2>"), "{html}");
    assert!(html.contains("<em>emphasis</em>"), "{html}");
    assert!(html.contains("<del>struck</del>"), "{html}");
    assert!(html.contains("<code>code</code>"), "{html}");
    assert!(
        html.contains("<th style=\"text-align: left\">a</th>"),
        "{html}"
    );
    assert!(
        html.contains("<td style=\"text-align: right\">2</td>"),
        "{html}"
    );
    assert!(html.contains("type=\"checkbox\" checked"), "{html}");
    assert!(html.contains("<a href=\"#src\""), "{html}");
    assert!(html.contains("id=\"src\""), "{html}");
}

#[test]
fn two_headings_with_one_text_get_two_ids() {
    let html = render("## Notes\n\n## Notes\n\n## Notes {#mine}\n");
    let html = html.as_str();
    assert!(html.contains("id=\"notes\""), "{html}");
    assert!(html.contains("id=\"notes-1\""), "{html}");
    // No heading attributes: the braces are text, and the id comes from it.
    assert!(html.contains("id=\"notes-mine\""), "{html}");
}

#[test]
fn a_fenced_block_keeps_its_language_class_and_nothing_else() {
    let html = render("```rust\nfn main() {}\n```\n\n<code class=\"btn\">x</code>\n");
    let html = html.as_str();
    assert!(
        html.contains("<pre><code class=\"language-rust\">fn main() {}"),
        "{html}"
    );
    assert!(!html.contains("btn"), "{html}");
}

#[test]
fn script_handlers_and_bad_schemes_go() {
    for vector in [
        "<script>alert(1)</script>",
        "<img src=x onerror=alert(1)>",
        "[x](javascript:alert(1))",
        "[x](JaVaScRiPt:alert(1))",
        "![x](data:text/html;base64,PHNjcmlwdD4=)",
        "<a href=\"vbscript:x\">x</a>",
        "<svg><script>alert(1)</script></svg>",
        "<iframe src=\"https://evil.example\"></iframe>",
        "<p style=\"position:fixed\">x</p>",
        "<td style=\"text-align: center; background: url(x)\">x</td>",
        "<input type=\"text\" name=\"password\">",
        "<span class=\"hl-x\" onclick=\"y()\">x</span>",
    ] {
        for html in [render(vector), render_user(vector)] {
            let html = html.as_str().to_ascii_lowercase();
            for bad in [
                "<script",
                "onerror",
                "onclick",
                "javascript:",
                "vbscript:",
                "data:",
                "<iframe",
                "position:fixed",
                "background",
                "type=\"text\"",
                "<svg",
            ] {
                assert!(!html.contains(bad), "{vector} left {bad}: {html}");
            }
        }
    }
}

#[test]
fn a_link_keeps_its_ranking_in_authored_text_and_not_in_user_text() {
    let authored = render("[a](https://example.com)");
    assert!(
        authored.as_str().contains("rel=\"noopener noreferrer\""),
        "{authored}"
    );
    let user = render_user("[a](https://example.com)");
    assert!(
        user.as_str()
            .contains("rel=\"nofollow ugc noopener noreferrer\""),
        "{user}"
    );
}

#[test]
fn every_id_in_user_text_has_the_prefix_and_its_links_follow() {
    let html = render_user("## Location\n\n[up](#location)\n\n<h3 id=\"cookie\">x</h3>\n");
    let html = html.as_str();
    assert!(html.contains("id=\"user-content-location\""), "{html}");
    assert!(html.contains("href=\"#user-content-location\""), "{html}");
    assert!(html.contains("id=\"user-content-cookie\""), "{html}");
    assert!(!html.contains("id=\"cookie\""), "{html}");
}

#[test]
fn user_text_has_no_h1() {
    let html = render_user("# Shout\n");
    assert!(!html.as_str().contains("<h1"), "{html}");
    assert!(html.as_str().contains("Shout"), "{html}");
}

#[test]
fn headings_without_ids_on_request() {
    let html = Markdown::authored().without_heading_ids().render("## A\n");
    assert_eq!(html.as_str(), "<h2>A</h2>\n");
}

#[test]
fn a_highlighter_keeps_its_hl_classes_and_loses_any_other() {
    let md = Markdown::authored().highlight(|lang, code| {
        (lang == "rust").then(|| {
            format!(
                "<span class=\"hl-keyword evil\">{}</span><span class=\"btn\">x</span>",
                code.trim()
            )
        })
    });
    let html = md.render("```rust\nfn\n```\n\n```text\n<b>\n```\n\n<span class=\"btn\">y</span>\n");
    let html = html.as_str();
    assert!(
        html.contains("<code class=\"language-rust\"><span class=\"hl-keyword\">fn</span>"),
        "{html}"
    );
    assert!(!html.contains("btn"), "{html}");
    assert!(!html.contains("evil"), "{html}");
    // A language the highlighter leaves alone stays escaped text.
    assert!(html.contains("&lt;b&gt;"), "{html}");
}
