//! Route templates, and the pure conversion from a file path to one.
//!
//! This module is the contract between the filesystem and the router. It has
//! no state and does no I/O: give it a string, it gives you a template or an
//! error. Nothing here can silently produce a route the author did not write.
//!
//! # Template syntax
//!
//! A brocanth template is a path whose segments are literal text, a dynamic
//! parameter `{name}`, or a catch-all parameter `{*name}`:
//!
//! ```text
//! /users/{id}/posts/{post_id}
//! /docs/{*rest}
//! ```
//!
//! A parameter name must be a Rust identifier (`[A-Za-z_][A-Za-z0-9_]*`).
//! The generated route helpers turn each name into a function argument, so a
//! name that is not an identifier is rejected at parse time rather than at
//! code-generation time.
//!
//! The characters `<` and `>` are reserved: the matcher engine uses them, and
//! brocanth refuses them in a template so that no literal can be mistaken for
//! a parameter.
//!
//! # File convention
//!
//! [`route_for_file`] applies three rules, in this order:
//!
//! | File (relative to the pages base) | Route |
//! | --------------------------------- | ----- |
//! | `index.rs`                        | `/` |
//! | `about.rs`                        | `/about` |
//! | `users/index.rs`                  | `/users` |
//! | `users/[id].rs`                   | `/users/{id}` |
//! | `docs/[...rest].rs`               | `/docs/{*rest}` |

use core::fmt;

/// What a parameter in a template captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParamKind {
    /// Captures exactly one path segment. Never captures `/`.
    Dynamic,
    /// Captures one or more path segments, `/` included.
    CatchAll,
}

/// One parameter of a template, in the order it appears.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ParamSpec {
    name: String,
    kind: ParamKind,
}

impl ParamSpec {
    /// The parameter name, without braces or star.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether the parameter captures one segment or many.
    #[must_use]
    pub const fn kind(&self) -> ParamKind {
        self.kind
    }

    /// True for `{*name}`, false for `{name}`.
    #[must_use]
    pub const fn is_catch_all(&self) -> bool {
        matches!(self.kind, ParamKind::CatchAll)
    }
}

/// A validated route template, such as `/users/{id}`.
///
/// Construct one with [`RouteTemplate::parse`] or [`route_for_file`]. Both
/// validate, so an existing value is always a template the router accepts.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RouteTemplate {
    pattern: String,
    params: Vec<ParamSpec>,
}

impl RouteTemplate {
    /// Parse and validate a template written in brocanth syntax.
    ///
    /// # Errors
    ///
    /// Returns [`TemplateError`] if the template is empty, has no leading
    /// slash, has an unbalanced brace, uses a reserved character, names a
    /// parameter that is not an identifier, or repeats a parameter name.
    pub fn parse(pattern: &str) -> Result<Self, TemplateError> {
        if pattern.is_empty() {
            return Err(TemplateError::Empty);
        }
        if !pattern.starts_with('/') {
            return Err(TemplateError::MissingLeadingSlash {
                pattern: pattern.to_owned(),
            });
        }

        let mut params: Vec<ParamSpec> = Vec::new();
        let mut rest = pattern;

        while let Some(open) = rest.find('{') {
            let before = &rest[..open];
            reject_reserved(before)?;
            if let Some(stray) = before.find('}') {
                return Err(TemplateError::UnbalancedBrace {
                    pattern: pattern.to_owned(),
                    at: pattern.len() - rest.len() + stray,
                });
            }

            let after_open = &rest[open + 1..];
            let close = after_open
                .find('}')
                .ok_or_else(|| TemplateError::UnbalancedBrace {
                    pattern: pattern.to_owned(),
                    at: pattern.len() - rest.len() + open,
                })?;
            let body = &after_open[..close];
            if body.contains('{') {
                return Err(TemplateError::UnbalancedBrace {
                    pattern: pattern.to_owned(),
                    at: pattern.len() - rest.len() + open,
                });
            }

            let (kind, name) = match body.strip_prefix('*') {
                Some(name) => (ParamKind::CatchAll, name),
                None => (ParamKind::Dynamic, body),
            };
            validate_param_name(name)?;
            if params.iter().any(|p| p.name == name) {
                return Err(TemplateError::DuplicateParam {
                    name: name.to_owned(),
                });
            }
            params.push(ParamSpec {
                name: name.to_owned(),
                kind,
            });

            rest = &after_open[close + 1..];
        }

        reject_reserved(rest)?;
        if let Some(stray) = rest.find('}') {
            return Err(TemplateError::UnbalancedBrace {
                pattern: pattern.to_owned(),
                at: pattern.len() - rest.len() + stray,
            });
        }

        Ok(Self {
            pattern: pattern.to_owned(),
            params,
        })
    }

