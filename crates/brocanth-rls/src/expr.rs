//! [`SqlExpr`]: a sea-query expression and the definitions it uses.
//!
//! A policy's `USING` and `WITH CHECK` are DDL, and DDL takes no bind
//! parameter. So a value is written as a literal, by sea-query's Postgres
//! writer and its escaping, never by `format!`. Column, function and setting
//! names are quoted or checked by this crate.

use std::collections::{BTreeMap, BTreeSet};

use brocanth_db::sea_query::{
    Expr, ExprTrait, Func, PostgresQueryBuilder, QueryBuilder, SelectStatement, Value,
};

use crate::ident::{self, SqlType};

/// A boolean (or any) SQL expression for a policy, and what it needs.
///
/// ```
/// use brocanth_rls::{Setting, SqlExpr, SqlType};
///
/// let tenant = Setting::new("tenant_id", SqlType::BigInt);
/// let mine = SqlExpr::col("tenant_id").eq(tenant.get());
/// assert_eq!(
///     mine.to_sql(),
///     r#""tenant_id" = CAST(NULLIF(current_setting('brocanth.tenant_id', TRUE), '') AS bigint)"#
/// );
/// ```
#[derive(Debug, Clone)]
pub struct SqlExpr {
    expr: Expr,
    uses: Uses,
}

/// The definitions an expression needs: functions (by snapshot key) and
/// settings (by key, with their type).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Uses {
    pub(crate) functions: BTreeSet<String>,
    pub(crate) settings: BTreeMap<String, SqlType>,
    /// A refusal found while the expression was built, reported when the
    /// schema is checked: a builder cannot return a `Result` at each step.
    pub(crate) problems: Vec<String>,
}

impl Uses {
    pub(crate) fn merge(&mut self, other: &Uses) {
        self.functions.extend(other.functions.iter().cloned());
        for (key, ty) in &other.settings {
            if let Some(seen) = self.settings.get(key) {
                if seen != ty {
                    self.problems.push(format!(
                        "the setting `{key}` is read as `{}` and as `{}`",
                        seen.as_sql(),
                        ty.as_sql()
                    ));
                }
            } else {
                self.settings.insert(key.clone(), ty.clone());
            }
        }
        self.problems.extend(other.problems.iter().cloned());
    }
}

impl SqlExpr {
    fn of(expr: Expr) -> SqlExpr {
        SqlExpr {
            expr,
            uses: Uses::default(),
        }
    }

    fn with(expr: Expr, a: &SqlExpr, b: &SqlExpr) -> SqlExpr {
        let mut uses = a.uses.clone();
        uses.merge(&b.uses);
        SqlExpr { expr, uses }
    }

    /// A column of the policy's table: `"name"`.
    pub fn col(name: &str) -> SqlExpr {
        let mut e = SqlExpr::of(Expr::col(name.to_string()));
        if let Err(err) = ident::check("column", name) {
            e.uses.problems.push(err.to_string());
        }
        e
    }

    /// A column of another table, for a subquery: `"table"."name"`.
    pub fn table_col(table: &str, name: &str) -> SqlExpr {
        let mut e = SqlExpr::of(Expr::col((table.to_string(), name.to_string())));
        for (what, n) in [("table", table), ("column", name)] {
            if let Err(err) = ident::check(what, n) {
                e.uses.problems.push(err.to_string());
            }
        }
        e
    }

    /// A value, written as a literal by sea-query.
    pub fn val(value: impl Into<Value>) -> SqlExpr {
        SqlExpr::of(Expr::val(value))
    }

    /// `TRUE`: every row.
    pub fn always() -> SqlExpr {
        SqlExpr::val(true)
    }

    /// `FALSE`: no row.
    pub fn never() -> SqlExpr {
        SqlExpr::val(false)
    }

    /// `CURRENT_USER`: the role the transaction runs as.
    pub fn current_user() -> SqlExpr {
        SqlExpr::of(Expr::cust("CURRENT_USER"))
    }

    /// `EXISTS (<select>)`. sea-query cannot report what the select uses, so
    /// add it with [`SqlExpr::uses`].
    pub fn exists(select: SelectStatement) -> SqlExpr {
        SqlExpr::of(Expr::exists(select))
    }

