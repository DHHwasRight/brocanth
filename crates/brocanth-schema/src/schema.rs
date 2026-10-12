//! The `Schema` trait and the combinators: `optional`, `nullable`, `default`,
//! `union`, `intersection`, `refine`, `transform`.
//!
//! # Two methods, and why there are two
//!
//! [`Schema::check`] collects issues and allocates nothing.
//! [`Schema::parse_at`] collects issues and produces [`Schema::Output`].
//! Validating a struct that already exists wants the first; turning a wire
//! value into a typed one wants the second. Keeping them apart means a
//! `#[derive(Validate)]` struct is checked without copying every string in it.
//!
//! # Where a predicate sees its value
//!
//! [`Schema::refine`] inspects the schema's **input**, not the output of a
//! [`Schema::transform`] in front of it, because `check` must work without
//! producing an output at all. So `transform` goes last:
//!
//! ```
//! use brocanth_schema::{string, Schema};
//!
//! let slug = string()
//!     .min_length(3)
//!     .refine("no_spaces", "must not contain a space", |s: &str| !s.contains(' '))
//!     .transform(|s: String| s.to_lowercase());
//!
//! assert_eq!(slug.parse("Hello").unwrap(), "hello");
//! assert!(slug.parse("a b").is_err());
//! ```

use std::borrow::Cow;
use std::marker::PhantomData;

use crate::issue::{Issues, Path};
use crate::node::{LiteralValue, SchemaNode};
use crate::value::{ReadValue, Value};
use crate::IssueCode;

// ---------------------------------------------------------------------------
// The trait
// ---------------------------------------------------------------------------

/// One rule, and the description of that rule.
///
/// Implement it for a schema of your own; the combinators below work on
/// anything that does.
pub trait Schema {
    /// The value this schema checks. `str` for a string schema, `i64` for an
    /// integer one, [`Value`] for anything that has to see the shape itself.
    type Input: ?Sized;

    /// What a successful [`Schema::parse`] produces. Usually an owned echo of
    /// the input; [`Schema::transform`] is what changes it.
    type Output;

    /// The introspectable description. See [`SchemaNode`].
    fn describe(&self) -> SchemaNode;

    /// Record every failure, allocating nothing but the issues themselves.
    ///
    /// Every constraint runs. This never stops at the first failure.
    fn check(&self, input: &Self::Input, at: &mut Path, issues: &mut Issues);

    /// Record every failure and, if there were none, produce the output.
    fn parse_at(
        &self,
        input: &Self::Input,
        at: &mut Path,
        issues: &mut Issues,
    ) -> Option<Self::Output>;

    /// Check a whole value from the root.
    ///
    /// # Errors
    ///
    /// Every issue found, never just the first.
    fn validate(&self, input: &Self::Input) -> Result<(), Issues> {
        let mut at = Path::root();
        let mut issues = Issues::new();
        self.check(input, &mut at, &mut issues);
        if issues.is_empty() {
            Ok(())
        } else {
            Err(issues)
        }
    }

    /// Check a whole value from the root and produce the output.
    ///
    /// # Errors
    ///
    /// Every issue found, never just the first.
    fn parse(&self, input: &Self::Input) -> Result<Self::Output, Issues> {
        let mut at = Path::root();
        let mut issues = Issues::new();
        let out = self.parse_at(input, &mut at, &mut issues);
        match out {
            Some(value) if issues.is_empty() => Ok(value),
            _ => Err(issues),
        }
    }

    /// The field may be absent, or null.
    fn optional(self) -> Optional<Self>
    where
        Self: Sized,
    {
        Optional(self)
    }

    /// The field must be present and its value may be null.
    fn nullable(self) -> Nullable<Self>
    where
        Self: Sized,
    {
        Nullable(self)
    }

