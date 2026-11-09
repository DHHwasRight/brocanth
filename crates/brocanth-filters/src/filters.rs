//! The declaration, the parse, and what a request asked for.

use std::fmt;

use brocanth_pagination::{query, Direction, Keyset};
use brocanth_policy::{Column, CompileError, Condition, Dialect, Entity, Pred, Value};

/// The query-string key of the sort: `sort=title`, `sort=-created_at`.
pub const SORT: &str = "sort";

/// What a filter's value is parsed into before it becomes a bind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A whole number.
    Int,
    /// A number with a fraction.
    Float,
    /// `true`, `false`, `1`, `0`, `on`, `off`, `yes`, `no`.
    Bool,
    /// Any text.
    Text,
    /// One of a fixed list of words: a status, a role.
    Choice(&'static [&'static str]),
}

impl Kind {
    fn describe(&self) -> String {
        match self {
            Kind::Int => "a whole number".to_string(),
            Kind::Float => "a number".to_string(),
            Kind::Bool => "true or false".to_string(),
            Kind::Text => "text".to_string(),
            Kind::Choice(words) => format!("one of {}", words.join(", ")),
        }
    }

    fn parse(&self, raw: &str) -> Option<Value> {
        match self {
            Kind::Int => raw.trim().parse::<i64>().ok().map(Value::Int),
            Kind::Float => raw
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|f| f.is_finite())
                .map(Value::Float),
            Kind::Bool => match raw.trim().to_ascii_lowercase().as_str() {
                "true" | "1" | "on" | "yes" => Some(Value::Bool(true)),
                "false" | "0" | "off" | "no" => Some(Value::Bool(false)),
                _ => None,
            },
            Kind::Text => Some(Value::Text(raw.to_string())),
            Kind::Choice(words) => words
                .iter()
                .find(|word| **word == raw)
                .map(|word| Value::Text((*word).to_string())),
        }
    }

    /// The lookups a kind allows when the declaration names none.
    fn lookups(&self) -> &'static [Lookup] {
        use Lookup::*;
        match self {
            Kind::Int | Kind::Float => &[Eq, Ne, Lt, Le, Gt, Ge, In],
            Kind::Bool => &[Eq],
            Kind::Text | Kind::Choice(_) => &[Eq, Ne, In],
        }
    }
}

/// How a value is compared: the suffix after `__` in the key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lookup {
    /// `status=open`. Repeated (`status=open&status=closed`, a group of
    /// checkboxes), it is `IN`.
    Eq,
    /// `status__ne=open`. Null-safe: a row with no status is kept.
    Ne,
    /// `score__lt=5`.
    Lt,
    /// `score__le=5`.
    Le,
    /// `score__gt=5`.
    Gt,
    /// `score__ge=5`.
    Ge,
    /// `status__in=open,closed`.
    In,
    /// `closed_at__null=true`, or `false` for "has a value".
    Null,
}

impl Lookup {
    /// The suffix, without `__`. Empty for [`Lookup::Eq`].
    pub fn suffix(self) -> &'static str {
        match self {
            Lookup::Eq => "",
            Lookup::Ne => "ne",
            Lookup::Lt => "lt",
            Lookup::Le => "le",
            Lookup::Gt => "gt",
            Lookup::Ge => "ge",
            Lookup::In => "in",
            Lookup::Null => "null",
        }
    }

    fn from_suffix(suffix: &str) -> Option<Lookup> {
        Some(match suffix {
            "" | "eq" => Lookup::Eq,
            "ne" => Lookup::Ne,
            "lt" => Lookup::Lt,
            "le" => Lookup::Le,
            "gt" => Lookup::Gt,
            "ge" => Lookup::Ge,
            "in" => Lookup::In,
            "null" => Lookup::Null,
            _ => return None,
        })
    }

    fn key(self, name: &str) -> String {
        match self {
            Lookup::Eq => name.to_string(),
            other => format!("{name}__{}", other.suffix()),
        }
    }
}

#[derive(Clone, Debug)]
struct Field<C> {
    name: &'static str,
    column: C,
    kind: Kind,
    lookups: Vec<Lookup>,
}

/// A sort a request asked for: a declared column and a direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort<C> {
    /// The query-string name: `created_at`.
    pub name: &'static str,
    /// The column.
    pub column: C,
    /// Which way.
    pub direction: Direction,
}