    /// The template in brocanth syntax, for example `/users/{id}`.
    #[must_use]
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    /// The parameters, in the order they appear in the template.
    ///
    /// There is no ceiling on the count.
    #[must_use]
    pub fn params(&self) -> &[ParamSpec] {
        &self.params
    }

    /// Translate to the matcher engine's syntax (`<name>` and `<*name>`).
    ///
    /// Validation has already proved that every brace is a parameter
    /// delimiter and that no literal `<` or `>` is present, so a character
    /// swap is exact.
    pub(crate) fn to_engine_syntax(&self) -> String {
        self.pattern
            .chars()
            .map(|c| match c {
                '{' => '<',
                '}' => '>',
                other => other,
            })
            .collect()
    }
}

impl fmt::Display for RouteTemplate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.pattern)
    }
}

fn reject_reserved(literal: &str) -> Result<(), TemplateError> {
    for ch in ['<', '>'] {
        if literal.contains(ch) {
            return Err(TemplateError::ReservedCharacter { character: ch });
        }
    }
    Ok(())
}

fn validate_param_name(name: &str) -> Result<(), TemplateError> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(TemplateError::EmptyParamName);
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err(TemplateError::InvalidParamName {
            name: name.to_owned(),
        });
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(TemplateError::InvalidParamName {
            name: name.to_owned(),
        });
    }
    Ok(())
}

/// A template that cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum TemplateError {
    /// The template string is empty.
    #[error("the template is empty")]
    Empty,

    /// The template does not start with `/`.
    #[error("the template `{pattern}` does not start with `/`")]
    MissingLeadingSlash {
        /// The rejected template.
        pattern: String,
    },

    /// A `{` has no `}`, or a `}` has no `{`.
    #[error("the template `{pattern}` has an unbalanced brace at byte {at}")]
    UnbalancedBrace {
        /// The rejected template.
        pattern: String,
        /// Byte offset of the brace that has no partner.
        at: usize,
    },

    /// A parameter is written `{}` or `{*}`.
    #[error("a parameter name is empty")]
    EmptyParamName,

    /// A parameter name is not a Rust identifier.
    #[error("the parameter name `{name}` is not an identifier")]
    InvalidParamName {
        /// The rejected name.
        name: String,
    },

    /// The same parameter name appears twice.
    #[error("the parameter name `{name}` appears more than once")]
    DuplicateParam {
        /// The repeated name.
        name: String,
    },

    /// The template contains `<` or `>`, which the matcher reserves.
    #[error("the character `{character}` is reserved and cannot appear in a template")]
    ReservedCharacter {
        /// The reserved character that was found.
        character: char,
    },
}