    /// The field may be absent, and this is what it means when it is.
    fn with_default(self, value: impl Into<LiteralValue>) -> WithDefault<Self>
    where
        Self: Sized,
    {
        WithDefault {
            inner: self,
            value: value.into(),
        }
    }

    /// Add a predicate no description can express.
    ///
    /// `code` is the machine-readable [`IssueCode::Refused`] payload and the
    /// name in the description; `message` is the sentence under the input.
    ///
    /// The predicate runs only when the inner schema accepted the value, so a
    /// "username is free" check never runs on something that is not even a
    /// username.
    fn refine<F>(
        self,
        code: impl Into<Cow<'static, str>>,
        message: impl Into<Cow<'static, str>>,
        predicate: F,
    ) -> Refined<Self, F>
    where
        Self: Sized,
        F: Fn(&Self::Input) -> bool,
    {
        Refined {
            inner: self,
            code: code.into(),
            message: message.into(),
            predicate,
        }
    }

    /// Convert the accepted value.
    ///
    /// The conversion runs on the output of a successful parse, so it is
    /// infallible by construction: a conversion that can fail is a
    /// [`Schema::refine`] before it.
    fn transform<T, F>(self, convert: F) -> Transformed<Self, F, T>
    where
        Self: Sized,
        F: Fn(Self::Output) -> T,
    {
        Transformed {
            inner: self,
            convert,
            _out: PhantomData,
        }
    }

    /// Erase the input type so this schema can sit in an object field or a
    /// union next to schemas that read something else.
    fn boxed(self) -> Box<dyn ValueSchema>
    where
        Self: Sized + Send + Sync + 'static,
        Self::Input: ReadValue,
    {
        Box::new(self)
    }
}

// ---------------------------------------------------------------------------
// The dynamic view
// ---------------------------------------------------------------------------

/// A schema seen through a [`Value`], with its input type erased.
///
/// An object's fields do not have one input type between them, so this is what
/// an object holds. Anything that is a [`Schema`] whose input can be found in a
/// `Value` is one of these, for free.
pub trait ValueSchema: Send + Sync + 'static {
    /// The description, same as [`Schema::describe`].
    fn describe_value(&self) -> SchemaNode;

    /// Check a `Value`, reporting `wrong_type` when it is the wrong shape.
    fn check_value(&self, value: &Value, at: &mut Path, issues: &mut Issues);
}

impl<S> ValueSchema for S
where
    S: Schema + Send + Sync + 'static,
    S::Input: ReadValue,
{
    fn describe_value(&self) -> SchemaNode {
        self.describe()
    }

    fn check_value(&self, value: &Value, at: &mut Path, issues: &mut Issues) {
        match <S::Input as ReadValue>::with(value, |input| self.check(input, at, issues)) {
            Ok(()) => {}
            Err(expected) => wrong_type(value, expected, at, issues),
        }
    }
}

/// Report a value that is not even the right kind.
pub(crate) fn wrong_type(value: &Value, expected: &str, at: &mut Path, issues: &mut Issues) {
    issues.report(at, IssueCode::WrongType, |i| {
        i.expected(expected.to_string())
            .received(value.kind())
            .message(format!("must be {expected}"))
    });
}

// ---------------------------------------------------------------------------
// optional / nullable / default
// ---------------------------------------------------------------------------

/// [`Schema::optional`]: the field may be absent, or null.
///
/// An object schema reads this out of the description and marks the field not
/// required; see [`SchemaNode::is_optional`].
#[derive(Clone, Debug)]
pub struct Optional<S>(pub(crate) S);

