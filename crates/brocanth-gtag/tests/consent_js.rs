//! The consent script itself, run under Node with a stand-in browser.
//!
//! Skipped when `node` is not on the `PATH`: the Rust tests already check
//! what the page carries, and this one checks what the script does with
//! it. Each case runs the real `SCRIPT` in a fresh `vm` context and prints
//! what it pushed to `dataLayer` and what it appended to `<head>`.

use std::process::Command;

use brocanth_gtag::{scrub, SCRIPT};

/// A browser with just enough DOM for the script.
const HARNESS: &str = r#"
const vm = require("vm");
const fs = require("fs");
const script = fs.readFileSync(process.argv[2], "utf8");
const [cfg, gpc, saved, href] = JSON.parse(process.argv[3]);
const appended = [];
const listeners = {};
const store = saved ? { "brocanth-consent": JSON.stringify(saved) } : {};
const banner = { hidden: true, addEventListener() {}, querySelector() { return null; } };
const window = {
  location: { href, hostname: "shop.example" },
  history: { pushState() {} },
  addEventListener(name, f) { listeners[name] = f; },
  localStorage: {
    getItem: (k) => (k in store ? store[k] : null),
    setItem: (k, v) => { store[k] = v; },
  },
};
const document = {
  referrer: "https://news.example/story?id=1",
  title: "Order for ann@example.com",
  cookie: "",
  head: { appendChild(el) { appended.push(el.src); } },
  getElementById(id) {
    if (id === "brocanth-gtag") return { textContent: JSON.stringify(cfg) };
    if (id === "brocanth-consent") return banner;
    return null;
  },
  createElement() { return {}; },
  addEventListener() {},
};
const navigator = { globalPrivacyControl: gpc };
vm.runInNewContext(script, { window, document, navigator, JSON, Date, encodeURIComponent });
const calls = window.dataLayer.map((args) => Array.from(args));
console.log(JSON.stringify({ calls, appended, banner: !banner.hidden,
  scrub: window.brocanthConsent.scrub(href) }));
"#;

/// What the script did, as Node printed it. A JSON reader is not worth a
/// dependency: the assertions look for substrings.
struct Run {
    out: String,
}

fn node() -> bool {
    Command::new("node").arg("--version").output().is_ok()
}

fn run(gpc: bool, saved: Option<&str>, href: &str) -> Run {
    let dir = std::env::temp_dir().join(format!(
        "brocanth-gtag-{}-{}",
        std::process::id(),
        href.len() + usize::from(gpc) * 7 + saved.map_or(0, str::len) * 13
    ));
    std::fs::create_dir_all(&dir).expect("a temp dir");
    let script = dir.join("consent.js");
    let harness = dir.join("harness.js");
    std::fs::write(&script, SCRIPT).expect("write the script");
    std::fs::write(&harness, HARNESS).expect("write the harness");
    let cfg = r#"{"id":"G-ABC123","mode":"basic","gpc":true,"days":182,"title":false,"allow":["utm_source"]}"#;
    let saved = saved.map_or("null".to_string(), |choice| {
        format!(r#"{{"choice":"{choice}","at":9999999999999}}"#)
    });
    let args = format!(r#"[{cfg},{gpc},{saved},"{href}"]"#);
    let output = Command::new("node")
        .arg(&harness)
        .arg(&script)
        .arg(args)
        .output()
        .expect("node runs");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Run {
        out: String::from_utf8(output.stdout).expect("utf-8"),
    }
}

#[test]
fn the_script_follows_the_consent_rules() {
    if !node() {
        eprintln!("skipped: no `node` on the PATH");
        return;
    }
    let href = "https://shop.example/u/ann@example.com?utm_source=news&token=x";

    // A first visit: the banner shows, every consent is denied, and no
    // Google script is requested.
    let first = run(false, None, href).out;
    assert!(first.contains(r#""banner":true"#), "{first}");
    assert!(first.contains(r#""analytics_storage":"denied""#), "{first}");
    assert!(first.contains(r#""appended":[]"#), "{first}");
    assert!(!first.contains(r#""event","page_view""#), "{first}");

    // Global Privacy Control: no banner, no script, not even a page view.
    let gpc = run(true, None, href).out;
    assert!(gpc.contains(r#""banner":false"#), "{gpc}");
    assert!(gpc.contains(r#""appended":[]"#), "{gpc}");

    // A saved acceptance: the tag loads and a scrubbed page view leaves.
    let granted = run(false, Some("granted"), href).out;
    assert!(
        granted.contains("googletagmanager.com/gtag/js?id=G-ABC123"),
        "{granted}"
    );
    assert!(
        granted.contains(r#""analytics_storage":"granted""#),
        "{granted}"
    );
    assert!(
        granted.contains(r#""page_location":"https://shop.example/u/redacted?utm_source=news""#),
        "{granted}"
    );
    assert!(
        !granted.contains("ann@example.com"),
        "the title holds an email: {granted}"
    );
    assert!(
        granted.contains(r#""page_referrer":"https://news.example""#),
        "{granted}"
    );

    // A saved refusal: no banner, no script.
    let denied = run(false, Some("denied"), href).out;
    assert!(denied.contains(r#""banner":false"#), "{denied}");
    assert!(denied.contains(r#""appended":[]"#), "{denied}");
}

#[test]
fn the_script_scrubs_like_its_rust_specification() {
    if !node() {
        eprintln!("skipped: no `node` on the PATH");
        return;
    }
    let allow = vec!["utm_source".to_string()];
    for href in [
        "https://shop.example/",
        "https://shop.example",
        "https://shop.example/a/b?utm_source=x&b=2#frag",
        "https://shop.example/u/ann%40example.com/x?utm_source=a%40b",
        "https://shop.example/?utm_source=&utm_source=y",
    ] {
        let out = run(true, None, href).out;
        let expected = format!(r#""scrub":"{}""#, scrub(href, &allow));
        assert!(out.contains(&expected), "{href}: {out}");
    }
}
