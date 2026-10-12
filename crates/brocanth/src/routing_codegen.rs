//! Typed route helpers: Rust source a `build.rs` writes.
//!
//! The Crystal prototype generated `Users::Show.path("42")` once per arity
//! from 0 to 3, then fell back to `**params`, because its macros cannot emit
//! a variadic signature. Past three parameters the compiler stopped
//! checking. Here there is no arity ladder: one function per route, taking
//! exactly that route's parameters, so a wrong count or a forgotten
//! parameter is a compile error.
//!
//! ```text
//! error[E0061]: this function takes 2 arguments but 1 argument was supplied
//!   --> src/main.rs:4:13
//!    |
//!  4 |     let p = routes::users::posts::path("42");
//!    |             ^^^^^^^^^^^^^^^^^^^^^^^^^^ ----  argument #2 is missing
//! ```
//!
//! Rails answers the same mistake at runtime, from inside `users_path`.
//!
//! # Using it from `build.rs`
//!
//! ```no_run
//! use brocanth::routing::{generate_helpers, RouteHelper, Verb};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let routes = vec![
//!     RouteHelper::for_file("src/pages/index.rs", "src/pages")?.with_verbs([Verb::Get]),
//!     RouteHelper::for_file("src/pages/users/[id].rs", "src/pages")?
//!         .with_verbs([Verb::Get, Verb::Post]),
//! ];
//! let source = generate_helpers(&routes)?;
//! std::fs::write(
//!     std::path::Path::new(&std::env::var("OUT_DIR")?).join("routes.rs"),
//!     source,
//! )?;
//! # Ok(())
//! # }
//! ```
//!
//! Then `include!(concat!(env!("OUT_DIR"), "/routes.rs"));` inside a module
//! of your choice. The generated file is self-contained: it needs no
//! dependency, brocanth included, so the crate that builds it cannot create
//! a cycle.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::routing_path::{
    is_rust_keyword, sanitize_ident, ParamKind, ParamSpec, PathError, RouteTemplate,
};
use crate::routing_verb::Verb;

/// Names the generated code owns inside a module. A route whose directory
/// is called one of these would collide with its own helper.
const RESERVED: &[&str] = &["path", "url", "TEMPLATE", "VERBS"];

/// The module the encoder helpers go in. A top-level route directory of
/// this name would collide with it.
const SUPPORT_MODULE: &str = "__brocanth_url";

/// One route, and where its helper should live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteHelper {
    module_path: Vec<String>,
    template: RouteTemplate,
    verbs: Vec<Verb>,
}

impl RouteHelper {
    /// Place `template` at `module_path`.
    ///
    /// An empty module path puts the helper in a module called `root`.
    #[must_use]
    pub fn new(module_path: Vec<String>, template: RouteTemplate) -> Self {
        Self {
            module_path,
            template,
            verbs: Vec::new(),
        }
    }

    /// Derive both the module path and the template from a page file.
    ///
    /// # Errors
    ///
    /// Returns [`PathError`] when the file does not name a route.
    pub fn for_file(file: &str, base: &str) -> Result<Self, PathError> {
        Ok(Self::new(
            crate::routing_path::module_path_for_file(file, base)?,
            crate::routing_path::route_for_file(file, base)?,
        ))
    }

    /// Record which verbs the page answers. Emitted as a `VERBS` constant.
    #[must_use]
    pub fn with_verbs(mut self, verbs: impl IntoIterator<Item = Verb>) -> Self {
        self.verbs = verbs.into_iter().collect();
        self.verbs.sort_unstable_by_key(|v| v.index());
        self.verbs.dedup();
        self
    }

    /// Where the helper lives, as module names.
    #[must_use]
    pub fn module_path(&self) -> &[String] {
        &self.module_path
    }

    /// The route this helper builds.
    #[must_use]
    pub const fn template(&self) -> &RouteTemplate {
        &self.template
    }

    /// The verbs recorded by [`RouteHelper::with_verbs`].
    #[must_use]
    pub fn verbs(&self) -> &[Verb] {
        &self.verbs
    }
}

/// The generated source could not be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CodegenError {
    /// Two routes want the same module path.
    #[error("two routes both map to the module `{module}`: `{first}` and `{second}`")]
    DuplicateModule {
        /// The module path, joined with `::`.
        module: String,
        /// The template that claimed it first.
        first: String,
        /// The template that collided with it.
        second: String,
    },

    /// A directory name collides with something the generator emits.
    #[error("the module `{module}` collides with the generated item `{item}`")]
    ReservedName {
        /// The module path, joined with `::`.
        module: String,
        /// The generated name it collides with.
        item: String,
    },
}