/// A file path that does not name a route.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PathError {
    /// The file path is not inside the pages base directory.
    #[error("the path `{path}` is not inside the base directory `{base}`")]
    NotUnderBase {
        /// The rejected path.
        path: String,
        /// The base it was compared against.
        base: String,
    },

    /// Nothing is left after the base directory is removed.
    #[error("the path `{path}` names no file below the base directory")]
    EmptyPath {
        /// The rejected path.
        path: String,
    },

    /// A segment is empty, `.`, or `..`.
    #[error("the path `{path}` has the invalid segment `{segment}`")]
    InvalidSegment {
        /// The rejected path.
        path: String,
        /// The segment that is not usable.
        segment: String,
    },

    /// A segment uses `[` or `]` but is not exactly `[name]` or `[...name]`.
    #[error("the segment `{segment}` is neither a literal nor a parameter")]
    MalformedParamSegment {
        /// The segment that is neither one thing nor the other.
        segment: String,
    },

    /// A catch-all segment has segments after it.
    #[error("the catch-all `[...{name}]` must be the last segment")]
    CatchAllNotLast {
        /// The catch-all parameter name.
        name: String,
    },

    /// Removing the extension leaves the file with no name.
    #[error("the file name `{segment}` is only an extension")]
    EmptyFileStem {
        /// The offending file name.
        segment: String,
    },

    /// The route built from the file is not a valid template.
    #[error("the path `{path}` produces an invalid template")]
    Template {
        /// The rejected path.
        path: String,
        /// Why the produced template is invalid.
        #[source]
        source: TemplateError,
    },
}

/// Convert a page file path into a route template.
///
/// `base` is the pages directory. Pass `""` when `file` is already relative
/// to it. Both arguments accept `/` or `\` as the separator.
///
/// ```
/// use brocanth::routing::route_for_file;
///
/// assert_eq!(route_for_file("pages/index.rs", "pages").unwrap().pattern(), "/");
/// assert_eq!(route_for_file("pages/users/[id].rs", "pages").unwrap().pattern(), "/users/{id}");
/// assert_eq!(route_for_file("pages/docs/[...rest].rs", "pages").unwrap().pattern(), "/docs/{*rest}");
/// ```
///
/// # Errors
///
/// Returns [`PathError`] when the file is outside the base, has an unusable
/// segment, uses a half-written parameter such as `[id].rs` beside other
/// text, or puts a catch-all anywhere but last.
pub fn route_for_file(file: &str, base: &str) -> Result<RouteTemplate, PathError> {
    let segments = route_segments(file, base)?;

    let mut pattern = String::new();
    for (index, segment) in segments.iter().enumerate() {
        let is_last = index + 1 == segments.len();
        pattern.push('/');
        match classify_segment(segment)? {
            Segment::Literal(text) => pattern.push_str(text),
            Segment::Dynamic(name) => {
                pattern.push('{');
                pattern.push_str(name);
                pattern.push('}');
            }
            Segment::CatchAll(name) => {
                if !is_last {
                    return Err(PathError::CatchAllNotLast {
                        name: name.to_owned(),
                    });
                }
                pattern.push_str("{*");
                pattern.push_str(name);
                pattern.push('}');
            }
        }
    }
    if pattern.is_empty() {
        pattern.push('/');
    }

    RouteTemplate::parse(&pattern).map_err(|source| PathError::Template {
        path: file.to_owned(),
        source,
    })
}

/// Derive the module path of a route helper from a page file path.
///
/// `pages/users/[id]/edit.rs` becomes `["users", "id", "edit"]`. A file that
/// maps to `/` produces an empty path; [`crate::routing::generate_helpers`]
/// puts that one under `root`.
///
/// # Errors
///
/// Same conditions as [`route_for_file`], plus a segment that cannot become a
/// Rust identifier.
pub fn module_path_for_file(file: &str, base: &str) -> Result<Vec<String>, PathError> {
    let segments = route_segments(file, base)?;
    let mut out = Vec::with_capacity(segments.len());
    for (index, segment) in segments.iter().enumerate() {
        let is_last = index + 1 == segments.len();
        let name = match classify_segment(segment)? {
            Segment::Literal(text) => text,
            Segment::Dynamic(name) => name,
            Segment::CatchAll(name) => {
                if !is_last {
                    return Err(PathError::CatchAllNotLast {
                        name: name.to_owned(),
                    });
                }
                name
            }
        };
        out.push(sanitize_ident(name));
    }
    Ok(out)
}

