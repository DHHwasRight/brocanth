//! The catalog compiler. A build script calls [`compile`].
//!
//! ```ignore
//! // build.rs
//! fn main() {
//!     brocanth_i18n::build::compile("locales", "en").unwrap_or_else(|e| panic!("{e}"));
//! }
//! ```
//!
//! It reads every `<tag>.toml` in the directory, checks them against each
//! other, and writes `$OUT_DIR/i18n.rs`. Every problem in every file is
//! reported at once, each with its file, line and column, because fixing a
//! catalog one build at a time is how a translator gives up.
//!
//! # The format
//!
//! ```toml
//! title = "Your cart"                       # t::title(locale)
//! greeting = "Hello, {name}!"               # t::greeting(locale, name)
//! total = "Total: {amount:number}"          # t::total(locale, 1234)
//! paid = "Paid {amount:number} on {day:date}"  # t::paid(locale, t::Paid { amount, day })
//!
//! [cart_items]                              # t::cart_items(locale, count)
//! "=0" = "Your cart is empty"
//! one = "{count} item"
//! other = "{count} items"
//!
//! [nav]                                     # t::nav::home(locale)
//! home = "Home"
//! ```
//!
//! - A key is a Rust function name: lower-case ASCII, digits and `_`, not a
//!   keyword.
//! - A table that has a CLDR category (`zero`, `one`, `two`, `few`, `many`,
//!   `other`) or an exact match (`"=0"`) is a plural message, and must have
//!   `other`. Any other table is a namespace, and becomes a module.
//! - `{name}` takes anything `Display`. `{name:number}` takes an `i64` and
//!   groups its digits the locale's way. `{name:date}`, `{name:date_short}`
//!   and `{name:date_long}` take a [`Date`](crate::Date) (feature `dates`).
//!   In a plural message, `{count}` is the count, formatted as a number.
//! - `{{` and `}}` are a literal brace.
//! - A message with one argument takes it after the locale. A message with
//!   two or more takes a struct named after the key, so the call site names
//!   every argument and two of them cannot be swapped.

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::path::{Path, PathBuf};

use serde_spanned::Spanned;
use toml::de::{DeTable, DeValue};

/// One catalog file.
#[derive(Clone, Debug)]
pub struct Catalog {
    /// The BCP 47 tag: the file name without `.toml`.
    pub tag: String,
    /// The path errors name.
    pub path: String,
    pub text: String,
}

