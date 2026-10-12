//! Finding an application, and reading the few facts every command needs
//! about it.
//!
//! # Why this reads the source rather than running it
//!
//! A brocanth application's route table, its plugin list and its middleware
//! mounts are all written in Rust, so the only way to *run* the table is to
//! compile and run the binary. `routes` and `check` have to answer before the
//! compiler does -- that is the whole value of them -- so they read the source
//! instead, through the framework's own path convention
//! ([`brocanth::routing::route_for_file`]) and a deliberately small scan for
//! the three calls that matter: `.plugin(..)`, `.mount(..)` and `.offer(..)`.
//!
//! The scan is literal, and that is a statement rather than an apology. It sees
//! a `mount("/admin/{*rest}", "session")` written in the open and does not see
//! one assembled from a variable at run time. §8 says every plugin must be
//! named in `main` and every mount is the application's own decision, so the
//! code this scan cannot read is code the plan already rules out -- and when
//! the typed-route-table macro lands, this file is the one thing it replaces.

use std::path::{Path, PathBuf};

use brocanth::routing::{route_for_file, RouteTemplate, Verb};

use crate::ui::{Diagnostic, Snippet};

/// The directory a brocanth application keeps its pages in, relative to the
/// package root. One file is one route.
pub const PAGES: &str = "src/pages";

/// A located application.
pub struct Project {
    /// The directory holding `Cargo.toml`.
    pub root: PathBuf,
    pub manifest_path: PathBuf,
    /// The package name, from `Cargo.toml`.
    pub package: String,
    /// Where pages live, relative to [`Project::root`], with `/` separators.
    pub pages_rel: String,
}

/// One route, read off the filesystem.
pub struct Route {
    pub template: RouteTemplate,
    /// Path relative to the pages directory, e.g. `users/[id].rs`.
    pub file: String,
    /// Every verb the file declares a handler for, in canonical order.
    pub verbs: Vec<Verb>,
    /// Functions named `handle*` that are not verbs -- `handle_json` is the
    /// content-negotiation hook, not a method.
    pub hooks: Vec<String>,
}

impl Route {
    /// A concrete path that this template matches, for asking a
    /// [`brocanth::routing::ScopePattern`] whether it covers the route.
    ///
    /// A pattern decides by shape, never by content, so any filler does: a
    /// parameter becomes one segment and a catch-all becomes one more.
    pub fn sample_path(&self) -> String {
        let mut out = String::new();
        for part in self.template.pattern().split('/').skip(1) {
            out.push('/');
            if part.starts_with('{') {
                out.push('_');
            } else {
                out.push_str(part);
            }
        }
        if out.is_empty() {
            out.push('/');
        }
        out
    }
}

/// One argument of a call the scan found, placed in its file.
#[derive(Clone, Debug)]
pub struct Arg {
    pub value: String,
    pub line: usize,
    /// 1-based character column of the argument as written, quotes included.
    pub column: usize,
    /// The whole source line, for the snippet.
    pub text: String,
    /// How many characters the argument occupies, quotes included.
    pub width: usize,
}

impl Arg {
    fn snippet(&self) -> Snippet {
        Snippet {
            line: self.line,
            column: self.column,
            text: self.text.clone(),
            span: self.width,
        }
    }
}

/// A call the scan found.
///
/// Each argument carries its own position, because a call written over four
/// lines -- which `rustfmt` does the moment the arguments are long -- must
/// still point a caret at the argument rather than at the call.
pub struct Site {
    /// The first argument: a plugin expression, a pattern, or a name.
    pub first: Arg,
    /// The second argument, when the call has one.
    pub second: Option<Arg>,
    pub file: String,
}

impl Site {
    /// The name the call is about: the second argument when there is one --
    /// a mount's middleware name -- and the first otherwise.
    pub fn name(&self) -> &str {
        match &self.second {
            Some(arg) => &arg.value,
            None => &self.first.value,
        }
    }

    /// `src/main.rs:24:9`, the form every Rust tool prints.
    pub fn location(&self) -> String {
        self.at(&self.first)
    }