impl<S> Schema for Optional<S>
where
    S: Schema,
    S::Input: ReadValue,
{
    type Input = Value;
    type Output = Option<S::Output>;

    fn describe(&self) -> SchemaNode {
        SchemaNode::Optional(Box::new(self.0.describe()))
    }

    fn check(&self, input: &Value, at: &mut Path, issues: &mut Issues) {
        if input.is_null() {
            return;
        }
        match <S::Input as ReadValue>::with(input, |v| self.0.check(v, at, issues)) {
            Ok(()) => {}
            Err(expected) => wrong_type(input, expected, at, issues),
        }
    }

    fn parse_at(&self, input: &Value, at: &mut Path, issues: &mut Issues) -> Option<Self::Output> {
        if input.is_null() {
            return Some(None);
        }
        match <S::Input as ReadValue>::with(input, |v| self.0.parse_at(v, at, issues)) {
            Ok(out) => Some(out),
            Err(expected) => {
                wrong_type(input, expected, at, issues);
                None
            }
        }
    }
}

/// [`Schema::nullable`]: the field must be present and its value may be null.
#[derive(Clone, Debug)]
pub struct Nullable<S>(pub(crate) S);

impl<S> Schema for Nullable<S>
where
    S: Schema,
    S::Input: ReadValue,
{
    type Input = Value;
    type Output = Option<S::Output>;

    fn describe(&self) -> SchemaNode {
        SchemaNode::Nullable(Box::new(self.0.describe()))
    }

    fn check(&self, input: &Value, at: &mut Path, issues: &mut Issues) {
        if input.is_null() {
            return;
        }
        match <S::Input as ReadValue>::with(input, |v| self.0.check(v, at, issues)) {
            Ok(()) => {}
            Err(expected) => wrong_type(input, expected, at, issues),
        }
    }

    fn parse_at(&self, input: &Value, at: &mut Path, issues: &mut Issues) -> Option<Self::Output> {
        if input.is_null() {
            return Some(None);
        }
        match <S::Input as ReadValue>::with(input, |v| self.0.parse_at(v, at, issues)) {
            Ok(out) => Some(out),
            Err(expected) => {
                wrong_type(input, expected, at, issues);
                None
            }
        }
    }
}

/// [`Schema::with_default`]: the field may be absent, and this is what it means
/// when it is.
#[derive(Clone, Debug)]
pub struct WithDefault<S> {
    pub(crate) inner: S,
    pub(crate) value: LiteralValue,
}

impl<S> WithDefault<S> {
    /// What an absent field means.
    #[must_use]
    pub fn default_value(&self) -> &LiteralValue {
        &self.value
    }
}