/// Everything wrong with the catalogs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub problems: Vec<String>,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let n = self.problems.len();
        writeln!(
            f,
            "brocanth-i18n: {n} problem{} in the catalogs",
            if n == 1 { "" } else { "s" }
        )?;
        for p in &self.problems {
            writeln!(f, "  {p}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {}

/// The generated source, and what was worth saying without failing.
#[derive(Clone, Debug)]
pub struct Generated {
    pub code: String,
    /// One line per key a translation lacks and takes from a fallback.
    pub warnings: Vec<String>,
}

/// Compiles `dir` into `$OUT_DIR/i18n.rs`, for a build script.
///
/// `dir` is relative to `CARGO_MANIFEST_DIR`. `default` is the tag of the
/// catalog every other one is checked against. Prints `rerun-if-changed`
/// for the directory and each file, and a `cargo:warning` per fallback.
///
/// # Errors
///
/// Every problem in every file, and I/O failures.
pub fn compile(dir: impl AsRef<Path>, default: &str) -> Result<Generated, Error> {
    let root = std::env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_default();
    let dir = root.join(dir.as_ref());
    println!("cargo:rerun-if-changed={}", dir.display());
    let catalogs = read_dir(&dir)?;
    for c in &catalogs {
        println!(
            "cargo:rerun-if-changed={}",
            dir.join(format!("{}.toml", c.tag)).display()
        );
    }
    let generated = generate(&catalogs, default)?;
    for w in &generated.warnings {
        println!("cargo:warning={w}");
    }
    let out = std::env::var_os("OUT_DIR").ok_or_else(|| Error {
        problems: vec!["OUT_DIR is not set: call `compile` from a build script".to_string()],
    })?;
    let out = Path::new(&out).join("i18n.rs");
    std::fs::write(&out, &generated.code).map_err(|e| Error {
        problems: vec![format!("{}: {e}", out.display())],
    })?;
    Ok(generated)
}

/// Reads every `*.toml` in `dir`, sorted by file name.
///
/// # Errors
///
/// The directory or a file cannot be read.
pub fn read_dir(dir: &Path) -> Result<Vec<Catalog>, Error> {
    let io = |e: std::io::Error| Error {
        problems: vec![format!("{}: {e}", dir.display())],
    };
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(io)? {
        let path = entry.map_err(io)?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let Some(tag) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let text = std::fs::read_to_string(&path).map_err(io)?;
        out.push(Catalog {
            tag: tag.to_string(),
            path: path.display().to_string(),
            text,
        });
    }
    out.sort_by(|a, b| a.tag.cmp(&b.tag));
    Ok(out)
}

/// Checks the catalogs against each other and writes the Rust.
///
/// Pure: no file system, no environment. [`compile`] is this plus the I/O.
///
/// # Errors
///
/// Every problem found, each naming a file and, where it has one, a line.
pub fn generate(catalogs: &[Catalog], default: &str) -> Result<Generated, Error> {
    let mut problems = Vec::new();

    // Tags first: a bad file name poisons everything below it.
    let mut seen: BTreeMap<String, &str> = BTreeMap::new();
    for c in catalogs {
        if !is_tag(&c.tag) {
            problems.push(format!(
                "{}: `{}` is not a BCP 47 tag; name the file `en.toml`, `fr-CA.toml`",
                c.path, c.tag
            ));
        }
        if let Some(first) = seen.insert(c.tag.to_ascii_lowercase(), &c.path) {
            problems.push(format!("{}: the same locale as {first}", c.path));
        }
    }
    let Some(default_index) = catalogs
        .iter()
        .position(|c| c.tag.eq_ignore_ascii_case(default))
    else {
        let found: Vec<&str> = catalogs.iter().map(|c| c.tag.as_str()).collect();
        problems.push(format!(
            "no catalog for the default locale `{default}`; found: {}",
            if found.is_empty() {
                "none".to_string()
            } else {
                found.join(", ")
            }
        ));
        return Err(Error { problems });
    };
    if !problems.is_empty() {
        return Err(Error { problems });
    }

    // The default first, then the rest in file-name order.
    let mut order: Vec<usize> = vec![default_index];
    order.extend((0..catalogs.len()).filter(|&i| i != default_index));
    let catalogs: Vec<&Catalog> = order.iter().map(|&i| &catalogs[i]).collect();

    let mut parsed = Vec::new();
    for c in &catalogs {
        let mut messages = BTreeMap::new();
        match DeTable::parse(&c.text) {
            Ok(table) => walk(
                c,
                table.into_inner(),
                &mut Vec::new(),
                &mut messages,
                &mut problems,
            ),
            Err(e) => {
                let at = e.span().map_or((1, 1), |s| line_col(&c.text, s.start));
                problems.push(format!(
                    "{}:{}:{}: {}",
                    c.path,
                    at.0,
                    at.1,
                    e.message().trim()
                ));
            }
        }
        parsed.push(messages);
    }

    let locales = Locales::new(&catalogs);
    let mut warnings = Vec::new();
    let default_messages = &parsed[0];
    for (i, messages) in parsed.iter().enumerate().skip(1) {
        let c = catalogs[i];
        for (key, msg) in messages {
            let Some(base) = default_messages.get(key) else {
                problems.push(format!(
                    "{}:{}: `{}` is not in the default locale `{}` ({}); add it there first",
                    c.path,
                    msg.line,
                    key.join("."),
                    catalogs[0].tag,
                    catalogs[0].path
                ));
                continue;
            };
            compare(c, catalogs[0], key, base, msg, &mut problems);
        }
        for key in default_messages.keys() {
            if !messages.contains_key(key) {
                let from = locales.resolve(i, key, &parsed);
                warnings.push(format!(
                    "{}: no `{}`; `{}` falls back to `{}`",
                    c.path,
                    key.join("."),
                    c.tag,
                    catalogs[from].tag
                ));
            }
        }
    }
    check_names(default_messages, catalogs[0], &mut problems);

    if !problems.is_empty() {
        return Err(Error { problems });
    }
    Ok(Generated {
        code: emit(&catalogs, &locales, &parsed),
        warnings,
    })
}

// ---------------------------------------------------------------------------
// The model
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Display,
    Number,
    Date(&'static str),
    /// `{count}` in a plural message.
    Count,
}

impl Kind {
    /// What the argument's type is. Two kinds with the same class may differ
    /// between locales: one translation writes a long date, another a short
    /// one, and the call site passes the same `Date`.
    fn class(self) -> &'static str {
        match self {
            Kind::Display => "anything Display",
            Kind::Number => "number",
            Kind::Date(_) => "date",
            Kind::Count => "count",
        }
    }

    fn rust_type(self, lifetime: bool) -> &'static str {
        match (self, lifetime) {
            (Kind::Display, false) => "impl ::core::fmt::Display",
            (Kind::Display, true) => "&'a dyn ::core::fmt::Display",
            (Kind::Number, _) => "i64",
            (Kind::Date(_), _) => "::brocanth_i18n::Date",
            (Kind::Count, _) => "u64",
        }
    }
}