enum Segment<'a> {
    Literal(&'a str),
    Dynamic(&'a str),
    CatchAll(&'a str),
}

fn classify_segment(segment: &str) -> Result<Segment<'_>, PathError> {
    let malformed = || PathError::MalformedParamSegment {
        segment: segment.to_owned(),
    };

    if let Some(body) = segment.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        if body.contains('[') || body.contains(']') {
            return Err(malformed());
        }
        let name = body.strip_prefix("...").unwrap_or(body);
        if name.is_empty() {
            return Err(malformed());
        }
        return Ok(if body.starts_with("...") {
            Segment::CatchAll(name)
        } else {
            Segment::Dynamic(name)
        });
    }

    if segment.contains('[') || segment.contains(']') {
        return Err(malformed());
    }
    // A literal that carries matcher or template punctuation would turn into
    // a parameter further down. Refuse it here instead.
    if segment.contains(['{', '}', '<', '>']) {
        return Err(malformed());
    }
    Ok(Segment::Literal(segment))
}

/// Strip the base, drop the extension, collapse `index`, and split.
fn route_segments(file: &str, base: &str) -> Result<Vec<String>, PathError> {
    let normalized = file.replace('\\', "/");
    let base = base.replace('\\', "/");
    let base = base.trim_end_matches('/');

    let relative = if base.is_empty() {
        normalized.strip_prefix('/').unwrap_or(normalized.as_str())
    } else if let Some(rest) = normalized.strip_prefix(base) {
        match rest.strip_prefix('/') {
            Some(rest) => rest,
            None if rest.is_empty() => rest,
            None => {
                return Err(PathError::NotUnderBase {
                    path: file.to_owned(),
                    base: base.to_owned(),
                })
            }
        }
    } else {
        return Err(PathError::NotUnderBase {
            path: file.to_owned(),
            base: base.to_owned(),
        });
    };

    if relative.is_empty() {
        return Err(PathError::EmptyPath {
            path: file.to_owned(),
        });
    }

    let mut parts: Vec<String> = relative.split('/').map(ToOwned::to_owned).collect();
    for part in &parts {
        if part.is_empty() || part == "." || part == ".." {
            return Err(PathError::InvalidSegment {
                path: file.to_owned(),
                segment: part.clone(),
            });
        }
    }

    // Drop the extension from the file name only.
    let last = parts
        .last_mut()
        .expect("relative path is not empty, so it has a last segment");
    if let Some(dot) = last.rfind('.') {
        if dot == 0 {
            return Err(PathError::EmptyFileStem {
                segment: last.clone(),
            });
        }
        last.truncate(dot);
    }
    if last.is_empty() {
        return Err(PathError::EmptyFileStem {
            segment: relative.rsplit('/').next().unwrap_or(relative).to_owned(),
        });
    }

    // `index` names its directory.
    if parts.last().is_some_and(|p| p == "index") {
        parts.pop();
    }

    Ok(parts)
}

pub(crate) fn sanitize_ident(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 2);
    for (index, ch) in raw.chars().enumerate() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            if index == 0 && ch.is_ascii_digit() {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push('_');
    }
    if is_rust_keyword(&out) {
        out.insert_str(0, "r#");
    }
    out
}

pub(crate) fn is_rust_keyword(word: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "as", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false",
        "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
        "ref", "return", "static", "struct", "super", "trait", "true", "type", "unsafe", "use",
        "where", "while", "async", "await", "abstract", "become", "box", "do", "final", "macro",
        "override", "priv", "try", "typeof", "unsized", "virtual", "yield",
    ];
    // `self`, `Self` and `_` cannot be raw identifiers, so they are mangled
    // rather than escaped.
    KEYWORDS.contains(&word)
}

/// Percent-encode one path segment.
///
/// Everything outside the RFC 3986 unreserved set is encoded, `/` included.
/// Over-encoding is safe: a server decodes `%2F` back to `/`, and the result
/// can never split a segment the caller meant to keep whole.
#[must_use]
pub fn encode_segment(raw: &str) -> String {
    encode(raw, false)
}

/// Percent-encode a catch-all value, keeping `/` as a separator.
#[must_use]
pub fn encode_catch_all(raw: &str) -> String {
    encode(raw, true)
}

fn encode(raw: &str, keep_slash: bool) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        let unreserved = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~');
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

#[cfg(test)]
mod tests {
    use super::*;

