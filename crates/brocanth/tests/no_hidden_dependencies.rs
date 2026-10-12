//! The test that keeps the modularity claim honest.
//!
//! PLAN.md §8: *"A build of the core alone, with every feature off, that
//! asserts the dependency tree contains no TOML parser, no SQL driver, no
//! logging framework and no async runtime. Modularity claimed in a README
//! rots; modularity asserted in CI does not."*
//!
//! So this reads `cargo metadata`, walks the core's own normal and build
//! dependencies — dev-dependencies are excluded because they never reach a
//! consumer's binary, and the core has none anyway — and fails if any banned
//! crate is reachable. The failure names the crate, says what kind of thing it
//! is, and prints the path that pulled it in, because "serde is in your tree"
//! without the path is a bug report you cannot act on.
//!
//! It parses the metadata with the twenty lines of JSON reader at the bottom
//! of this file rather than with `serde_json`, for the obvious reason: a test
//! that asserts the core does not depend on serde cannot itself depend on
//! serde.

use std::collections::{HashMap, HashSet};
use std::process::Command;

/// What must never be reachable from the core, and what to call it when it is.
///
/// A rule matches a package name exactly, or as a prefix when it ends in `-`
/// or `_`, which is how crate families spell themselves (`tokio-util`,
/// `serde_json`, `sea-orm-migration`).
const BANNED: &[(&str, &str)] = &[
    // TOML parsers, and the serde satellites a TOML parser drags in.
    ("toml", "a TOML parser"),
    ("toml_", "a TOML parser"),
    ("toml-", "a TOML parser"),
    ("basic-toml", "a TOML parser"),
    ("taplo", "a TOML parser"),
    ("winnow", "a TOML parser's parser combinator"),
    ("serde_spanned", "a TOML parser's span type"),
    // Serialisation. The core takes already-built values; it never parses one.
    ("serde", "serde"),
    ("serde_", "serde"),
    ("serde-", "serde"),
    ("rmp", "serde"),
    ("ron", "serde"),
    ("yaml-rust", "a YAML parser"),
    ("serde_yaml", "serde"),
    // SQL drivers and ORMs. The ORM is recommended and absent (§8.4).
    ("sqlx", "a SQL driver"),
    ("sqlx-", "a SQL driver"),
    ("sea-orm", "a SQL driver"),
    ("sea-", "a SQL driver"),
    ("diesel", "a SQL driver"),
    ("rusqlite", "a SQL driver"),
    ("libsqlite3-sys", "a SQL driver"),
    ("postgres", "a SQL driver"),
    ("tokio-postgres", "a SQL driver"),
    ("mysql", "a SQL driver"),
    ("mysql_", "a SQL driver"),
    ("r2d2", "a connection pool"),
    ("deadpool", "a connection pool"),
    // Logging and tracing frameworks. The Crystal original does not assume a
    // log layer, and neither does this.
    ("log", "a logging framework"),
    ("tracing", "a logging framework"),
    ("tracing-", "a logging framework"),
    ("env_logger", "a logging framework"),
    ("slog", "a logging framework"),
    ("fern", "a logging framework"),
    ("fastlog", "a logging framework"),
    // Async runtimes. The core is synchronous; the server layer is a plugin's
    // problem and the application's choice.
    ("tokio", "an async runtime"),
    ("tokio-", "an async runtime"),
    ("tokio_", "an async runtime"),
    ("async-std", "an async runtime"),
    ("smol", "an async runtime"),
    ("async-io", "an async runtime"),
    ("async-executor", "an async runtime"),
    ("async-global-executor", "an async runtime"),
    ("mio", "an async runtime's reactor"),
    ("futures", "an async runtime"),
    ("futures-", "an async runtime"),
    ("actix-rt", "an async runtime"),
    ("embassy-executor", "an async runtime"),
];

fn banned_reason(name: &str) -> Option<&'static str> {
    BANNED.iter().find_map(|(rule, what)| {
        let hit = if rule.ends_with('-') || rule.ends_with('_') {
            name.starts_with(rule)
        } else {
            name == *rule
        };
        hit.then_some(*what)
    })
}