#[derive(Clone, Debug)]
enum Piece {
    Text(String),
    Hole(String, Kind),
}

#[derive(Clone, Debug)]
struct Text {
    pieces: Vec<Piece>,
}

impl Text {
    fn holes(&self) -> impl Iterator<Item = (&str, Kind)> {
        self.pieces.iter().filter_map(|p| match p {
            Piece::Hole(name, kind) => Some((name.as_str(), *kind)),
            Piece::Text(_) => None,
        })
    }
}

#[derive(Clone, Debug)]
enum Body {
    Plain(Text),
    Plural {
        exact: Vec<(u64, Text)>,
        /// Every category but `other`, in CLDR order.
        categories: Vec<(&'static str, Text)>,
        other: Text,
    },
}

#[derive(Clone, Debug)]
struct Message {
    line: usize,
    body: Body,
    /// The arguments other than `count`, by name.
    holes: BTreeMap<String, Kind>,
}

impl Message {
    fn is_plural(&self) -> bool {
        matches!(self.body, Body::Plural { .. })
    }

    fn texts(&self) -> Vec<&Text> {
        match &self.body {
            Body::Plain(t) => vec![t],
            Body::Plural {
                exact,
                categories,
                other,
            } => exact
                .iter()
                .map(|(_, t)| t)
                .chain(categories.iter().map(|(_, t)| t))
                .chain([other])
                .collect(),
        }
    }