    fn route(file: &str) -> String {
        route_for_file(file, "src/pages")
            .unwrap_or_else(|e| panic!("{file}: {e}"))
            .pattern()
            .to_owned()
    }

    // --- the file convention, case by case -------------------------------

    #[test]
    fn index_collapses_to_the_root() {
        assert_eq!(route("src/pages/index.rs"), "/");
    }

    #[test]
    fn a_plain_file_becomes_its_own_segment() {
        assert_eq!(route("src/pages/about.rs"), "/about");
    }

    #[test]
    fn index_collapses_to_its_directory() {
        assert_eq!(route("src/pages/users/index.rs"), "/users");
    }

    #[test]
    fn index_collapses_through_nested_directories() {
        assert_eq!(route("src/pages/admin/users/index.rs"), "/admin/users");
    }

    #[test]
    fn index_collapses_under_a_parameter_directory() {
        assert_eq!(route("src/pages/users/[id]/index.rs"), "/users/{id}");
    }

    #[test]
    fn only_the_last_index_collapses() {
        // A directory genuinely called `index` keeps its name.
        assert_eq!(route("src/pages/index/index.rs"), "/index");
    }

    #[test]
    fn bracket_becomes_a_named_param() {
        assert_eq!(route("src/pages/users/[id].rs"), "/users/{id}");
    }

    #[test]
    fn a_param_may_be_the_only_segment() {
        assert_eq!(route("src/pages/[slug].rs"), "/{slug}");
    }

    #[test]
    fn a_param_directory_may_carry_children() {
        assert_eq!(route("src/pages/users/[id]/edit.rs"), "/users/{id}/edit");
    }

    #[test]
    fn several_params_keep_their_order() {
        let template = route_for_file("src/pages/users/[uid]/posts/[pid].rs", "src/pages").unwrap();
        assert_eq!(template.pattern(), "/users/{uid}/posts/{pid}");
        let names: Vec<&str> = template.params().iter().map(ParamSpec::name).collect();
        assert_eq!(names, ["uid", "pid"]);
    }

    #[test]
    fn triple_dot_bracket_becomes_a_catch_all() {
        let template = route_for_file("src/pages/docs/[...rest].rs", "src/pages").unwrap();
        assert_eq!(template.pattern(), "/docs/{*rest}");
        assert_eq!(template.params()[0].kind(), ParamKind::CatchAll);
        assert!(template.params()[0].is_catch_all());
    }

    #[test]
    fn a_catch_all_may_be_the_only_segment() {
        assert_eq!(route("src/pages/[...path].rs"), "/{*path}");
    }

    #[test]
    fn a_catch_all_sits_beside_a_dynamic_sibling() {
        // The pair that decided the matcher. See `routing`'s module docs.
        assert_eq!(route("src/pages/docs/[slug].rs"), "/docs/{slug}");
        assert_eq!(route("src/pages/docs/[...rest].rs"), "/docs/{*rest}");
    }

    #[test]
    fn the_extension_does_not_matter() {
        assert_eq!(route_for_file("pages/about.crx", "pages").unwrap().pattern(), "/about");
        assert_eq!(route_for_file("pages/about.html", "pages").unwrap().pattern(), "/about");
    }

    #[test]
    fn only_the_last_extension_is_dropped() {
        assert_eq!(route("src/pages/sitemap.xml.rs"), "/sitemap.xml");
    }

    #[test]
    fn a_file_without_an_extension_keeps_its_name() {
        assert_eq!(route("src/pages/README"), "/README");
    }

    #[test]
    fn an_empty_base_means_the_path_is_already_relative() {
        assert_eq!(route_for_file("users/[id].rs", "").unwrap().pattern(), "/users/{id}");
    }

    #[test]
    fn a_trailing_slash_on_the_base_is_ignored() {
        assert_eq!(
            route_for_file("src/pages/about.rs", "src/pages/").unwrap().pattern(),
            "/about"
        );
    }

    #[test]
    fn windows_separators_are_accepted() {
        assert_eq!(
            route_for_file(r"src\pages\users\[id].rs", r"src\pages").unwrap().pattern(),
            "/users/{id}"
        );
    }