    /// Any sea-query expression. Its dependencies are what you add with
    /// [`SqlExpr::uses`]. A string inside `Expr::cust` is SQL text: never
    /// put input there.
    pub fn raw(expr: Expr) -> SqlExpr {
        SqlExpr::of(expr)
    }

    /// Adds what `other` uses to this expression. For a subquery built with
    /// sea-query from another `SqlExpr` ([`SqlExpr::sea`]).
    pub fn uses(mut self, other: &SqlExpr) -> SqlExpr {
        self.uses.merge(&other.uses);
        self
    }

    /// The sea-query expression, for a subquery.
    pub fn sea(&self) -> Expr {
        self.expr.clone()
    }

    /// `self = other`.
    pub fn eq(self, other: SqlExpr) -> SqlExpr {
        let e = self.expr.clone().eq(other.expr.clone());
        SqlExpr::with(e, &self, &other)
    }

    /// `self <> other`.
    pub fn ne(self, other: SqlExpr) -> SqlExpr {
        let e = self.expr.clone().ne(other.expr.clone());
        SqlExpr::with(e, &self, &other)
    }

    /// `self < other`.
    pub fn lt(self, other: SqlExpr) -> SqlExpr {
        let e = self.expr.clone().lt(other.expr.clone());
        SqlExpr::with(e, &self, &other)
    }

    /// `self <= other`.
    pub fn le(self, other: SqlExpr) -> SqlExpr {
        let e = self.expr.clone().lte(other.expr.clone());
        SqlExpr::with(e, &self, &other)
    }

    /// `self > other`.
    pub fn gt(self, other: SqlExpr) -> SqlExpr {
        let e = self.expr.clone().gt(other.expr.clone());
        SqlExpr::with(e, &self, &other)
    }

    /// `self >= other`.
    pub fn ge(self, other: SqlExpr) -> SqlExpr {
        let e = self.expr.clone().gte(other.expr.clone());
        SqlExpr::with(e, &self, &other)
    }

    /// `self IS NOT DISTINCT FROM other`: equal, and `NULL` equals `NULL`.
    pub fn is(self, other: SqlExpr) -> SqlExpr {
        let e = Expr::cust_with_exprs(
            "$1 IS NOT DISTINCT FROM $2",
            [self.expr.clone(), other.expr.clone()],
        );
        SqlExpr::with(e, &self, &other)
    }

    /// `self AND other`.
    pub fn and(self, other: SqlExpr) -> SqlExpr {
        let e = self.expr.clone().and(other.expr.clone());
        SqlExpr::with(e, &self, &other)
    }

    /// `self OR other`.
    pub fn or(self, other: SqlExpr) -> SqlExpr {
        let e = self.expr.clone().or(other.expr.clone());
        SqlExpr::with(e, &self, &other)
    }

    /// `NOT self`.
    #[allow(clippy::should_implement_trait)]
    pub fn not(self) -> SqlExpr {
        SqlExpr {
            expr: self.expr.not(),
            uses: self.uses,
        }
    }

    /// `self IS NULL`.
    pub fn is_null(self) -> SqlExpr {
        SqlExpr {
            expr: self.expr.is_null(),
            uses: self.uses,
        }
    }

    /// `self IS NOT NULL`.
    pub fn is_not_null(self) -> SqlExpr {
        SqlExpr {
            expr: self.expr.is_not_null(),
            uses: self.uses,
        }
    }

    /// `self IN (v, …)`.
    pub fn is_in<V: Into<Value>>(self, values: impl IntoIterator<Item = V>) -> SqlExpr {
        let values: Vec<Value> = values.into_iter().map(Into::into).collect();
        SqlExpr {
            expr: self.expr.is_in(values),
            uses: self.uses,
        }
    }

    /// `CAST(self AS type)`.
    pub fn cast(self, ty: SqlType) -> SqlExpr {
        let mut uses = self.uses;
        if let Err(e) = ty.check() {
            uses.problems.push(e.to_string());
        }
        SqlExpr {
            expr: self.expr.cast_as(ty.as_sql().to_string()),
            uses,
        }
    }

    /// The expression as Postgres SQL, values written as literals.
    pub fn to_sql(&self) -> String {
        let mut sql = String::new();
        PostgresQueryBuilder.prepare_expr(&self.expr, &mut sql);
        sql
    }

