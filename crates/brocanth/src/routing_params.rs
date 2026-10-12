//! Route parameters that cannot lie.
//!
//! The Crystal prototype returned `""` for a name it did not know, so a
//! mistyped `ctx.param("usr_id")` produced an empty string and the handler
//! carried on with it. Here an unknown name is [`None`], and the typed
//! accessors turn a missing or unparseable value into a 400 instead of a
//! panic or a default.
//!
//! Values borrow from the request path and from the router, so there is no
//! copy and no ceiling on how many parameters a route may have.

use core::fmt;
use core::str::FromStr;

/// The parameters captured by a route match.
///
/// Both the names and the values are borrowed, so a `Params` lives only as
/// long as the match it came from.
#[derive(Clone, Copy)]
pub struct Params<'a> {
    entries: &'a [(&'a str, &'a str)],
}

impl<'a> Params<'a> {
    /// Wrap a slice of name/value pairs.
    #[must_use]
    pub const fn new(entries: &'a [(&'a str, &'a str)]) -> Self {
        Self { entries }
    }

    /// An empty set, for a route with no parameters.
    #[must_use]
    pub const fn empty() -> Self {
        Self { entries: &[] }
    }

    /// How many parameters the route captured.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the route captured nothing.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The value of `name`, or [`None`] if the route has no such parameter.
    ///
    /// A name the route never declared returns [`None`]. It never returns an
    /// empty string in its place.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&'a str> {
        self.entries
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| *value)
    }

    /// Whether `name` was captured.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// The value of `name`, or a 400-shaped error.
    ///
    /// # Errors
    ///
    /// [`ParamError::missing`] if the route has no parameter of that name.
    pub fn require(&self, name: &str) -> Result<&'a str, ParamError> {
        self.get(name).ok_or_else(|| ParamError::missing(name))
    }

    /// Parse `name` into `T`, or return a 400-shaped error.
    ///
    /// Use this instead of `get(..).unwrap().parse().unwrap()`. A missing
    /// name and an unparseable value both produce a [`ParamError`], which
    /// reports [`http::StatusCode::BAD_REQUEST`].
    ///
    /// # Errors
    ///
    /// [`ParamErrorKind::Missing`] when the route has no such parameter, and
    /// [`ParamErrorKind::Invalid`] when `T::from_str` rejects the value.
    pub fn parse<T>(&self, name: &str) -> Result<T, ParamError>
    where
        T: FromStr,
        T::Err: fmt::Display,
    {
        let raw = self.require(name)?;
        T::from_str(raw).map_err(|error| {
            ParamError::invalid(name, raw, core::any::type_name::<T>(), &error.to_string())
        })
    }

    /// Every captured pair, in the order the template declares them.
    pub fn iter(&self) -> impl Iterator<Item = (&'a str, &'a str)> + '_ {
        self.entries.iter().copied()
    }
}

impl<'a> IntoIterator for Params<'a> {
    type Item = (&'a str, &'a str);
    type IntoIter = core::iter::Copied<core::slice::Iter<'a, (&'a str, &'a str)>>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter().copied()
    }
}

impl fmt::Debug for Params<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.entries.iter().copied()).finish()
    }
}

/// Why a parameter could not be delivered as the handler asked for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamErrorKind {
    /// The route declares no parameter of that name.
    Missing,
    /// The value is present but the target type rejected it.
    Invalid {
        /// The raw value taken from the path.
        value: String,
        /// The type that was asked for.
        expected: &'static str,
        /// What the parser reported.
        reason: String,
    },
}

/// A parameter the handler asked for and did not get.
///
/// Both kinds map to 400. A typo in the name is a programming error, but it
/// is also indistinguishable at runtime from a route that changed shape, so
/// reporting it as a bad request keeps the server alive and the cause
/// visible in the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamError {
    name: String,
    kind: ParamErrorKind,
}