    /// Where the second argument is, which for a mount is the name rather than
    /// the pattern it was mounted at.
    pub fn location_of_second(&self) -> String {
        self.at(self.second.as_ref().unwrap_or(&self.first))
    }

    fn at(&self, arg: &Arg) -> String {
        format!("{}:{}:{}", self.file, arg.line, arg.column)
    }

    pub fn snippet(&self) -> Snippet {
        self.first.snippet()
    }

    pub fn snippet_on_second(&self) -> Snippet {
        self.second.as_ref().unwrap_or(&self.first).snippet()
    }
}

impl Project {
    /// Walks up from `start` looking for a Cargo package whose pages directory
    /// exists, the way cargo looks for `Cargo.toml`. A command run three
    /// directories into `src/pages/` should still work.
    pub fn discover(start: Option<&Path>, pages: Option<&str>) -> Result<Self, Vec<Diagnostic>> {
        let pages_rel = pages.unwrap_or(PAGES).replace('\\', "/");
        let start = match start {
            Some(dir) => dir.to_path_buf(),
            None => std::env::current_dir().map_err(|e| {
                vec![Diagnostic::error(
                    "cwd",
                    format!("the current directory is unreadable: {e}"),
                )]
            })?,
        };
        let mut dir = start.canonicalize().unwrap_or_else(|_| start.clone());
        let mut manifest_without_pages = None;
        loop {
            let manifest = dir.join("Cargo.toml");
            if manifest.is_file() {
                if dir.join(&pages_rel).is_dir() {
                    return Self::open(&dir, &pages_rel);
                }
                manifest_without_pages.get_or_insert(manifest);
            }
            if !dir.pop() {
                break;
            }
        }
        Err(vec![match manifest_without_pages {
            Some(manifest) => Diagnostic::error(
                "no-pages",
                format!("`{pages_rel}` is not a directory, so this package declares no routes"),
            )
            .at(manifest.display().to_string())
            .note("in brocanth the filesystem is the route table: one file is one route")
            .help("run `brocanth create <NAME>` to get the layout, or pass `--pages <DIR>`"),
            None => Diagnostic::error(
                "no-package",
                format!("no Cargo.toml in {} or any parent", start.display()),
            )
            .help("run `brocanth create <NAME>` to make one, or pass `--dir <DIR>`"),
        }])
    }

    fn open(root: &Path, pages_rel: &str) -> Result<Self, Vec<Diagnostic>> {
        let manifest_path = root.join("Cargo.toml");
        let manifest = std::fs::read_to_string(&manifest_path).map_err(|e| {
            vec![Diagnostic::error("manifest", e.to_string())
                .at(manifest_path.display().to_string())]
        })?;
        Ok(Self {
            root: root.to_path_buf(),
            package: package_name(&manifest).unwrap_or_else(|| "?".to_string()),
            manifest_path,
            pages_rel: pages_rel.to_string(),
        })
    }

    /// A path to show a human: relative to the package root when it is inside
    /// it, absolute when it is not.
    pub fn display(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .display()
            .to_string()
            .replace('\\', "/")
    }

    pub fn pages_dir(&self) -> PathBuf {
        self.root.join(&self.pages_rel)
    }

    /// Every route the pages directory declares, ordered by template so two
    /// runs print the same table.
    ///
    /// A file that does not name a route is a finding rather than a silent
    /// skip, so it comes back as a diagnostic beside the routes that parsed.
    pub fn routes(&self) -> (Vec<Route>, Vec<Diagnostic>) {
        let dir = self.pages_dir();
        let mut routes = Vec::new();
        let mut problems = Vec::new();
        for path in rust_files(&dir) {
            let rel = path
                .strip_prefix(&dir)
                .expect("under the pages directory")
                .to_string_lossy()
                .replace('\\', "/");
            // `mod.rs` is the module wiring the pages need to be reachable from
            // `main`. It is not a page, and a route called `/mod` would be a
            // surprise nobody asked for.
            if rel == "mod.rs" || rel.ends_with("/mod.rs") {
                continue;
            }
            let template = match route_for_file(&rel, "") {
                Ok(template) => template,
                Err(e) => {
                    problems.push(
                        Diagnostic::error("route/path", e.to_string())
                            .at(format!("{}/{rel}", self.pages_rel))
                            .help(
                                "the file convention is `[name]` for one segment and \
                                 `[...name]` for the rest",
                            ),
                    );
                    continue;
                }
            };
            let source = std::fs::read_to_string(&path).unwrap_or_default();
            let (verbs, hooks) = handlers_in(&source);
            routes.push(Route {
                template,
                file: rel,
                verbs,
                hooks,
            });
        }
        routes.sort_by(|a, b| a.template.pattern().cmp(b.template.pattern()));
        (routes, problems)
    }