#[test]
fn the_core_links_no_toml_no_serde_no_sql_no_logging_and_no_async_runtime() {
    let meta = cargo_metadata();

    // package id -> package name
    let mut names: HashMap<String, String> = HashMap::new();
    for pkg in meta.field("packages").expect("packages").array() {
        let id = pkg.field("id").expect("package id").string().to_string();
        let name = pkg
            .field("name")
            .expect("package name")
            .string()
            .to_string();
        names.insert(id, name);
    }

    // package id -> ids of its normal and build dependencies
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    let resolve = meta
        .field("resolve")
        .expect("`cargo metadata` returned no resolve graph");
    for node in resolve.field("nodes").expect("resolve nodes").array() {
        let id = node.field("id").expect("node id").string().to_string();
        let mut out = Vec::new();
        for dep in node.field("deps").map(Json::array).unwrap_or_default() {
            let kinds = dep.field("dep_kinds").map(Json::array).unwrap_or_default();
            // `kind` is null for a normal dependency, "dev" or "build"
            // otherwise. A dev-dependency never reaches a consumer, so it is
            // not part of what the core makes anyone link.
            let reaches_a_consumer = kinds
                .iter()
                .any(|k| !matches!(k.field("kind").map(|v| v.is_string_eq("dev")), Some(true)));
            if reaches_a_consumer {
                out.push(dep.field("pkg").expect("dep pkg").string().to_string());
            }
        }
        edges.insert(id, out);
    }

    let root = names
        .iter()
        .find(|(_, name)| name.as_str() == "brocanth")
        .map(|(id, _)| id.clone())
        .expect("the core is not in its own metadata");

    // Breadth-first, remembering who pulled in what, so the message can print
    // the path rather than only the destination.
    let mut parent: HashMap<String, String> = HashMap::new();
    let mut seen: HashSet<String> = HashSet::from([root.clone()]);
    let mut queue = vec![root.clone()];
    let mut offences: Vec<String> = Vec::new();

    while let Some(id) = queue.pop() {
        for dep in edges.get(&id).map(Vec::as_slice).unwrap_or(&[]) {
            if !seen.insert(dep.clone()) {
                continue;
            }
            parent.insert(dep.clone(), id.clone());
            queue.push(dep.clone());

            let name = names.get(dep).map(String::as_str).unwrap_or(dep);
            if let Some(what) = banned_reason(name) {
                offences.push(format!(
                    "  {name} is {what}, pulled in by {}",
                    path_to(dep, &parent, &names)
                ));
            }
        }
    }

    assert!(
        offences.is_empty(),
        "the core must stay a router handing a request and a context to a controller \
         (PLAN.md §8), and these crates are now reachable from it:\n{}\n\
         Each of them belongs in a plugin crate that depends on the core, not in the \
         core. If one is genuinely needed here, the seam is wrong — widen the seam.",
        offences.join("\n")
    );

    // And the positive half of the claim, so that deleting the list above is
    // not a way to pass.
    let direct: HashSet<&str> = edges[&root].iter().map(|id| names[id].as_str()).collect();
    let mut direct: Vec<&str> = direct.into_iter().collect();
    direct.sort_unstable();
    assert_eq!(
        direct,
        vec!["http", "thiserror", "wayfind"],
        "the core's direct dependencies are fixed at three; adding a fourth is a \
         design decision, not a detail"
    );
}

fn path_to(id: &str, parent: &HashMap<String, String>, names: &HashMap<String, String>) -> String {
    let mut chain = vec![names.get(id).map(String::as_str).unwrap_or(id)];
    let mut cur = id;
    while let Some(up) = parent.get(cur) {
        chain.push(names.get(up).map(String::as_str).unwrap_or(up));
        cur = up;
    }
    chain.reverse();
    chain.join(" -> ")
}

fn cargo_metadata() -> Json {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
    let out = Command::new(&cargo)
        .args(["metadata", "--format-version", "1", "--manifest-path"])
        .arg(manifest)
        .output()
        .unwrap_or_else(|e| panic!("cannot run `{cargo} metadata`: {e}"));
    assert!(
        out.status.success(),
        "`cargo metadata` failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Json::parse(&String::from_utf8(out.stdout).expect("cargo metadata is not UTF-8"))
}

// ---------------------------------------------------------------------------
// Just enough JSON
// ---------------------------------------------------------------------------
//
// A test that asserts "the core does not depend on serde" cannot reach for
// serde_json to find out. This reads exactly what `cargo metadata` emits and
// panics on anything else.

#[derive(Debug, Clone, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    fn parse(text: &str) -> Json {
        let bytes: Vec<char> = text.chars().collect();
        let mut p = Parser { s: &bytes, i: 0 };
        p.skip_ws();
        let v = p.value();
        p.skip_ws();
        assert!(p.i >= p.s.len(), "trailing bytes after the JSON document");
        v
    }

    fn field(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn array(&self) -> Vec<&Json> {
        match self {
            Json::Array(items) => items.iter().collect(),
            _ => Vec::new(),
        }
    }

    fn string(&self) -> &str {
        match self {
            Json::String(s) => s,
            other => panic!("expected a JSON string, got {other:?}"),
        }
    }

    fn is_string_eq(&self, want: &str) -> bool {
        matches!(self, Json::String(s) if s == want)
    }
}