    #[test]
    fn a_literal_segment_keeps_its_case_and_dashes() {
        assert_eq!(route("src/pages/Press-Kit.rs"), "/Press-Kit");
    }

    // --- conversions that must fail --------------------------------------

    #[test]
    fn a_file_outside_the_base_is_refused() {
        let error = route_for_file("src/other/about.rs", "src/pages").unwrap_err();
        assert!(matches!(error, PathError::NotUnderBase { .. }), "{error:?}");
    }

    #[test]
    fn a_base_that_is_only_a_prefix_of_the_directory_is_refused() {
        let error = route_for_file("src/pages_old/about.rs", "src/pages").unwrap_err();
        assert!(matches!(error, PathError::NotUnderBase { .. }), "{error:?}");
    }

    #[test]
    fn the_base_itself_names_no_route() {
        let error = route_for_file("src/pages", "src/pages").unwrap_err();
        assert!(matches!(error, PathError::EmptyPath { .. }), "{error:?}");
    }

    #[test]
    fn a_dot_dot_segment_is_refused() {
        let error = route_for_file("src/pages/../secret.rs", "src/pages").unwrap_err();
        assert!(matches!(error, PathError::InvalidSegment { .. }), "{error:?}");
    }

    #[test]
    fn a_doubled_slash_is_refused() {
        let error = route_for_file("src/pages//about.rs", "src/pages").unwrap_err();
        assert!(matches!(error, PathError::InvalidSegment { .. }), "{error:?}");
    }

    #[test]
    fn a_dotfile_is_refused() {
        let error = route_for_file("src/pages/.gitkeep", "src/pages").unwrap_err();
        assert!(matches!(error, PathError::EmptyFileStem { .. }), "{error:?}");
    }

    #[test]
    fn an_empty_bracket_is_refused() {
        let error = route_for_file("src/pages/[].rs", "src/pages").unwrap_err();
        assert!(matches!(error, PathError::MalformedParamSegment { .. }), "{error:?}");
    }

    #[test]
    fn an_empty_catch_all_is_refused() {
        let error = route_for_file("src/pages/[...].rs", "src/pages").unwrap_err();
        assert!(matches!(error, PathError::MalformedParamSegment { .. }), "{error:?}");
    }

    #[test]
    fn a_half_written_param_is_refused_rather_than_taken_as_a_literal() {
        for bad in ["src/pages/[id.rs", "src/pages/id].rs", "src/pages/x[id].rs", "src/pages/[a][b].rs"] {
            let error = route_for_file(bad, "src/pages").unwrap_err();
            assert!(
                matches!(error, PathError::MalformedParamSegment { .. }),
                "{bad}: {error:?}"
            );
        }
    }

    #[test]
    fn a_catch_all_that_is_not_last_is_refused() {
        let error = route_for_file("src/pages/[...rest]/edit.rs", "src/pages").unwrap_err();
        assert!(matches!(error, PathError::CatchAllNotLast { .. }), "{error:?}");
    }

    #[test]
    fn a_repeated_param_name_is_refused() {
        let error = route_for_file("src/pages/[id]/x/[id].rs", "src/pages").unwrap_err();
        assert!(
            matches!(
                error,
                PathError::Template {
                    source: TemplateError::DuplicateParam { .. },
                    ..
                }
            ),
            "{error:?}"
        );
    }

    #[test]
    fn a_param_name_that_is_not_an_identifier_is_refused() {
        let error = route_for_file("src/pages/[user-id].rs", "src/pages").unwrap_err();
        assert!(
            matches!(
                error,
                PathError::Template {
                    source: TemplateError::InvalidParamName { .. },
                    ..
                }
            ),
            "{error:?}"
        );
    }

    #[test]
    fn a_literal_carrying_template_punctuation_is_refused() {
        // `a{b}.rs` must not quietly become the parameter `b`.
        for bad in ["src/pages/a{b}.rs", "src/pages/a<b>.rs"] {
            let error = route_for_file(bad, "src/pages").unwrap_err();
            assert!(
                matches!(error, PathError::MalformedParamSegment { .. }),
                "{bad}: {error:?}"
            );
        }
    }