    /// Where the application mounts middleware: `router().mount(pattern, name)`.
    ///
    /// Only the application mounts -- a plugin cannot, because a
    /// `MiddlewareCatalog` has no `mount` -- so both arguments are written out
    /// at the call, which is what makes them readable from here.
    pub fn mounts(&self) -> Vec<Site> {
        self.scan(".mount(", Shape::Literals)
    }

    /// What the plugins offer: `catalog.offer(name, description)`.
    pub fn offers(&self) -> Vec<Site> {
        self.scan(".offer(", Shape::Literals)
    }

    /// Which plugins `main` names.
    ///
    /// Four spellings, because there are four: `.plugin(..)` and
    /// `.plugin_as(..)` in the core, and `.plugin_from_config::<P>("name")` and
    /// `.plugin_as_from_config::<P>("name")` from the configuration crate's
    /// extension trait.
    pub fn registrations(&self) -> Vec<Site> {
        let mut sites = self.scan(".plugin(", Shape::Expression);
        sites.extend(self.scan(".plugin_as(", Shape::Expression));
        // `with_config` registers `ConfigPlugin`, so configuration is a plugin
        // here in the same sense as everywhere else -- and the count must say
        // so, or the tool and the application disagree about what is installed.
        sites.extend(self.scan(".with_config(", Shape::Expression));
        sites.extend(self.scan(".plugin_from_config::<", Shape::Turbofish));
        sites.extend(self.scan(".plugin_as_from_config::<", Shape::Turbofish));
        sites.sort_by(|a, b| (&a.file, a.first.line).cmp(&(&b.file, b.first.line)));
        sites
    }

    /// Finds every `call` in the application's own sources, pages excluded --
    /// a page declares a route, and none of the three calls belongs in one.
    fn scan(&self, call: &str, shape: Shape) -> Vec<Site> {
        let pages = self.pages_dir();
        let mut out = Vec::new();
        for path in rust_files(&self.root.join("src")) {
            if path.starts_with(&pages) {
                continue;
            }
            let Ok(source) = std::fs::read_to_string(&path) else {
                continue;
            };
            let file = self.display(&path);
            let lines = Lines::of(&source);
            let mut from = 0;
            while let Some(found) = source[from..].find(call) {
                let open = from + found + call.len();
                from = open;
                if let Some((first, second)) = read_call(&source, open, shape) {
                    out.push(Site {
                        first: lines.place(&source, first),
                        second: second.map(|a| lines.place(&source, a)),
                        file: file.clone(),
                    });
                }
            }
        }
        out.sort_by(|a, b| (&a.file, a.first.line).cmp(&(&b.file, b.first.line)));
        out
    }
}

// -- reading ----------------------------------------------------------------

/// How a call's arguments are written.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// Every argument is a string literal: `mount("/admin", "session")`.
    Literals,
    /// The first argument is an expression: `plugin(Logging::new(..))`.
    Expression,
    /// `plugin_from_config::<Logging>("logging")`: a type, then a name.
    Turbofish,
}

/// An argument before it has been placed in its file.
struct Raw {
    value: String,
    /// Byte offset of the argument as written, opening quote included.
    byte: usize,
    /// Characters the argument occupies, quotes included.
    width: usize,
}

/// Line starts, so a byte offset becomes a line and a column without rescanning
/// the file for each one.
struct Lines(Vec<usize>);