struct Parser<'a> {
    s: &'a [char],
    i: usize,
}

impl Parser<'_> {
    fn skip_ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_whitespace() {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: char) {
        assert_eq!(self.s[self.i], c, "expected `{c}` at byte {}", self.i);
        self.i += 1;
    }

    fn value(&mut self) -> Json {
        match self.s[self.i] {
            '{' => self.object(),
            '[' => self.array(),
            '"' => Json::String(self.string()),
            't' => {
                self.i += 4;
                Json::Bool(true)
            }
            'f' => {
                self.i += 5;
                Json::Bool(false)
            }
            'n' => {
                self.i += 4;
                Json::Null
            }
            _ => self.number(),
        }
    }

    fn object(&mut self) -> Json {
        self.eat('{');
        let mut pairs = Vec::new();
        self.skip_ws();
        if self.s[self.i] == '}' {
            self.i += 1;
            return Json::Object(pairs);
        }
        loop {
            self.skip_ws();
            let key = self.string();
            self.skip_ws();
            self.eat(':');
            self.skip_ws();
            pairs.push((key, self.value()));
            self.skip_ws();
            match self.s[self.i] {
                ',' => self.i += 1,
                '}' => {
                    self.i += 1;
                    return Json::Object(pairs);
                }
                c => panic!("expected `,` or `}}` in an object, got `{c}`"),
            }
        }
    }

    fn array(&mut self) -> Json {
        self.eat('[');
        let mut items = Vec::new();
        self.skip_ws();
        if self.s[self.i] == ']' {
            self.i += 1;
            return Json::Array(items);
        }
        loop {
            self.skip_ws();
            items.push(self.value());
            self.skip_ws();
            match self.s[self.i] {
                ',' => self.i += 1,
                ']' => {
                    self.i += 1;
                    return Json::Array(items);
                }
                c => panic!("expected `,` or `]` in an array, got `{c}`"),
            }
        }
    }

    fn string(&mut self) -> String {
        self.eat('"');
        let mut out = String::new();
        loop {
            let c = self.s[self.i];
            self.i += 1;
            match c {
                '"' => return out,
                '\\' => {
                    let esc = self.s[self.i];
                    self.i += 1;
                    out.push(match esc {
                        'n' => '\n',
                        't' => '\t',
                        'r' => '\r',
                        'b' => '\u{8}',
                        'f' => '\u{c}',
                        'u' => {
                            let hex: String = self.s[self.i..self.i + 4].iter().collect();
                            self.i += 4;
                            let n = u32::from_str_radix(&hex, 16).expect("bad \\u escape");
                            char::from_u32(n).unwrap_or('\u{fffd}')
                        }
                        other => other,
                    });
                }
                other => out.push(other),
            }
        }
    }

    fn number(&mut self) -> Json {
        let start = self.i;
        while self.i < self.s.len()
            && matches!(self.s[self.i], '0'..='9' | '-' | '+' | '.' | 'e' | 'E')
        {
            self.i += 1;
        }
        let text: String = self.s[start..self.i].iter().collect();
        Json::Number(text.parse().expect("not a number"))
    }
}

#[test]
fn the_json_reader_reads_what_cargo_metadata_emits() {
    let v = Json::parse(r#"{"a":[1,-2.5e3,true,false,null],"b":{"c":"x\ny A"},"d":[],"e":{}}"#);
    assert_eq!(v.field("a").unwrap().array().len(), 5);
    assert_eq!(v.field("b").unwrap().field("c").unwrap().string(), "x\ny A");
    assert!(v.field("d").unwrap().array().is_empty());
    assert_eq!(v.field("e").unwrap(), &Json::Object(vec![]));
    assert!(v.field("missing").is_none());
}

#[test]
fn the_ban_list_matches_families_and_leaves_the_three_dependencies_alone() {
    assert_eq!(banned_reason("toml"), Some("a TOML parser"));
    assert_eq!(banned_reason("toml_edit"), Some("a TOML parser"));
    assert_eq!(banned_reason("serde_json"), Some("serde"));
    assert_eq!(banned_reason("tokio-util"), Some("an async runtime"));
    assert_eq!(banned_reason("sea-orm-migration"), Some("a SQL driver"));

    for allowed in ["http", "wayfind", "thiserror", "bytes", "itoa", "memchr"] {
        assert_eq!(banned_reason(allowed), None, "{allowed} must stay allowed");
    }
}
