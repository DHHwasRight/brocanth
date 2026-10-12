//! Scope patterns: the predicate side of a template.
//!
//! Middleware attaches to a pattern, not to a route, so the question a scope
//! pattern answers is "does this request path fall inside?" and nothing
//! more. A radix trie is the wrong structure for that: a path can fall
//! inside several scopes at once (`/{*rest}` and `/admin/{*rest}` both cover
//! `/admin/users`), and the stack needs *every* match in registration order,
//! not the single best one. So a scope pattern is a compiled segment list
//! and the stack walks them.
//!
//! One deliberate difference from the route matcher: here a catch-all
//! matches **zero or more** segments, so `/admin/{*rest}` covers `/admin`
//! itself. A scope names a subtree, and a subtree includes its own root.

use core::fmt;

use crate::routing_path::TemplateError;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Literal(String),
    /// Exactly one segment, any content.
    Dynamic,
    /// Zero or more segments. Only valid as the last segment.
    CatchAll,
}

/// A compiled path predicate, written in the same syntax as a route
/// template.
///
/// ```
/// use brocanth::routing::ScopePattern;
///
/// let scope = ScopePattern::parse("/admin/{*rest}").unwrap();
/// assert!(scope.matches("/admin"));
/// assert!(scope.matches("/admin/users/7"));
/// assert!(!scope.matches("/public"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopePattern {
    source: String,
    segments: Vec<Segment>,
}

impl ScopePattern {
    /// Compile a pattern.
    ///
    /// Parameter names are parsed and validated for consistency with route
    /// templates, then discarded: a scope decides yes or no, it does not
    /// capture. Keeping capture out of scopes is what stops middleware
    /// parameters from shadowing route parameters, which is how the Crystal
    /// prototype made `ctx.param` ambiguous.
    ///
    /// # Errors
    ///
    /// Returns [`TemplateError`] on the same grounds as
    /// [`crate::routing::RouteTemplate::parse`], plus a catch-all that is
    /// not the last segment.
    pub fn parse(pattern: &str) -> Result<Self, TemplateError> {
        let template = crate::routing_path::RouteTemplate::parse(pattern)?;
        let body = template.pattern().trim_end_matches('/');
        let body = body.strip_prefix('/').unwrap_or(body);

        let mut segments = Vec::new();
        if !body.is_empty() {
            let raw: Vec<&str> = body.split('/').collect();
            for (index, part) in raw.iter().enumerate() {
                let is_last = index + 1 == raw.len();
                let segment = if let Some(inner) = part.strip_prefix('{').and_then(|p| p.strip_suffix('}')) {
                    if let Some(name) = inner.strip_prefix('*') {
                        if !is_last {
                            return Err(TemplateError::InvalidParamName {
                                name: format!("*{name}"),
                            });
                        }
                        Segment::CatchAll
                    } else {
                        Segment::Dynamic
                    }
                } else {
                    Segment::Literal((*part).to_owned())
                };
                segments.push(segment);
            }
        }

        Ok(Self {
            source: pattern.to_owned(),
            segments,
        })
    }

    /// The pattern as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.source
    }

    /// Whether `path` falls inside this scope.
    ///
    /// A trailing slash on the path is ignored, so `/admin` and `/admin/`
    /// give the same answer.
    #[must_use]
    pub fn matches(&self, path: &str) -> bool {
        let body = path.trim_end_matches('/');
        let body = body.strip_prefix('/').unwrap_or(body);
        let parts: Vec<&str> = if body.is_empty() {
            Vec::new()
        } else {
            body.split('/').collect()
        };

        let mut index = 0;
        for segment in &self.segments {
            match segment {
                Segment::Literal(text) => {
                    if parts.get(index) != Some(&text.as_str()) {
                        return false;
                    }
                    index += 1;
                }
                Segment::Dynamic => {
                    if index >= parts.len() {
                        return false;
                    }
                    index += 1;
                }
                // Zero or more, and always last, so the rest is consumed.
                Segment::CatchAll => return true,
            }
        }
        index == parts.len()
    }
}

impl fmt::Display for ScopePattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.source)
    }
}