impl Lines {
    fn of(source: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(source.match_indices('\n').map(|(i, _)| i + 1));
        Self(starts)
    }

    fn place(&self, source: &str, raw: Raw) -> Arg {
        let index = self.0.partition_point(|start| *start <= raw.byte) - 1;
        let start = self.0[index];
        let end = source[start..]
            .find('\n')
            .map(|i| start + i)
            .unwrap_or(source.len());
        Arg {
            // A column is a character count, not a byte count: a comment in
            // Japanese before the call must not push the caret right.
            column: source[start..raw.byte].chars().count() + 1,
            line: index + 1,
            text: source[start..end].trim_end_matches('\r').to_string(),
            width: raw.width,
            value: raw.value,
        }
    }
}

/// Reads the arguments of a call whose `(` or `<` has already been passed.
///
/// Returns [`None`] when the call never closes, which only happens in a file
/// that would not compile either.
fn read_call(source: &str, open: usize, shape: Shape) -> Option<(Raw, Option<Raw>)> {
    if shape == Shape::Turbofish {
        let close = source[open..].find(">(")?;
        let ty = source[open..open + close].trim();
        let name = literals(source, open + close + 2)?.into_iter().next();
        return Some((
            Raw {
                value: ty.to_string(),
                byte: open,
                width: ty.chars().count().max(1),
            },
            name,
        ));
    }

    let found = literals(source, open)?;
    if shape == Shape::Expression {
        // The expression runs to the first top-level comma, or to the close.
        let end = top_level_comma(source, open).unwrap_or_else(|| close_paren(source, open));
        let value = source[open..end]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        // `.plugin_as("db_log", Logging::new(..))` puts the name first, so the
        // first literal inside the call is the name either way.
        let name = found.into_iter().next();
        return Some((
            Raw {
                width: value.chars().count().max(1),
                value,
                byte: open,
            },
            name,
        ));
    }

    let mut found = found.into_iter();
    let first = found.next()?;
    Some((first, found.next()))
}

/// Every string literal inside the call starting after `open`, in order.
///
/// A `(` or `)` inside a literal does not change the depth, and a `\"` does not
/// end one -- which is why this is a scanner rather than a `find`.
fn literals(source: &str, open: usize) -> Option<Vec<Raw>> {
    let mut out = Vec::new();
    let mut depth = 1usize;
    let mut chars = source[open..].char_indices();
    while let Some((offset, c)) = chars.next() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(out);
                }
            }
            '"' => {
                let mut value = String::new();
                // The opening quote, plus one per character read -- which
                // includes the closing quote, so the caret covers the literal
                // exactly as it was written.
                let mut width = 1usize;
                let mut closed = false;
                let mut escaped = false;
                for (_, c) in chars.by_ref() {
                    width += 1;
                    if escaped {
                        value.push(c);
                        escaped = false;
                        continue;
                    }
                    match c {
                        '\\' => escaped = true,
                        '"' => {
                            closed = true;
                            break;
                        }
                        _ => value.push(c),
                    }
                }
                if !closed {
                    return None;
                }
                out.push(Raw {
                    value,
                    byte: open + offset,
                    width,
                });
            }
            _ => {}
        }
    }
    None
}

fn close_paren(source: &str, open: usize) -> usize {
    let mut depth = 1usize;
    for (offset, c) in source[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return open + offset;
                }
            }
            _ => {}
        }
    }
    source.len()
}

fn top_level_comma(source: &str, open: usize) -> Option<usize> {
    let mut depth = 1usize;
    for (offset, c) in source[open..].char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return None;
                }
            }
            ']' => depth = depth.saturating_sub(1),
            ',' if depth == 1 => return Some(open + offset),
            _ => {}
        }
    }
    None
}

/// `name = "blog"` out of a `[package]` table.
///
/// Hand-rolled rather than through a TOML parser the CLI would otherwise not
/// need: one key is wanted, and the value is only ever shown to a human.
fn package_name(manifest: &str) -> Option<String> {
    let mut in_package = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if in_package {
            if let Some(value) = line.strip_prefix("name") {
                let value = value.trim_start().strip_prefix('=')?.trim();
                return Some(value.trim_matches(['"', '\'']).to_string());
            }
        }
    }
    None
}