#[derive(Default)]
struct Node<'a> {
    children: BTreeMap<String, Node<'a>>,
    leaf: Option<&'a RouteHelper>,
}

/// Build the Rust source for a set of route helpers.
///
/// The output is deterministic: modules are ordered by name, so a rerun with
/// the same input produces a byte-identical file and `build.rs` does not
/// churn the build cache.
///
/// # Errors
///
/// Returns [`CodegenError`] when two routes claim the same module, or when a
/// module name collides with a generated item.
pub fn generate_helpers(routes: &[RouteHelper]) -> Result<String, CodegenError> {
    let mut root = Node::default();

    for route in routes {
        let path: Vec<String> = if route.module_path.is_empty() {
            vec!["root".to_owned()]
        } else {
            route.module_path.clone()
        };

        let mut node = &mut root;
        for segment in &path {
            node = node.children.entry(segment.clone()).or_default();
        }
        if let Some(existing) = node.leaf {
            return Err(CodegenError::DuplicateModule {
                module: path.join("::"),
                first: existing.template.pattern().to_owned(),
                second: route.template.pattern().to_owned(),
            });
        }
        node.leaf = Some(route);
    }

    check_collisions(&root, &mut Vec::new())?;

    let mut out = String::with_capacity(1024);
    out.push_str(HEADER);
    out.push_str(SUPPORT);
    for (name, child) in &root.children {
        emit_module(&mut out, name, child, 0, &mut String::new());
    }
    Ok(out)
}

fn check_collisions(node: &Node<'_>, stack: &mut Vec<String>) -> Result<(), CodegenError> {
    if stack.is_empty() && node.children.contains_key(SUPPORT_MODULE) {
        return Err(CodegenError::ReservedName {
            module: SUPPORT_MODULE.to_owned(),
            item: SUPPORT_MODULE.to_owned(),
        });
    }
    if node.leaf.is_some() {
        for reserved in RESERVED {
            if node.children.contains_key(*reserved) {
                return Err(CodegenError::ReservedName {
                    module: stack.join("::"),
                    item: (*reserved).to_owned(),
                });
            }
        }
    }
    for (name, child) in &node.children {
        stack.push(name.clone());
        check_collisions(child, stack)?;
        stack.pop();
    }
    Ok(())
}

fn emit_module(out: &mut String, name: &str, node: &Node<'_>, depth: usize, pad: &mut String) {
    let indent = pad.clone();
    let _ = writeln!(out, "{indent}pub mod {name} {{");
    pad.push_str("    ");

    if let Some(route) = node.leaf {
        emit_leaf(out, route, depth + 1, pad);
    }
    for (child_name, child) in &node.children {
        emit_module(out, child_name, child, depth + 1, pad);
    }

    pad.truncate(pad.len() - 4);
    let _ = writeln!(out, "{indent}}}");
}