impl<S> Schema for WithDefault<S>
where
    S: Schema,
    S::Input: ReadValue,
{
    type Input = Value;
    type Output = Option<S::Output>;

    fn describe(&self) -> SchemaNode {
        SchemaNode::Default {
            inner: Box::new(self.inner.describe()),
            value: self.value.clone(),
        }
    }

    fn check(&self, input: &Value, at: &mut Path, issues: &mut Issues) {
        if input.is_null() {
            return;
        }
        match <S::Input as ReadValue>::with(input, |v| self.inner.check(v, at, issues)) {
            Ok(()) => {}
            Err(expected) => wrong_type(input, expected, at, issues),
        }
    }

    fn parse_at(&self, input: &Value, at: &mut Path, issues: &mut Issues) -> Option<Self::Output> {
        if input.is_null() {
            return Some(None);
        }
        match <S::Input as ReadValue>::with(input, |v| self.inner.parse_at(v, at, issues)) {
            Ok(out) => Some(out),
            Err(expected) => {
                wrong_type(input, expected, at, issues);
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// refine / transform
// ---------------------------------------------------------------------------

/// [`Schema::refine`]: an inner schema plus a predicate.
#[derive(Clone, Debug)]
pub struct Refined<S, F> {
    pub(crate) inner: S,
    pub(crate) code: Cow<'static, str>,
    pub(crate) message: Cow<'static, str>,
    pub(crate) predicate: F,
}

impl<S, F> Schema for Refined<S, F>
where
    S: Schema,
    F: Fn(&S::Input) -> bool,
{
    type Input = S::Input;
    type Output = S::Output;

    fn describe(&self) -> SchemaNode {
        SchemaNode::Refined {
            inner: Box::new(self.inner.describe()),
            code: self.code.clone(),
            description: Some(self.message.clone()),
        }
    }

    fn check(&self, input: &Self::Input, at: &mut Path, issues: &mut Issues) {
        let before = issues.len();
        self.inner.check(input, at, issues);
        if issues.len() == before {
            self.run_predicate(input, at, issues);
        }
    }

    fn parse_at(
        &self,
        input: &Self::Input,
        at: &mut Path,
        issues: &mut Issues,
    ) -> Option<Self::Output> {
        let before = issues.len();
        let out = self.inner.parse_at(input, at, issues);
        if issues.len() != before {
            return None;
        }
        self.run_predicate(input, at, issues);
        if issues.len() == before {
            out
        } else {
            None
        }
    }
}

impl<S, F> Refined<S, F>
where
    S: Schema,
    F: Fn(&S::Input) -> bool,
{
    fn run_predicate(&self, input: &S::Input, at: &mut Path, issues: &mut Issues) {
        if !(self.predicate)(input) {
            let code = IssueCode::Refused(self.code.clone());
            issues.report(at, code, |i| {
                i.expected(self.message.to_string())
                    .received("a value the check refused")
                    .message(self.message.to_string())
            });
        }
    }
}

/// [`Schema::transform`]: an inner schema whose accepted value is converted.
pub struct Transformed<S, F, T> {
    pub(crate) inner: S,
    pub(crate) convert: F,
    pub(crate) _out: PhantomData<fn() -> T>,
}

impl<S: Clone, F: Clone, T> Clone for Transformed<S, F, T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            convert: self.convert.clone(),
            _out: PhantomData,
        }
    }
}

impl<S: std::fmt::Debug, F, T> std::fmt::Debug for Transformed<S, F, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transformed")
            .field("inner", &self.inner)
            .field("output", &std::any::type_name::<T>())
            .finish()
    }
}

impl<S, F, T> Schema for Transformed<S, F, T>
where
    S: Schema,
    F: Fn(S::Output) -> T,
{
    type Input = S::Input;
    type Output = T;

    fn describe(&self) -> SchemaNode {
        SchemaNode::Transformed {
            inner: Box::new(self.inner.describe()),
            output: Cow::Borrowed(std::any::type_name::<T>()),
        }
    }

    fn check(&self, input: &Self::Input, at: &mut Path, issues: &mut Issues) {
        self.inner.check(input, at, issues);
    }

    fn parse_at(
        &self,
        input: &Self::Input,
        at: &mut Path,
        issues: &mut Issues,
    ) -> Option<Self::Output> {
        self.inner
            .parse_at(input, at, issues)
            .map(|out| (self.convert)(out))
    }
}

// ---------------------------------------------------------------------------
// union / intersection
// ---------------------------------------------------------------------------

/// Any one of these. Built by [`union`].
pub struct Union {
    pub(crate) variants: Vec<Box<dyn ValueSchema>>,
}

/// An empty union, to be filled with [`Union::or`].
///
/// ```
/// use brocanth_schema::{int, literal, string, union, Schema, Value};
///
/// let id = union().or(int().min(1)).or(string().uuid());
///
/// assert!(id.validate(&Value::Int(7)).is_ok());
/// assert!(id.validate(&Value::text("not-a-uuid")).is_err());
///
/// let kind = union().or(literal("todo")).or(literal("done"));
/// assert!(kind.validate(&Value::text("done")).is_ok());
/// ```
#[must_use]
pub fn union() -> Union {
    Union {
        variants: Vec::new(),
    }
}

impl Union {
    /// Add a branch.
    #[must_use]
    pub fn or<S>(mut self, schema: S) -> Self
    where
        S: Schema + Send + Sync + 'static,
        S::Input: ReadValue,
    {
        self.variants.push(Box::new(schema));
        self
    }
}