impl ParamError {
    /// Build a "no such parameter" error.
    #[must_use]
    pub fn missing(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            kind: ParamErrorKind::Missing,
        }
    }

    /// Build a "present but unusable" error.
    #[must_use]
    pub fn invalid(name: &str, value: &str, expected: &'static str, reason: &str) -> Self {
        Self {
            name: name.to_owned(),
            kind: ParamErrorKind::Invalid {
                value: value.to_owned(),
                expected,
                reason: reason.to_owned(),
            },
        }
    }

    /// The parameter name the handler asked for.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether the name was unknown or the value unusable.
    #[must_use]
    pub const fn kind(&self) -> &ParamErrorKind {
        &self.kind
    }

    /// The status to answer with: always [`http::StatusCode::BAD_REQUEST`].
    #[must_use]
    pub const fn status(&self) -> http::StatusCode {
        http::StatusCode::BAD_REQUEST
    }
}

impl fmt::Display for ParamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ParamErrorKind::Missing => {
                write!(f, "the route has no parameter named `{}`", self.name)
            }
            ParamErrorKind::Invalid {
                value,
                expected,
                reason,
            } => write!(
                f,
                "the parameter `{}` has the value `{value}`, which is not a valid {expected}: {reason}",
                self.name
            ),
        }
    }
}

impl core::error::Error for ParamError {}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTRIES: &[(&str, &str)] = &[("user_id", "42"), ("slug", "hello-world")];

    fn params() -> Params<'static> {
        Params::new(ENTRIES)
    }

    #[test]
    fn a_known_name_gives_its_value() {
        assert_eq!(params().get("user_id"), Some("42"));
        assert_eq!(params().get("slug"), Some("hello-world"));
    }

    #[test]
    fn an_unknown_name_gives_none_not_an_empty_string() {
        // The prototype's defect: `usr_id` returned "" and the handler
        // carried on with it.
        assert_eq!(params().get("usr_id"), None);
        assert_ne!(params().get("usr_id"), Some(""));
        assert!(!params().contains("usr_id"));
    }

    #[test]
    fn an_unknown_name_is_a_bad_request_not_a_panic() {
        let error = params().require("usr_id").unwrap_err();
        assert_eq!(error.name(), "usr_id");
        assert_eq!(error.kind(), &ParamErrorKind::Missing);
        assert_eq!(error.status(), http::StatusCode::BAD_REQUEST);
        assert_eq!(
            error.to_string(),
            "the route has no parameter named `usr_id`"
        );
    }

    #[test]
    fn a_typed_read_parses_the_value() {
        assert_eq!(params().parse::<u64>("user_id").unwrap(), 42);
        assert_eq!(params().parse::<String>("slug").unwrap(), "hello-world");
    }

    #[test]
    fn an_unparseable_value_is_a_bad_request_not_a_panic() {
        let error = params().parse::<u64>("slug").unwrap_err();
        assert_eq!(error.status(), http::StatusCode::BAD_REQUEST);
        match error.kind() {
            ParamErrorKind::Invalid { value, expected, .. } => {
                assert_eq!(value, "hello-world");
                assert_eq!(*expected, "u64");
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
        assert!(error.to_string().contains("not a valid u64"), "{error}");
    }

    #[test]
    fn a_typed_read_of_an_unknown_name_reports_missing_not_invalid() {
        let error = params().parse::<u64>("nope").unwrap_err();
        assert_eq!(error.kind(), &ParamErrorKind::Missing);
    }

    #[test]
    fn iteration_keeps_template_order() {
        let seen: Vec<(&str, &str)> = params().iter().collect();
        assert_eq!(seen, [("user_id", "42"), ("slug", "hello-world")]);
    }

    #[test]
    fn an_empty_set_reports_itself_as_empty() {
        let empty = Params::empty();
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);
        assert_eq!(empty.get("anything"), None);
    }

    #[test]
    fn debug_shows_the_pairs() {
        assert_eq!(
            format!("{:?}", params()),
            r#"{"user_id": "42", "slug": "hello-world"}"#
        );
    }
}