    fn uses_count(&self) -> bool {
        self.texts()
            .iter()
            .any(|t| t.holes().any(|(_, k)| k == Kind::Count))
    }
}

type Key = Vec<String>;
type Messages = BTreeMap<Key, Message>;

const CATEGORIES: [&str; 6] = ["zero", "one", "two", "few", "many", "other"];

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

fn walk(
    c: &Catalog,
    table: DeTable<'_>,
    prefix: &mut Vec<String>,
    out: &mut Messages,
    problems: &mut Vec<String>,
) {
    for (key, value) in table {
        let (line, col) = line_col(&c.text, key.span().start);
        let name = key.into_inner().into_owned();
        let at = format!("{}:{line}:{col}", c.path);
        if let Some(why) = bad_ident(&name) {
            problems.push(format!("{at}: the key `{name}` {why}"));
            continue;
        }
        let mut path = prefix.clone();
        path.push(name);
        match value.into_inner() {
            DeValue::String(s) => {
                let text = parse_text(&s, false, &at, &path, problems);
                let holes = holes_of(&[&text], &at, &path, problems);
                out.insert(
                    path,
                    Message {
                        line,
                        body: Body::Plain(text),
                        holes,
                    },
                );
            }
            DeValue::Table(inner) => {
                let plural = inner.keys().any(|k| {
                    let k: &str = k.get_ref();
                    CATEGORIES.contains(&k) || k.starts_with('=')
                });
                if plural {
                    if let Some(msg) = plural_message(c, inner, line, &at, &path, problems) {
                        out.insert(path, msg);
                    }
                } else {
                    prefix.push(path.pop().expect("pushed above"));
                    walk(c, inner, prefix, out, problems);
                    prefix.pop();
                }
            }
            _ => problems.push(format!(
                "{at}: `{}` is not a string or a table; a message is text",
                path.join(".")
            )),
        }
    }
}

fn plural_message(
    c: &Catalog,
    table: DeTable<'_>,
    line: usize,
    at: &str,
    path: &Key,
    problems: &mut Vec<String>,
) -> Option<Message> {
    let mut exact = Vec::new();
    let mut found: BTreeMap<&'static str, Text> = BTreeMap::new();
    let mut ok = true;
    for (key, value) in table {
        let (l, col) = line_col(&c.text, key.span().start);
        let here = format!("{}:{l}:{col}", c.path);
        let name = key.into_inner();
        let Some(text) = string(value) else {
            problems.push(format!(
                "{here}: `{}.{name}` is not a string; each case of a plural is text",
                path.join(".")
            ));
            ok = false;
            continue;
        };
        let text = parse_text(&text, true, &here, path, problems);
        if let Some(n) = name.strip_prefix('=') {
            match n.parse::<u64>() {
                Ok(n) => exact.push((n, text)),
                Err(_) => {
                    problems.push(format!(
                        "{here}: `{name}` in `{}` is not an exact count; write `\"=0\"`",
                        path.join(".")
                    ));
                    ok = false;
                }
            }
        } else if let Some(cat) = CATEGORIES.iter().find(|c| **c == name) {
            found.insert(cat, text);
        } else {
            problems.push(format!(
                "{here}: `{name}` in the plural `{}` is not a CLDR category; \
                 use zero, one, two, few, many, other or \"=N\"",
                path.join(".")
            ));
            ok = false;
        }
    }
    let Some(other) = found.remove("other") else {
        problems.push(format!(
            "{at}: the plural `{}` has no `other`, and every locale needs it",
            path.join(".")
        ));
        return None;
    };
    exact.sort_by_key(|(n, _)| *n);
    let categories: Vec<(&'static str, Text)> = CATEGORIES
        .iter()
        .filter_map(|cat| found.remove(cat).map(|t| (*cat, t)))
        .collect();
    let mut texts: Vec<&Text> = exact.iter().map(|(_, t)| t).collect();
    texts.extend(categories.iter().map(|(_, t)| t));
    texts.push(&other);
    let holes = holes_of(&texts, at, path, problems);
    ok.then_some(Message {
        line,
        body: Body::Plural {
            exact,
            categories,
            other,
        },
        holes,
    })
}

fn string(value: Spanned<DeValue<'_>>) -> Option<String> {
    match value.into_inner() {
        DeValue::String(s) => Some(s.into_owned()),
        _ => None,
    }
}

/// `"Hello, {name}!"` → `[Text("Hello, "), Hole(name), Text("!")]`.
fn parse_text(
    source: &str,
    plural: bool,
    at: &str,
    path: &Key,
    problems: &mut Vec<String>,
) -> Text {
    let key = path.join(".");
    let mut pieces = Vec::new();
    let mut text = String::new();
    let mut chars = source.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                text.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                text.push('}');
            }
            '}' => {
                problems.push(format!(
                    "{at}: `{key}` has a `}}` with no `{{`; write `}}}}` for a literal brace"
                ));
            }
            '{' => {
                let mut inner = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == '}' {
                        closed = true;
                        break;
                    }
                    inner.push(c);
                }
                if !closed {
                    problems.push(format!(
                        "{at}: `{key}` opens `{{{inner}` and never closes it; \
                         write `{{{{` for a literal brace"
                    ));
                    break;
                }
                let (name, kind) = match inner.split_once(':') {
                    Some((n, k)) => (n.trim(), Some(k.trim())),
                    None => (inner.trim(), None),
                };
                let why = bad_ident(name).or_else(|| {
                    RESERVED
                        .contains(&name)
                        .then_some("is a name the generated function uses itself")
                });
                if let Some(why) = why {
                    problems.push(format!(
                        "{at}: `{key}` has the placeholder `{{{inner}}}`, whose name {why}"
                    ));
                    continue;
                }
                let kind = match (kind, plural && name == "count") {
                    (None, true) => Kind::Count,
                    (Some(_), true) => {
                        problems.push(format!(
                            "{at}: `{key}` gives `{{count}}` a type; in a plural, `count` is \
                             always the number the plural is chosen by, so write `{{count}}`"
                        ));
                        continue;
                    }
                    (None, false) => Kind::Display,
                    (Some("number"), false) => Kind::Number,
                    (Some("date"), false) => Kind::Date("Medium"),
                    (Some("date_short"), false) => Kind::Date("Short"),
                    (Some("date_long"), false) => Kind::Date("Long"),
                    (Some(other), false) => {
                        problems.push(format!(
                            "{at}: `{key}` has `{{{name}:{other}}}`; the types are number, date, \
                             date_short and date_long"
                        ));
                        continue;
                    }
                };
                if !text.is_empty() {
                    pieces.push(Piece::Text(std::mem::take(&mut text)));
                }
                pieces.push(Piece::Hole(name.to_string(), kind));
            }
            c => text.push(c),
        }
    }
    if !text.is_empty() {
        pieces.push(Piece::Text(text));
    }
    Text { pieces }
}

/// The arguments of one message in one locale, `count` excluded. A name used
/// twice with two types is refused here.
fn holes_of(
    texts: &[&Text],
    at: &str,
    path: &Key,
    problems: &mut Vec<String>,
) -> BTreeMap<String, Kind> {
    let mut out: BTreeMap<String, Kind> = BTreeMap::new();
    for t in texts {
        for (name, kind) in t.holes() {
            if kind == Kind::Count {
                continue;
            }
            match out.get(name) {
                Some(prev) if prev.class() != kind.class() => problems.push(format!(
                    "{at}: `{}` uses `{{{name}}}` as {} and as {}",
                    path.join("."),
                    prev.class(),
                    kind.class()
                )),
                Some(_) => {}
                None => {
                    out.insert(name.to_string(), kind);
                }
            }
        }
    }
    out
}