/// Every `.rs` file under `dir`, sorted, so output does not depend on the order
/// the filesystem hands directory entries back.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// The verbs and the content-negotiation hooks a page file declares.
///
/// `Verb::from_handler_fn` is the framework's own mapping, so `handle` is GET
/// and `handle_json` is not a verb at all -- this function does not get a vote.
fn handlers_in(source: &str) -> (Vec<Verb>, Vec<String>) {
    let mut verbs = Vec::new();
    let mut hooks = Vec::new();
    for name in fn_names(source) {
        match Verb::from_handler_fn(&name) {
            Some(verb) if !verbs.contains(&verb) => verbs.push(verb),
            Some(_) => {}
            None if name.starts_with("handle") && !hooks.contains(&name) => hooks.push(name),
            None => {}
        }
    }
    verbs.sort_by_key(|v| v.index());
    (verbs, hooks)
}

fn fn_names(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in source.lines() {
        let line = line.trim_start();
        // A doc comment mentioning `fn handle` is prose, not a declaration.
        if line.starts_with("//") {
            continue;
        }
        let Some(rest) = line
            .strip_prefix("pub fn ")
            .or_else(|| line.strip_prefix("fn "))
            .or_else(|| line.strip_prefix("pub async fn "))
            .or_else(|| line.strip_prefix("async fn "))
        else {
            continue;
        };
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            out.push(name);
        }
    }
    out
}