impl Schema for Union {
    type Input = Value;
    type Output = ();

    fn describe(&self) -> SchemaNode {
        SchemaNode::Union(self.variants.iter().map(|v| v.describe_value()).collect())
    }

    fn check(&self, input: &Value, at: &mut Path, issues: &mut Issues) {
        let mut names = Vec::new();
        for variant in &self.variants {
            let mut probe = Issues::new();
            let mut probe_at = at.clone();
            variant.check_value(input, &mut probe_at, &mut probe);
            if probe.is_empty() {
                return;
            }
            names.push(describe_briefly(&variant.describe_value()));
        }
        let expected = if names.is_empty() {
            "a value (the union has no branches)".to_string()
        } else {
            format!("one of: {}", names.join(", "))
        };
        issues.report(at, IssueCode::NoUnionVariant, |i| {
            i.expected(expected).received(input.render())
        });
    }

    fn parse_at(&self, input: &Value, at: &mut Path, issues: &mut Issues) -> Option<()> {
        let before = issues.len();
        self.check(input, at, issues);
        (issues.len() == before).then_some(())
    }
}

/// All of these at once. Built by [`intersection`].
pub struct Intersection {
    pub(crate) parts: Vec<Box<dyn ValueSchema>>,
}

/// An empty intersection, to be filled with [`Intersection::and`].
///
/// Every part runs and every part's issues are kept, because "it failed one of
/// three rules" is not an answer a user can act on.
///
/// ```
/// use brocanth_schema::{intersection, object, string, Schema, Value};
///
/// let audited = intersection()
///     .and(object().field("name", string()).unknown_keys_ignored())
///     .and(object().field("author", string()).unknown_keys_ignored());
///
/// let ok = Value::object([("name", Value::text("a")), ("author", Value::text("b"))]);
/// assert!(audited.validate(&ok).is_ok());
/// ```
#[must_use]
pub fn intersection() -> Intersection {
    Intersection { parts: Vec::new() }
}

impl Intersection {
    /// Add a part.
    #[must_use]
    pub fn and<S>(mut self, schema: S) -> Self
    where
        S: Schema + Send + Sync + 'static,
        S::Input: ReadValue,
    {
        self.parts.push(Box::new(schema));
        self
    }
}

impl Schema for Intersection {
    type Input = Value;
    type Output = ();

    fn describe(&self) -> SchemaNode {
        SchemaNode::Intersection(self.parts.iter().map(|p| p.describe_value()).collect())
    }

    fn check(&self, input: &Value, at: &mut Path, issues: &mut Issues) {
        for part in &self.parts {
            part.check_value(input, at, issues);
        }
    }

    fn parse_at(&self, input: &Value, at: &mut Path, issues: &mut Issues) -> Option<()> {
        let before = issues.len();
        self.check(input, at, issues);
        (issues.len() == before).then_some(())
    }
}

/// A one-line name for a node, for a `no_union_variant` message.
pub(crate) fn describe_briefly(node: &SchemaNode) -> String {
    match node {
        SchemaNode::Literal(value) => value.to_json(),
        SchemaNode::Enum(e) => format!(
            "one of {}",
            e.variants
                .iter()
                .map(|v| format!("\"{v}\""))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        SchemaNode::String(s) => match s.format {
            Some(format) => format!("a {} string", format.as_str()),
            None => "text".to_string(),
        },
        SchemaNode::Integer(_) => "a whole number".to_string(),
        SchemaNode::Number(_) => "a number".to_string(),
        SchemaNode::Boolean => "a boolean".to_string(),
        SchemaNode::Date(_) => "a date".to_string(),
        SchemaNode::Time(_) => "a time".to_string(),
        SchemaNode::DateTime(_) => "a date and time".to_string(),
        SchemaNode::Array(_) => "an array".to_string(),
        SchemaNode::Object(_) => "an object".to_string(),
        other => other.kind().to_string(),
    }
}