/// A translation against the default, for one key.
fn compare(
    c: &Catalog,
    base_catalog: &Catalog,
    key: &Key,
    base: &Message,
    msg: &Message,
    problems: &mut Vec<String>,
) {
    let at = format!("{}:{}", c.path, msg.line);
    let name = key.join(".");
    if base.is_plural() != msg.is_plural() {
        let shape = |m: &Message| {
            if m.is_plural() {
                "a plural"
            } else {
                "plain text"
            }
        };
        problems.push(format!(
            "{at}: `{name}` is {} here and {} in `{}`; the call site takes one shape",
            shape(msg),
            shape(base),
            base_catalog.tag
        ));
        return;
    }
    for (hole, kind) in &msg.holes {
        match base.holes.get(hole) {
            None => problems.push(format!(
                "{at}: `{name}` has the placeholder `{{{hole}}}` and `{}` does not; \
                 a translation cannot ask for an argument the call site does not pass",
                base_catalog.tag
            )),
            Some(b) if b.class() != kind.class() => problems.push(format!(
                "{at}: `{name}` takes `{{{hole}}}` as {} here and as {} in `{}`",
                kind.class(),
                b.class(),
                base_catalog.tag
            )),
            Some(_) => {}
        }
    }
    for hole in base.holes.keys() {
        if !msg.holes.contains_key(hole) {
            problems.push(format!(
                "{at}: `{name}` lacks the placeholder `{{{hole}}}` that `{}` has; \
                 the value would be dropped without a word",
                base_catalog.tag
            ));
        }
    }
}