fn emit_leaf(out: &mut String, route: &RouteHelper, depth: usize, pad: &str) {
    let template = route.template.pattern();
    let params = route.template.params();
    // `depth` modules up from here is the file root, where the support
    // module sits.
    let support: String = "super::".repeat(depth) + SUPPORT_MODULE;

    let _ = writeln!(out, "{pad}/// Route template: `{template}`.");
    let _ = writeln!(
        out,
        "{pad}pub const TEMPLATE: &str = \"{}\";",
        escape(template)
    );
    if !route.verbs.is_empty() {
        let verbs: Vec<String> = route
            .verbs
            .iter()
            .map(|v| format!("\"{}\"", v.as_str()))
            .collect();
        let _ = writeln!(out, "{pad}/// Verbs this page answers.");
        let _ = writeln!(
            out,
            "{pad}pub const VERBS: &[&str] = &[{}];",
            verbs.join(", ")
        );
    }

    let args = signature(params);
    let _ = writeln!(out, "{pad}/// Build the path for `{template}`.");
    let _ = writeln!(out, "{pad}#[must_use]");
    let _ = writeln!(
        out,
        "{pad}pub fn path({args}) -> ::std::string::String {{"
    );
    if params.is_empty() {
        let _ = writeln!(
            out,
            "{pad}    ::std::string::String::from(\"{}\")",
            escape(template)
        );
    } else {
        let _ = writeln!(
            out,
            "{pad}    let mut out = ::std::string::String::new();"
        );
        for piece in pieces(template, params) {
            match piece {
                Piece::Literal(text) => {
                    let _ = writeln!(out, "{pad}    out.push_str(\"{}\");", escape(text));
                }
                Piece::Param(spec) => {
                    let func = match spec.kind() {
                        ParamKind::Dynamic => "segment",
                        ParamKind::CatchAll => "catch_all",
                    };
                    let ident = arg_ident(spec.name());
                    let _ = writeln!(
                        out,
                        "{pad}    out.push_str(&{support}::{func}(&{ident}.to_string()));"
                    );
                }
            }
        }
        let _ = writeln!(out, "{pad}    out");
    }
    let _ = writeln!(out, "{pad}}}");

    let url_args = if params.is_empty() {
        "base: impl ::core::fmt::Display".to_owned()
    } else {
        format!("base: impl ::core::fmt::Display, {args}")
    };
    let forward: Vec<String> = params.iter().map(|p| arg_ident(p.name())).collect();
    let _ = writeln!(
        out,
        "{pad}/// Build an absolute URL for `{template}` under `base`."
    );
    let _ = writeln!(out, "{pad}#[must_use]");
    let _ = writeln!(
        out,
        "{pad}pub fn url({url_args}) -> ::std::string::String {{"
    );
    let _ = writeln!(out, "{pad}    let mut out = base.to_string();");
    let _ = writeln!(
        out,
        "{pad}    while out.ends_with('/') {{ out.pop(); }}"
    );
    let _ = writeln!(
        out,
        "{pad}    out.push_str(&path({}));",
        forward.join(", ")
    );
    let _ = writeln!(out, "{pad}    out");
    let _ = writeln!(out, "{pad}}}");
}

fn signature(params: &[ParamSpec]) -> String {
    params
        .iter()
        .map(|p| format!("{}: impl ::core::fmt::Display", arg_ident(p.name())))
        .collect::<Vec<_>>()
        .join(", ")
}

fn arg_ident(name: &str) -> String {
    let cleaned = sanitize_ident(name);
    if is_rust_keyword(&cleaned) {
        format!("r#{cleaned}")
    } else {
        cleaned
    }
}

enum Piece<'a> {
    Literal(&'a str),
    Param(&'a ParamSpec),
}

/// Split a validated template into literal runs and parameters.
fn pieces<'a>(template: &'a str, params: &'a [ParamSpec]) -> Vec<Piece<'a>> {
    let mut out = Vec::new();
    let mut rest = template;
    let mut next = 0;
    while let Some(open) = rest.find('{') {
        if open > 0 {
            out.push(Piece::Literal(&rest[..open]));
        }
        let after = &rest[open + 1..];
        let close = after
            .find('}')
            .expect("the template was validated, so every brace closes");
        out.push(Piece::Param(
            params
                .get(next)
                .expect("the template was validated, so parameters are in order"),
        ));
        next += 1;
        rest = &after[close + 1..];
    }
    if !rest.is_empty() {
        out.push(Piece::Literal(rest));
    }
    out
}

fn escape(raw: &str) -> String {
    raw.chars()
        .flat_map(|c| match c {
            '"' => vec!['\\', '"'],
            '\\' => vec!['\\', '\\'],
            other => vec![other],
        })
        .collect()
}

const HEADER: &str = "\
// @generated by brocanth::routing::generate_helpers. Do not edit.
//
// One function per route, taking exactly that route's parameters. A wrong
// argument count is a compile error here, not a 500 at request time.
#![allow(dead_code, unused_imports)]
";

const SUPPORT: &str = r##"
#[doc(hidden)]
pub mod __brocanth_url {
    //! Percent-encoding, inlined so the generated file needs no dependency.

    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    fn encode(raw: &str, keep_slash: bool) -> ::std::string::String {
        let mut out = ::std::string::String::with_capacity(raw.len());
        for byte in raw.bytes() {
            let unreserved =
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~');
            if unreserved || (keep_slash && byte == b'/') {
                out.push(byte as char);
            } else {
                out.push('%');
                out.push(HEX[usize::from(byte >> 4)] as char);
                out.push(HEX[usize::from(byte & 0x0f)] as char);
            }
        }
        out
    }

    /// Encode one path segment. `/` is encoded, so the value cannot escape
    /// the segment the route gave it.
    #[must_use]
    pub fn segment(raw: &str) -> ::std::string::String {
        encode(raw, false)
    }

    /// Encode a catch-all value, keeping `/` as a separator.
    #[must_use]
    pub fn catch_all(raw: &str) -> ::std::string::String {
        encode(raw, true)
    }
}
"##;