/// The filters and sorts one list accepts.
///
/// Built once, in code, from the entity's own column enum. Each name in the
/// query string is looked up here; a name that is not declared names no
/// column and is left alone, because `page`, `q` and the rest share the
/// query string.
#[derive(Clone, Debug)]
pub struct Filters<C: Column> {
    fields: Vec<Field<C>>,
    sorts: Vec<(&'static str, C)>,
    default_sort: Option<Sort<C>>,
}

impl<C: Column> Default for Filters<C> {
    fn default() -> Self {
        Filters {
            fields: Vec::new(),
            sorts: Vec::new(),
            default_sort: None,
        }
    }
}

impl<C: Column> Filters<C> {
    /// No filter, no sort.
    pub fn new() -> Self {
        Filters::default()
    }

    /// A filter of `kind` on `column`, read from `name`, with the lookups
    /// the kind allows.
    pub fn field(mut self, name: &'static str, column: C, kind: Kind) -> Self {
        self.fields.push(Field {
            name,
            column,
            kind,
            lookups: kind.lookups().to_vec(),
        });
        self
    }

    /// A whole-number filter: `=`, `ne`, `lt`, `le`, `gt`, `ge`, `in`.
    pub fn int(self, name: &'static str, column: C) -> Self {
        self.field(name, column, Kind::Int)
    }

    /// A number filter, with the lookups of [`Filters::int`].
    pub fn float(self, name: &'static str, column: C) -> Self {
        self.field(name, column, Kind::Float)
    }

    /// A text filter: `=`, `ne`, `in`. Exact: a word inside the text is a
    /// search, and `brocanth-search` does it with an index.
    pub fn text(self, name: &'static str, column: C) -> Self {
        self.field(name, column, Kind::Text)
    }