    pub(crate) fn used(&self) -> &Uses {
        &self.uses
    }

    pub(crate) fn call(name_sql: String, key: String, args: Vec<SqlExpr>) -> SqlExpr {
        let mut uses = Uses::default();
        uses.functions.insert(key);
        let mut exprs = Vec::with_capacity(args.len());
        for arg in &args {
            uses.merge(&arg.uses);
            exprs.push(arg.expr.clone());
        }
        SqlExpr {
            expr: Func::cust(name_sql).args(exprs).into(),
            uses,
        }
    }

    pub(crate) fn param(n: usize) -> SqlExpr {
        SqlExpr::of(Expr::cust(format!("${n}")))
    }
}

/// A value the request sets on its transaction: `brocanth.<key>`.
///
/// [`Setting::get`] reads it as `type`, or `NULL` when the transaction did
/// not set it. `brocanth_db::Principal::set(key, value)` sets it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    key: String,
    ty: SqlType,
}

impl Setting {
    /// The setting `brocanth.<key>`, read as `ty`. `key` is
    /// `[a-z][a-z0-9_]*`; a schema refuses another.
    pub fn new(key: impl Into<String>, ty: SqlType) -> Setting {
        Setting {
            key: key.into(),
            ty,
        }
    }

    /// The key, without the `brocanth.` prefix.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The type the policy reads it as.
    pub fn sql_type(&self) -> &SqlType {
        &self.ty
    }

    /// `CAST(NULLIF(current_setting('brocanth.<key>', TRUE), '') AS <type>)`.
    ///
    /// `NULLIF` matters: after an earlier transaction on the same connection
    /// set the key, `current_setting` returns `''` and not `NULL`, and
    /// `''::bigint` is an error.
    pub fn get(&self) -> SqlExpr {
        let name = format!("{}{}", brocanth_db::principal::SETTING_PREFIX, self.key);
        let read = Func::cust("current_setting").arg(name).arg(true);
        let blank = Func::cust("NULLIF").arg(Expr::from(read)).arg("");
        let mut e = SqlExpr::of(Expr::from(blank).cast_as(self.ty.as_sql().to_string()));
        if let Err(err) = brocanth_db::principal::check_key(&self.key) {
            e.uses.problems.push(err.to_string());
        }
        if let Err(err) = self.ty.check() {
            e.uses.problems.push(err.to_string());
        }
        e.uses.settings.insert(self.key.clone(), self.ty.clone());
        e
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_is_a_literal_with_its_quotes_doubled() {
        let e = SqlExpr::col("status").eq(SqlExpr::val("it's'; DROP TABLE x; --"));
        assert_eq!(e.to_sql(), r#""status" = E'it\'s\'; DROP TABLE x; --'"#);
    }

    #[test]
    fn a_line_break_in_a_value_stays_on_its_line() {
        let e = SqlExpr::col("t").eq(SqlExpr::val("a\n-- down\nb"));
        assert!(!e.to_sql().contains('\n'), "{}", e.to_sql());
    }

    #[test]
    fn a_column_name_is_quoted() {
        assert_eq!(SqlExpr::col(r#"we"ird"#).to_sql(), r#""we""ird""#);
    }

    #[test]
    fn combinators_merge_what_each_side_uses() {
        let a = Setting::new("tenant_id", SqlType::BigInt);
        let b = Setting::new("user_id", SqlType::BigInt);
        let e = SqlExpr::col("tenant_id")
            .eq(a.get())
            .and(SqlExpr::col("owner_id").eq(b.get()).or(SqlExpr::always()));
        assert_eq!(
            e.used().settings.keys().collect::<Vec<_>>(),
            ["tenant_id", "user_id"]
        );
    }

    #[test]
    fn one_setting_read_as_two_types_is_a_problem() {
        let a = Setting::new("tenant_id", SqlType::BigInt);
        let b = Setting::new("tenant_id", SqlType::Text);
        let e = a.get().eq(b.get());
        assert_eq!(e.used().problems.len(), 1, "{:?}", e.used().problems);
    }

    #[test]
    fn a_bad_key_is_a_problem_not_sql() {
        let e = Setting::new("x'); DROP", SqlType::Text).get();
        assert!(!e.used().problems.is_empty());
    }
}
