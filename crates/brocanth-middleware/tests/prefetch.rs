//! `assets/dpprefetch.js`: what can be checked without a browser.
//!
//! JavaScript has no test harness in this repository. So: the script is
//! checked as text for what inlining it in a page needs, `node --check`
//! parses it, and `node` runs it once over a stub document with just
//! enough DOM for the link rules. A machine without `node` skips the last
//! two and says so on stderr.

use std::io::Write;
use std::process::{Command, Stdio};

use brocanth_middleware::dp::PREFETCH;

#[test]
fn the_script_can_go_inline_and_only_runs_once_per_page() {
    assert!(
        !PREFETCH.contains("</script"),
        "inline in a <script> as it is"
    );
    assert!(
        PREFETCH.contains("if (window.__dpPrefetch) return;\n  window.__dpPrefetch = true;"),
        "the double-load guard, the same pattern dpmutate.js uses"
    );
    // A page carries both scripts, and each guards on its own name.
    assert!(brocanth_middleware::dp::SCRIPT.contains("window.__dpMutate"));
}

#[test]
fn inlined_in_a_page_the_script_ends_its_own_tag_once() {
    let page = format!(
        "<!doctype html><html dp-prefetch><body>\
         <a id=a href=\"/posts/7\">post</a>\
         <a id=b href=\"/logout\" dp-prefetch=\"false\">out</a>\
         <script>{PREFETCH}</script></body></html>"
    );
    assert_eq!(page.matches("<script>").count(), 1);
    assert_eq!(
        page.matches("</script>").count(),
        1,
        "the script does not end the tag early"
    );
}

fn node() -> Option<&'static str> {
    let ok = Command::new("node")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        let line = format!(
            "!! SKIPPED {}: no `node` on PATH\n",
            std::thread::current().name().unwrap_or("a test")
        );
        if let Ok(mut fd2) = std::fs::OpenOptions::new().write(true).open("/dev/stderr") {
            let _ = fd2.write_all(line.as_bytes());
        }
        return None;
    }
    Some("node")
}

#[test]
fn node_parses_it() {
    let Some(node) = node() else { return };
    let dir = std::env::temp_dir().join(format!("brocanth-prefetch-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("dpprefetch.js");
    std::fs::write(&file, PREFETCH).unwrap();
    let out = Command::new(node)
        .arg("--check")
        .arg(&file)
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A stub document: elements with attributes, `closest` over a parent
/// chain, and a `head` that records what was appended.
const HARNESS: &str = r#"
const appended = [];
class El {
  constructor(tag, attrs, parent) { this.tagName = tag; this.attrs = attrs; this.parentElement = parent ?? null; }
  getAttribute(n) { return n in this.attrs ? this.attrs[n] : null; }
  hasAttribute(n) { return n in this.attrs; }
  closest(sel) {
    const m = /^\[([a-z-]+)\]$/.exec(sel) ?? /^a\[([a-z-]+)\]$/.exec(sel);
    for (let at = this; at; at = at.parentElement) {
      if (sel.startsWith("a[") && at.tagName !== "A") continue;
      if (at.hasAttribute(m[1])) return at;
    }
    return null;
  }
}
globalThis.Element = El;
globalThis.window = globalThis;
globalThis.location = { href: "https://shop.test/posts?page=2#top", origin: "https://shop.test" };
Object.defineProperty(globalThis, "navigator", { value: { connection: { saveData: false, effectiveType: "4g" } }, configurable: true });
globalThis.document = {
  readyState: "complete",
  addEventListener() {},
  querySelectorAll() { return []; },
  createElement() { return { relList: { supports: (r) => r === "prefetch" } }; },
  head: { append(l) { appended.push(l.href); } },
};
"#;

const CASES: &str = r##"
const p = globalThis.dpprefetch;
const html = new El("HTML", { "dp-prefetch": "" });
const nav = new El("NAV", { "dp-prefetch": "viewport" }, html);
const a = (attrs, parent) => new El("A", attrs, parent ?? html);
const out = {
  inherited: p.strategyOf(a({ href: "/x" })),
  nested: p.strategyOf(a({ href: "/x" }, nav)),
  off: p.strategyOf(a({ href: "/x", "dp-prefetch": "false" })),
  none: p.strategyOf(new El("A", { href: "/x" })),
  same: p.urlOf(a({ href: "/about#team" })),
  other: p.urlOf(a({ href: "https://evil.test/" })),
  mail: p.urlOf(a({ href: "mailto:a@b.c" })),
  hash: p.urlOf(a({ href: "#reviews" })),
  thisPage: p.urlOf(a({ href: "/posts?page=2" })),
  write: p.urlOf(a({ href: "/posts/1", "dp-delete": "" })),
  download: p.urlOf(a({ href: "/export.csv", download: "" })),
  first: p.prefetch("https://shop.test/about"),
  again: p.prefetch("https://shop.test/about"),
};
navigator.connection.saveData = true;
out.saving = p.prefetch("https://shop.test/pricing");
out.appended = appended;
console.log(JSON.stringify(out));
"##;

#[test]
fn the_link_rules_hold_over_a_stub_document() {
    let Some(node) = node() else { return };
    let program = format!("{HARNESS}\n{PREFETCH}\n{CASES}");
    let mut child = Command::new(node)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(program.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let got = String::from_utf8_lossy(&out.stdout);
    let expected = concat!(
        r#"{"inherited":"hover","nested":"viewport","off":null,"none":null,"#,
        r#""same":"https://shop.test/about","other":null,"mail":null,"hash":null,"#,
        r#""thisPage":null,"write":null,"download":null,"first":true,"again":false,"#,
        r#""saving":false,"appended":["https://shop.test/about"]}"#,
    );
    assert_eq!(got.trim(), expected);
}