    /// A filter whose value is one of `choices`: `=`, `ne`, `in`. Any other
    /// value is refused, so a select box cannot be edited into a query.
    pub fn choice(self, name: &'static str, column: C, choices: &'static [&'static str]) -> Self {
        self.field(name, column, Kind::Choice(choices))
    }

    /// A yes-or-no filter: `=`.
    pub fn boolean(self, name: &'static str, column: C) -> Self {
        self.field(name, column, Kind::Bool)
    }

    /// Lets the last declared filter ask for absence: `name__null=true`.
    pub fn nullable(mut self) -> Self {
        if let Some(field) = self.fields.last_mut() {
            if !field.lookups.contains(&Lookup::Null) {
                field.lookups.push(Lookup::Null);
            }
        }
        self
    }

    /// Replaces the lookups of the last declared filter.
    pub fn only(mut self, lookups: &[Lookup]) -> Self {
        if let Some(field) = self.fields.last_mut() {
            field.lookups = lookups.to_vec();
        }
        self
    }

    /// A sort on `column`, asked for as `sort=name` or `sort=-name`.
    pub fn sort(mut self, name: &'static str, column: C) -> Self {
        self.sorts.push((name, column));
        self
    }

    /// The sort when the request names none. It need not be declared with
    /// [`Filters::sort`].
    pub fn default_sort(mut self, name: &'static str, column: C, direction: Direction) -> Self {
        self.default_sort = Some(Sort {
            name,
            column,
            direction,
        });
        self
    }

    /// Reads the filters and the sort a query string asks for.
    ///
    /// An empty value is no filter: a GET form sends `status=` for a select
    /// left on "any".
    ///
    /// # Errors
    ///
    /// [`FilterError`] for a lookup the filter does not allow, a value that
    /// is not its kind, or a sort on a column that is not declared. Each one
    /// is a 400: ignoring it would answer a different question than the one
    /// that was asked.
    pub fn parse(&self, query_string: &str) -> Result<Applied<C>, FilterError> {
        let mut parts: Vec<Pred<C>> = Vec::new();
        let mut active: Vec<Active> = Vec::new();
        let mut equal: Vec<(usize, Vec<Value>)> = Vec::new();
        let mut sort = None;

        for (key, raw) in query::pairs(query_string) {
            if key == SORT {
                if !raw.is_empty() {
                    sort = Some(self.sort_from(&raw)?);
                }
                continue;
            }
            let (name, suffix) = key.split_once("__").unwrap_or((key.as_str(), ""));
            let Some((index, field)) = self.fields.iter().enumerate().find(|(_, f)| f.name == name)
            else {
                continue;
            };
            let Some(lookup) = Lookup::from_suffix(suffix).filter(|l| field.lookups.contains(l))
            else {
                return Err(FilterError::Lookup {
                    key: key.clone(),
                    allowed: field.lookups.iter().map(|l| l.key(field.name)).collect(),
                });
            };
            if raw.is_empty() {
                continue;
            }
            let value = |raw: &str| {
                field.kind.parse(raw).ok_or_else(|| FilterError::Value {
                    key: key.clone(),
                    value: raw.to_string(),
                    expected: field.kind.describe(),
                })
            };
            let pred = match lookup {
                Lookup::Eq => {
                    let v = value(&raw)?;
                    match equal.iter_mut().find(|(i, _)| *i == index) {
                        Some((_, values)) => values.push(v),
                        None => equal.push((index, vec![v])),
                    }
                    None
                }
                Lookup::Ne => Some(Pred::is_not(field.column, value(&raw)?)),
                Lookup::Lt => Some(Pred::lt(field.column, value(&raw)?)),
                Lookup::Le => Some(Pred::le(field.column, value(&raw)?)),
                Lookup::Gt => Some(Pred::gt(field.column, value(&raw)?)),
                Lookup::Ge => Some(Pred::ge(field.column, value(&raw)?)),
                Lookup::In => {
                    let values = raw
                        .split(',')
                        .filter(|part| !part.is_empty())
                        .map(value)
                        .collect::<Result<Vec<_>, _>>()?;
                    Some(Pred::one_of(field.column, values))
                }
                Lookup::Null => match Kind::Bool.parse(&raw) {
                    Some(Value::Bool(true)) => Some(Pred::is_null(field.column)),
                    Some(_) => Some(Pred::is_not_null(field.column)),
                    None => {
                        return Err(FilterError::Value {
                            key: key.clone(),
                            value: raw.clone(),
                            expected: Kind::Bool.describe(),
                        })
                    }
                },
            };
            parts.extend(pred);
            active.push(Active {
                name: field.name,
                lookup,
                value: raw,
            });
        }

        for (index, mut values) in equal {
            let column = self.fields[index].column;
            parts.push(if values.len() == 1 {
                Pred::eq(column, values.remove(0))
            } else {
                Pred::one_of(column, values)
            });
        }

        Ok(Applied {
            pred: Pred::all(parts),
            active,
            sort: sort.or(self.default_sort),
            asked_sort: sort.is_some(),
        })
    }

    fn sort_from(&self, raw: &str) -> Result<Sort<C>, FilterError> {
        let (direction, name) = match raw.strip_prefix('-') {
            Some(rest) => (Direction::Descending, rest),
            None => (Direction::Ascending, raw.trim_start_matches('+')),
        };
        self.sorts
            .iter()
            .find(|(declared, _)| *declared == name)
            .map(|(declared, column)| Sort {
                name: declared,
                column: *column,
                direction,
            })
            .ok_or_else(|| FilterError::Sort {
                name: name.to_string(),
                allowed: self.sorts.iter().map(|(n, _)| (*n).to_string()).collect(),
            })
    }
}

/// One filter a request set, as it was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Active {
    /// The declared name.
    pub name: &'static str,
    /// How it compares.
    pub lookup: Lookup,
    /// The raw value, decoded.
    pub value: String,
}

/// What one request asked for: a predicate and a sort.
#[derive(Clone, Debug)]
pub struct Applied<C: Column> {
    pred: Pred<C>,
    active: Vec<Active>,
    sort: Option<Sort<C>>,
    asked_sort: bool,
}

impl<C: Column> Applied<C> {
    /// The filters, as one predicate. [`Pred::Always`] when none is set.
    ///
    /// Do not compile it alone: [`Applied::within`] ANDs it with the rule.
    pub fn pred(&self) -> &Pred<C> {
        &self.pred
    }

    /// The filters, owned, for a caller that ANDs them itself, such as
    /// `brocanth_resource::Resource::filter`.
    pub fn into_pred(self) -> Pred<C> {
        self.pred
    }