    // --- template parsing -------------------------------------------------

    #[test]
    fn templates_parse_their_params_in_order() {
        let t = RouteTemplate::parse("/a/{one}/b/{two}/{*three}").unwrap();
        let got: Vec<(&str, ParamKind)> =
            t.params().iter().map(|p| (p.name(), p.kind())).collect();
        assert_eq!(
            got,
            [
                ("one", ParamKind::Dynamic),
                ("two", ParamKind::Dynamic),
                ("three", ParamKind::CatchAll),
            ]
        );
    }

    #[test]
    fn a_template_without_a_leading_slash_is_refused() {
        assert!(matches!(
            RouteTemplate::parse("users/{id}"),
            Err(TemplateError::MissingLeadingSlash { .. })
        ));
    }

    #[test]
    fn an_empty_template_is_refused() {
        assert!(matches!(RouteTemplate::parse(""), Err(TemplateError::Empty)));
    }

    #[test]
    fn unbalanced_braces_are_refused() {
        for bad in ["/a/{id", "/a/id}", "/a/{{id}", "/{a}}"] {
            assert!(
                matches!(
                    RouteTemplate::parse(bad),
                    Err(TemplateError::UnbalancedBrace { .. })
                ),
                "{bad} should be unbalanced"
            );
        }
    }

    #[test]
    fn angle_brackets_are_reserved() {
        assert!(matches!(
            RouteTemplate::parse("/a/<id>"),
            Err(TemplateError::ReservedCharacter { character: '<' })
        ));
    }

    #[test]
    fn engine_syntax_swaps_braces_for_angles() {
        let t = RouteTemplate::parse("/docs/{slug}/{*rest}").unwrap();
        assert_eq!(t.to_engine_syntax(), "/docs/<slug>/<*rest>");
    }

    #[test]
    fn the_root_template_has_no_params() {
        let t = RouteTemplate::parse("/").unwrap();
        assert_eq!(t.pattern(), "/");
        assert!(t.params().is_empty());
    }

    // --- module paths for code generation ---------------------------------

    #[test]
    fn module_paths_follow_the_same_rules() {
        let cases = [
            ("src/pages/index.rs", Vec::<&str>::new()),
            ("src/pages/about.rs", vec!["about"]),
            ("src/pages/users/index.rs", vec!["users"]),
            ("src/pages/users/[id].rs", vec!["users", "id"]),
            ("src/pages/users/[id]/edit.rs", vec!["users", "id", "edit"]),
            ("src/pages/docs/[...rest].rs", vec!["docs", "rest"]),
            ("src/pages/Press-Kit.rs", vec!["press_kit"]),
        ];
        for (file, expected) in cases {
            assert_eq!(
                module_path_for_file(file, "src/pages").unwrap(),
                expected,
                "{file}"
            );
        }
    }

    #[test]
    fn a_directory_named_after_a_keyword_becomes_a_raw_identifier() {
        assert_eq!(
            module_path_for_file("src/pages/type/index.rs", "src/pages").unwrap(),
            ["r#type"]
        );
    }

    #[test]
    fn a_directory_starting_with_a_digit_gets_a_prefix() {
        assert_eq!(
            module_path_for_file("src/pages/2024/index.rs", "src/pages").unwrap(),
            ["_2024"]
        );
    }

    // --- encoding ---------------------------------------------------------

    #[test]
    fn a_segment_cannot_escape_itself() {
        assert_eq!(encode_segment("a/b"), "a%2Fb");
        assert_eq!(encode_segment("hello world"), "hello%20world");
        assert_eq!(encode_segment("a?b#c"), "a%3Fb%23c");
    }

    #[test]
    fn unreserved_characters_pass_through() {
        assert_eq!(encode_segment("Az09-._~"), "Az09-._~");
    }

    #[test]
    fn a_catch_all_keeps_its_separators() {
        assert_eq!(encode_catch_all("a/b c"), "a/b%20c");
    }

    #[test]
    fn non_ascii_is_encoded_byte_by_byte() {
        assert_eq!(encode_segment("é"), "%C3%A9");
    }
}