/// The closest of `candidates` to `name`, when one is close enough to be worth
/// suggesting.
///
/// The framework's own suggester, not a second one: `brocanth check` must
/// propose the same correction the application's own build would, or a reader
/// gets two different answers to one typo.
pub fn nearest(name: &str, candidates: &[&str]) -> Option<String> {
    brocanth::suggest::nearest_name(name, candidates)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One call in one file, as the scan would read it.
    fn call(source: &str, marker: &str, shape: Shape) -> Site {
        let open = source.find(marker).expect("the marker is there") + marker.len();
        let (first, second) = read_call(source, open, shape).expect("the call closes");
        let lines = Lines::of(source);
        Site {
            first: lines.place(source, first),
            second: second.map(|a| lines.place(source, a)),
            file: "src/main.rs".into(),
        }
    }

    #[test]
    fn the_package_name_comes_from_the_package_table() {
        let manifest =
            "[package]\nname = \"blog\"\nversion = \"0.1.0\"\n\n[dependencies]\nname = \"not this\"\n";
        assert_eq!(package_name(manifest).as_deref(), Some("blog"));
        assert_eq!(package_name("[dependencies]\nname = \"x\"\n"), None);
    }

    #[test]
    fn the_framework_decides_which_function_names_are_verbs() {
        let source = "pub fn handle(c: &mut Ctx) -> Page {}\npub fn handle_post() {}\npub fn handle_json() {}\nfn helper() {}\n";
        let (verbs, hooks) = handlers_in(source);
        assert_eq!(verbs, [Verb::Get, Verb::Post]);
        assert_eq!(hooks, ["handle_json"], "a hook is not a method");
    }

    #[test]
    fn a_commented_out_handler_is_not_a_handler() {
        let (verbs, _) = handlers_in("// pub fn handle() {}\n//! calls `fn handle_post`\n");
        assert!(verbs.is_empty(), "{verbs:?}");
    }

    #[test]
    fn a_mount_is_read_with_both_arguments_and_a_caret_on_each() {
        let source =
            "fn setup() {\n        s.router().mount(\"/admin/{*rest}\", \"session\");\n}\n";
        let site = call(source, ".mount(", Shape::Literals);
        assert_eq!(site.first.value, "/admin/{*rest}");
        assert_eq!(site.name(), "session");
        assert_eq!(site.location(), "src/main.rs:2:26");
        assert_eq!(site.first.width, "\"/admin/{*rest}\"".len());
        assert_eq!(site.location_of_second(), "src/main.rs:2:44");
        assert_eq!(site.snippet_on_second().span, "\"session\"".len());
    }

    #[test]
    fn a_call_rustfmt_split_over_four_lines_is_still_read() {
        // The shape `rustfmt` produces the moment the arguments are long, which
        // is most real `offer` calls. A line scan would miss every one of them.
        let source =
            "fn f() {\n    catalog.offer(\n        \"session\",\n        \"reads the cookie\",\n    );\n}\n";
        let site = call(source, ".offer(", Shape::Literals);
        assert_eq!(site.first.value, "session");
        assert_eq!(site.second.as_ref().unwrap().value, "reads the cookie");
        assert_eq!(
            site.location(),
            "src/main.rs:3:9",
            "the argument, not the call"
        );
        assert_eq!(site.first.text.trim(), "\"session\",");
    }

    #[test]
    fn a_parenthesis_inside_a_literal_does_not_end_the_call() {
        let source = "catalog.offer(\"session\", \"a cookie (signed)\");\n";
        let site = call(source, ".offer(", Shape::Literals);
        assert_eq!(site.second.as_ref().unwrap().value, "a cookie (signed)");
    }

    #[test]
    fn an_escaped_quote_does_not_end_a_literal() {
        let source = "catalog.offer(\"session\", \"the \\\"session\\\" cookie\");\n";
        let site = call(source, ".offer(", Shape::Literals);
        assert_eq!(
            site.second.as_ref().unwrap().value,
            "the \"session\" cookie"
        );
    }

    #[test]
    fn a_plugin_registration_is_read_as_an_expression() {
        let source = "    .plugin(plugins::logging::Logging::new(Settings::default()))\n";
        let site = call(source, ".plugin(", Shape::Expression);
        assert_eq!(
            site.first.value,
            "plugins::logging::Logging::new(Settings::default())"
        );
    }

    #[test]
    fn a_named_registration_keeps_the_name_it_was_given() {
        let source = "    .plugin_as(\"db_log\", Logging::new(s))\n";
        let site = call(source, ".plugin_as(", Shape::Expression);
        assert_eq!(site.name(), "db_log");
    }

    #[test]
    fn a_registration_through_the_config_crate_reads_its_type_and_section() {
        let source = "    .plugin_from_config::<plugins::logging::Logging>(\"logging\")\n";
        let site = call(source, ".plugin_from_config::<", Shape::Turbofish);
        assert_eq!(site.first.value, "plugins::logging::Logging");
        assert_eq!(site.name(), "logging");
    }

    #[test]
    fn a_call_that_never_closes_is_not_half_read() {
        let source = "s.router().mount(\"/a\"";
        let open = source.find(".mount(").unwrap() + ".mount(".len();
        assert!(read_call(source, open, Shape::Literals).is_none());
    }

    #[test]
    fn a_column_is_counted_in_characters_not_bytes() {
        // A wide comment before the call must not move the caret off the
        // argument it is pointing at.
        let source = "// \u{65e5}\u{672c}\n    s.mount(\"/a\", \"b\");\n";
        let site = call(source, ".mount(", Shape::Literals);
        assert_eq!(site.first.line, 2);
        assert_eq!(site.first.column, 13);
    }

    #[test]
    fn a_sample_path_has_the_shape_of_the_template_and_no_parameters() {
        let route = |pattern: &str| Route {
            template: RouteTemplate::parse(pattern).unwrap(),
            file: String::new(),
            verbs: Vec::new(),
            hooks: Vec::new(),
        };
        assert_eq!(route("/").sample_path(), "/");
        assert_eq!(route("/users/{id}").sample_path(), "/users/_");
        assert_eq!(route("/docs/{*rest}").sample_path(), "/docs/_");
    }

    #[test]
    fn a_suggestion_is_the_frameworks_own() {
        assert_eq!(
            nearest("sesion", &["session", "request_log"]).as_deref(),
            Some("session")
        );
        assert_eq!(nearest("logging", &["session"]), None);
    }
}