    /// The policy's rule AND the filters.
    ///
    /// A filter narrows what the rule admits and never widens it. A rule of
    /// [`Pred::Never`] stays `Never`.
    pub fn within(&self, rule: Pred<C>) -> Pred<C> {
        rule.and(self.pred.clone())
    }

    /// The rule AND the filters, compiled: the `WHERE` of the count and of
    /// the page. Give both the same condition, so the total counts the rows
    /// the pages show.
    ///
    /// # Errors
    ///
    /// What [`Pred::condition`] returns.
    pub fn condition<E: Entity<Column = C>>(
        &self,
        rule: Pred<C>,
        dialect: Dialect,
    ) -> Result<Condition, CompileError> {
        self.within(rule).condition::<E>(dialect)
    }

    /// The sort the request asked for, or the declared default.
    pub fn sort(&self) -> Option<Sort<C>> {
        self.sort
    }

    /// The keyset order for this sort, with `key` (the primary key) as the
    /// tie-breaker. The key alone, ascending, when there is no sort.
    pub fn keyset(&self, key: C) -> Keyset<C> {
        match self.sort {
            Some(sort) => Keyset::new(sort.column, key, sort.direction),
            None => Keyset::by_key(key, Direction::Ascending),
        }
    }

    /// The `ORDER BY` list for this sort, with `key` last so two pages never
    /// share a row: `"posts"."created_at" DESC, "posts"."id" DESC`.
    ///
    /// # Errors
    ///
    /// What [`Keyset::order_by`] returns.
    pub fn order_by<E: Entity<Column = C>>(&self, key: C) -> Result<String, CompileError> {
        self.keyset(key).order_by::<E>()
    }

    /// Each filter the request set, in the order it set them.
    pub fn active(&self) -> &[Active] {
        &self.active
    }

    /// The raw value of `name` with `lookup`, to fill a form field back in.
    pub fn value(&self, name: &str, lookup: Lookup) -> Option<&str> {
        self.active
            .iter()
            .find(|a| a.name == name && a.lookup == lookup)
            .map(|a| a.value.as_str())
    }

    /// Whether `name` is set to `value` with `=`: a checkbox to tick, an
    /// option to select.
    pub fn is(&self, name: &str, value: &str) -> bool {
        self.active
            .iter()
            .any(|a| a.name == name && a.lookup == Lookup::Eq && a.value == value)
    }

    /// Whether the request set no filter.
    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }

    /// The `sort` value to write back into a link: `-created_at`. `None`
    /// when the request named none, so a link keeps the default implicit.
    pub fn sort_param(&self) -> Option<String> {
        let sort = self.sort.filter(|_| self.asked_sort)?;
        Some(match sort.direction {
            Direction::Ascending => sort.name.to_string(),
            Direction::Descending => format!("-{}", sort.name),
        })
    }
}

/// A query string the declaration refuses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilterError {
    /// A lookup the filter does not allow: `status__gt`.
    Lookup {
        /// The key as written.
        key: String,
        /// The keys the filter accepts.
        allowed: Vec<String>,
    },
    /// A value that is not the filter's kind.
    Value {
        /// The key as written.
        key: String,
        /// The value as written.
        value: String,
        /// What the filter takes.
        expected: String,
    },
    /// A sort on a name that is not declared.
    Sort {
        /// The name as written, without `-`.
        name: String,
        /// The names that may be sorted on.
        allowed: Vec<String>,
    },
}

impl fmt::Display for FilterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FilterError::Lookup { key, allowed } => write!(
                f,
                "`{key}` is not a filter this list accepts\n\
                 help: use one of {}",
                allowed.join(", ")
            ),
            FilterError::Value {
                key,
                value,
                expected,
            } => write!(
                f,
                "`{key}={value}`: the value is not {expected}\n\
                 help: give `{key}` {expected}, or leave it empty to drop the filter"
            ),
            FilterError::Sort { name, allowed } if allowed.is_empty() => write!(
                f,
                "`sort={name}`: this list has no sort\n\
                 help: drop `sort`"
            ),
            FilterError::Sort { name, allowed } => write!(
                f,
                "`sort={name}`: `{name}` is not a column this list sorts on\n\
                 help: use one of {}, with `-` in front for largest first",
                allowed.join(", ")
            ),
        }
    }
}

impl std::error::Error for FilterError {}