/// Two keys whose argument structs would have the same name.
fn check_names(messages: &Messages, c: &Catalog, problems: &mut Vec<String>) {
    let mut structs: BTreeMap<(Vec<String>, String), &Key> = BTreeMap::new();
    for (key, msg) in messages {
        if msg.holes.len() < 2 {
            continue;
        }
        let (module, last) = key.split_at(key.len() - 1);
        let name = camel(&last[0]);
        if let Some(prev) = structs.insert((module.to_vec(), name.clone()), key) {
            problems.push(format!(
                "{}:{}: `{}` and `{}` would both take arguments as `{name}`; rename one",
                c.path,
                msg.line,
                prev.join("."),
                key.join(".")
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// Fallback
// ---------------------------------------------------------------------------

struct Locales {
    tags: Vec<String>,
    variants: Vec<String>,
    /// Index of the parent catalog, by removing subtags. `None`: the default
    /// is next.
    parents: Vec<Option<usize>>,
}

impl Locales {
    fn new(catalogs: &[&Catalog]) -> Self {
        let tags: Vec<String> = catalogs.iter().map(|c| c.tag.clone()).collect();
        let parents = tags
            .iter()
            .enumerate()
            .map(|(i, tag)| {
                let mut t = tag.as_str();
                while let Some(cut) = t.rfind('-') {
                    t = &t[..cut];
                    if let Some(p) = tags.iter().position(|o| o.eq_ignore_ascii_case(t)) {
                        return (p != i).then_some(p);
                    }
                }
                None
            })
            .collect();
        let variants = tags.iter().map(|t| camel(t)).collect();
        Self {
            tags,
            variants,
            parents,
        }
    }

    /// The catalog whose text locale `i` shows for `key`.
    fn resolve(&self, i: usize, key: &Key, parsed: &[Messages]) -> usize {
        let mut at = i;
        loop {
            if parsed[at].contains_key(key) {
                return at;
            }
            match self.parents[at] {
                Some(p) => at = p,
                None => return 0,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

fn emit(catalogs: &[&Catalog], locales: &Locales, parsed: &[Messages]) -> String {
    let mut o = String::new();
    let files: Vec<String> = catalogs.iter().map(|c| format!("{}.toml", c.tag)).collect();
    let _ = writeln!(
        o,
        "// @generated by brocanth-i18n from {}. Edit those, not this.\n",
        files.join(", ")
    );
    emit_enum(&mut o, locales);

    o.push_str("/// One function per catalog key.\n");
    o.push_str("pub mod t {\n");
    let mut open: Vec<String> = Vec::new();
    for (key, base) in &parsed[0] {
        let (module, last) = key.split_at(key.len() - 1);
        // Close and open modules so `open` matches `module`.
        let common = open.iter().zip(module).take_while(|(a, b)| a == b).count();
        while open.len() > common {
            open.pop();
            let _ = writeln!(o, "{}}}", indent(open.len() + 1));
        }
        for m in &module[common..] {
            let _ = writeln!(o, "\n{}pub mod {m} {{", indent(open.len() + 1));
            open.push(m.clone());
        }
        emit_message(&mut o, open.len() + 1, &last[0], key, base, locales, parsed);
    }
    while !open.is_empty() {
        open.pop();
        let _ = writeln!(o, "{}}}", indent(open.len() + 1));
    }
    o.push_str("}\n");
    o
}

fn emit_enum(o: &mut String, l: &Locales) {
    let n = l.tags.len();
    o.push_str("/// A locale with a catalog. One variant per file.\n");
    o.push_str("#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]\n");
    o.push_str("pub enum Locale {\n");
    for i in 0..n {
        let doc = match l.parents[i] {
            _ if i == 0 => format!("`{}`, the default", l.tags[i]),
            Some(p) => format!("`{}`, then `{}`", l.tags[i], l.tags[p]),
            None => format!("`{}`", l.tags[i]),
        };
        let _ = writeln!(o, "    /// {doc}\n    {},", l.variants[i]);
    }
    o.push_str("}\n\n");

    o.push_str("impl ::brocanth_i18n::Locales for Locale {\n");
    let all: Vec<String> = l.variants.iter().map(|v| format!("Self::{v}")).collect();
    let _ = writeln!(o, "    const ALL: &'static [Self] = &[{}];", all.join(", "));
    let _ = writeln!(o, "    const DEFAULT: Self = Self::{};\n", l.variants[0]);
    o.push_str("    fn tag(self) -> &'static str {\n        match self {\n");
    for i in 0..n {
        let _ = writeln!(o, "            Self::{} => {:?},", l.variants[i], l.tags[i]);
    }
    o.push_str("        }\n    }\n\n");
    o.push_str("    fn parent(self) -> ::core::option::Option<Self> {\n        match self {\n");
    let mut none = Vec::new();
    for i in 0..n {
        match l.parents[i] {
            Some(p) => {
                let _ = writeln!(
                    o,
                    "            Self::{} => ::core::option::Option::Some(Self::{}),",
                    l.variants[i], l.variants[p]
                );
            }
            None => none.push(format!("Self::{}", l.variants[i])),
        }
    }
    let _ = writeln!(
        o,
        "            {} => ::core::option::Option::None,",
        none.join(" | ")
    );
    o.push_str("        }\n    }\n}\n\n");

    o.push_str("impl Locale {\n");
    o.push_str("    /// CLDR plural rules and formatters for this locale, built on first use.\n");
    o.push_str("    pub fn formats(self) -> &'static ::brocanth_i18n::Formats {\n");
    let _ = writeln!(
        o,
        "        static CELLS: [::std::sync::OnceLock<::brocanth_i18n::Formats>; {n}] =\n            \
         [const {{ ::std::sync::OnceLock::new() }}; {n}];"
    );
    o.push_str(
        "        CELLS[self as usize]\n            \
         .get_or_init(|| ::brocanth_i18n::Formats::new(::brocanth_i18n::Locales::tag(self)))\n",
    );
    o.push_str("    }\n}\n\n");

    o.push_str("impl ::core::fmt::Display for Locale {\n");
    o.push_str("    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {\n");
    o.push_str("        f.write_str(::brocanth_i18n::Locales::tag(*self))\n    }\n}\n\n");
}

fn emit_message(
    o: &mut String,
    depth: usize,
    name: &str,
    key: &Key,
    base: &Message,
    locales: &Locales,
    parsed: &[Messages],
) {
    let pad = indent(depth);
    let locale_ty = format!("{}Locale", "super::".repeat(depth));
    let holes: Vec<(&String, &Kind)> = base.holes.iter().collect();
    let plural = base.is_plural();

    // The signature.
    let mut params = vec![format!("locale: {locale_ty}")];
    if plural {
        params.push("count: u64".to_string());
    }
    let mut prologue = String::new();
    let doc_text = match &base.body {
        Body::Plain(t) => source_of(t),
        Body::Plural { other, .. } => source_of(other),
    };
    o.push('\n');
    match holes.len() {
        0 => {}
        1 => params.push(format!("{}: {}", holes[0].0, holes[0].1.rust_type(false))),
        _ => {
            let ty = camel(name);
            let lifetime = holes.iter().any(|(_, k)| **k == Kind::Display);
            let generics = if lifetime { "<'a>" } else { "" };
            let _ = writeln!(
                o,
                "{pad}/// The arguments of [`{name}`], by name, so two cannot be swapped."
            );
            let _ = writeln!(o, "{pad}#[derive(Clone, Copy)]");
            let _ = writeln!(o, "{pad}pub struct {ty}{generics} {{");
            for (h, k) in &holes {
                let _ = writeln!(o, "{pad}    pub {h}: {},", k.rust_type(lifetime));
            }
            let _ = writeln!(o, "{pad}}}\n");
            params.push(format!("args: {ty}{}", if lifetime { "<'_>" } else { "" }));
            let fields: Vec<&str> = holes.iter().map(|(h, _)| h.as_str()).collect();
            let _ = writeln!(
                prologue,
                "{pad}    let {ty} {{ {} }} = args;",
                fields.join(", ")
            );
        }
    }
    let _ = writeln!(o, "{pad}/// `{}`: {doc_text:?}", locales.tags[0]);
    let _ = writeln!(
        o,
        "{pad}pub fn {name}({}) -> ::std::string::String {{",
        params.join(", ")
    );
    o.push_str(&prologue);
    if plural
        && !parsed
            .iter()
            .any(|m| m.get(key).is_some_and(Message::uses_count))
    {
        // Every case of every locale ignores the number. Legal, if odd: the
        // categories still choose the text.
        let _ = writeln!(o, "{pad}    let _ = count;");
    }

    // One arm per catalog that some locale shows, listing those locales.
    let mut arms: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..locales.tags.len() {
        arms.entry(locales.resolve(i, key, parsed))
            .or_default()
            .push(i);
    }
    let _ = writeln!(o, "{pad}    match locale {{");
    for (from, shown_by) in &arms {
        let pats: Vec<String> = shown_by
            .iter()
            .map(|&i| format!("{locale_ty}::{}", locales.variants[i]))
            .collect();
        let msg = &parsed[*from][key];
        let source = format!("{locale_ty}::{}", locales.variants[*from]);
        let _ = writeln!(o, "{pad}        {} => {{", pats.join(" | "));
        emit_body(o, depth + 3, msg, &source);
        let _ = writeln!(o, "{pad}        }}");
    }
    let _ = writeln!(o, "{pad}    }}");
    let _ = writeln!(o, "{pad}}}");
}

/// The body of one arm. `source` is the locale whose text this is, and so
/// whose plural rules and number format apply: an English fallback on a
/// French page says "0 items", not "0 item".
fn emit_body(o: &mut String, depth: usize, msg: &Message, source: &str) {
    let pad = indent(depth);
    let needs_formats = match &msg.body {
        Body::Plain(t) => t.holes().any(|(_, k)| k != Kind::Display),
        Body::Plural { categories, .. } => {
            !categories.is_empty()
                || msg
                    .texts()
                    .iter()
                    .any(|t| t.holes().any(|(_, k)| k != Kind::Display))
        }
    };
    if needs_formats {
        let _ = writeln!(o, "{pad}let f = {source}.formats();");
    }
    match &msg.body {
        Body::Plain(t) => {
            let (lets, expr) = render_parts(t);
            for l in lets {
                let _ = writeln!(o, "{pad}{l}");
            }
            let _ = writeln!(o, "{pad}{expr}");
        }
        Body::Plural {
            exact,
            categories,
            other,
        } => {
            let category_match = |o: &mut String, pad: &str| {
                if categories.is_empty() {
                    let _ = writeln!(o, "{pad}{}", render(other, pad));
                    return;
                }
                let _ = writeln!(o, "{pad}match f.plural(count) {{");
                for (cat, t) in categories {
                    let _ = writeln!(
                        o,
                        "{pad}    ::brocanth_i18n::Plural::{} => {},",
                        camel(cat),
                        render(t, &format!("{pad}    "))
                    );
                }
                let _ = writeln!(o, "{pad}    _ => {},", render(other, &format!("{pad}    ")));
                let _ = writeln!(o, "{pad}}}");
            };
            if exact.is_empty() {
                category_match(o, &pad);
            } else {
                let _ = writeln!(o, "{pad}match count {{");
                for (n, t) in exact {
                    let _ = writeln!(o, "{pad}    {n} => {},", render(t, &format!("{pad}    ")));
                }
                let _ = writeln!(o, "{pad}    _ => {{");
                category_match(o, &format!("{pad}        "));
                let _ = writeln!(o, "{pad}    }}");
                let _ = writeln!(o, "{pad}}}");
            }
        }
    }
}

/// One text as an expression producing a `String`.
fn render(t: &Text, pad: &str) -> String {
    let (lets, expr) = render_parts(t);
    if lets.is_empty() {
        return expr;
    }
    let mut out = String::from("{\n");
    for l in &lets {
        let _ = writeln!(out, "{pad}    {l}");
    }
    let _ = write!(out, "{pad}    {expr}\n{pad}}}");
    out
}

/// The `let` lines that format the typed arguments, and the expression.
fn render_parts(t: &Text) -> (Vec<String>, String) {
    let mut fmt = String::new();
    let mut lets = Vec::new();
    let mut any_hole = false;
    for p in &t.pieces {
        match p {
            Piece::Text(s) => fmt.push_str(&s.replace('{', "{{").replace('}', "}}")),
            Piece::Hole(name, kind) => {
                any_hole = true;
                let _ = write!(fmt, "{{{name}}}");
                let conv = match kind {
                    Kind::Display => None,
                    Kind::Number => Some(format!("f.number({name})")),
                    Kind::Count => Some(format!("f.count({name})")),
                    Kind::Date(len) => Some(format!(
                        "f.date({name}, ::brocanth_i18n::DateLength::{len})"
                    )),
                };
                if let Some(conv) = conv {
                    let line = format!("let {name} = {conv};");
                    if !lets.contains(&line) {
                        lets.push(line);
                    }
                }
            }
        }
    }
    if !any_hole {
        let literal: String = t
            .pieces
            .iter()
            .map(|p| match p {
                Piece::Text(s) => s.as_str(),
                Piece::Hole(..) => "",
            })
            .collect();
        return (lets, format!("::std::string::String::from({literal:?})"));
    }
    (lets, format!("::std::format!({fmt:?})"))
}

/// The catalog text back, for a doc comment.
fn source_of(t: &Text) -> String {
    let mut s = String::new();
    for p in &t.pieces {
        match p {
            Piece::Text(x) => s.push_str(&x.replace('{', "{{").replace('}', "}}")),
            Piece::Hole(name, Kind::Display | Kind::Count) => {
                let _ = write!(s, "{{{name}}}");
            }
            Piece::Hole(name, Kind::Number) => {
                let _ = write!(s, "{{{name}:number}}");
            }
            Piece::Hole(name, Kind::Date(len)) => {
                let suffix = match *len {
                    "Short" => "date_short",
                    "Long" => "date_long",
                    _ => "date",
                };
                let _ = write!(s, "{{{name}:{suffix}}}");
            }
        }
    }
    s
}

// ---------------------------------------------------------------------------
// Small things
// ---------------------------------------------------------------------------

fn indent(depth: usize) -> String {
    "    ".repeat(depth)
}

/// `cart_items` → `CartItems`, `fr-CA` → `FrCa`.
fn camel(s: &str) -> String {
    s.split(['_', '-'])
        .filter(|p| !p.is_empty())
        .map(|p| {
            let lower = p.to_ascii_lowercase();
            let mut c = lower.chars();
            match c.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + c.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// `en`, `fr-CA`, `zh-Hant-TW`: a language of two or three letters, then
/// subtags of two to eight letters or digits. The full grammar is RFC 5646;
/// this is the part a file name needs, and ICU4X parses the tag again at
/// run time.
fn is_tag(tag: &str) -> bool {
    let mut parts = tag.split('-');
    let lang = parts.next().unwrap_or("");
    (2..=3).contains(&lang.len())
        && lang.bytes().all(|b| b.is_ascii_alphabetic())
        && parts.all(|p| (2..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "gen", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut",
    "pub", "ref", "return", "self", "static", "struct", "super", "trait", "true", "try", "type",
    "unsafe", "use", "where", "while", "yield", "abstract", "become", "box", "do", "final",
    "macro", "override", "priv", "typeof", "unsized", "virtual",
];

/// Why `s` cannot be a function, module or argument name, or `None`.
///
/// `f`, `args` and `locale` are names the generated code uses itself.
fn bad_ident(s: &str) -> Option<&'static str> {
    let mut bytes = s.bytes();
    match bytes.next() {
        None => return Some("is empty"),
        Some(b) if !(b.is_ascii_lowercase() || b == b'_') => {
            return Some("must start with a lower-case letter or `_`")
        }
        _ => {}
    }
    if !bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') {
        return Some("may hold only lower-case ASCII letters, digits and `_`");
    }
    if s == "_" {
        return Some("is `_`, which is not a name");
    }
    if KEYWORDS.contains(&s) {
        return Some("is a Rust keyword");
    }
    None
}

/// The names the generated function uses for itself, which a placeholder
/// would shadow.
const RESERVED: [&str; 3] = ["f", "args", "locale"];

fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, col)
}
