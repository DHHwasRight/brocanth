//! A statement this crate built, and the one-row admission test for a row that
//! does not exist yet.
//!
//! # Why there is a second statement type
//!
//! `brocanth_policy::Query` has no public constructor, which is correct: it is
//! what that crate's own compiler emits, and nothing else should be able to
//! pass itself off as one. This crate has to emit one statement of its own —
//! the create admission — so it carries its own [`Statement`] and converts a
//! `Query` into it. The data source sees one type either way.
//!
//! # The create admission
//!
//! PLAN.md §9 lists the four positions a rule occupies, and one of them is
//! *"an admission test against a synthesised one-row CTE"*. A create is the
//! only operation where the row is not in the database, so there is nothing to
//! select and the row has to be synthesised:
//!
//! ```sql
//! WITH "posts" AS (SELECT $2::bigint AS "author_id", NULL AS "status")
//! SELECT 1 FROM "posts" WHERE ("posts"."author_id" = $1) LIMIT 1
//! ```
//!
//! The CTE is named after the table, so it shadows it, and the condition
//! compiled by `brocanth-policy` is spliced in **unchanged**. That matters:
//! the text the database evaluates for a create is the same text it evaluates
//! for a list, which is PLAN.md §9 obligation 1 — the two interpretations
//! cannot disagree when there is only one of them.
//!
//! Every column of the entity is selected, not only the ones the request set,
//! because a condition may name a column the request left out and
//! `NULL` is what that column will hold. An author who decided what absence
//! means wrote `or_deny`, `or_allow` or `is_null` in the policy, and this is
//! where that decision is applied.
//!
//! # Bind order, which is the whole difficulty
//!
//! The condition is compiled on its own and numbers its placeholders from one.
//! It cannot be renumbered without rewriting its text, so instead the two
//! dialects get the two orders that need no rewriting:
//!
//! * **PostgreSQL** numbers explicitly, so the CTE continues the numbering
//!   from where the condition stopped and the binds are
//!   `condition ++ row`. Text order does not matter to `$n`.
//! * **SQLite** numbers by position in the text, and the CTE comes first, so
//!   the binds are `row ++ condition`.
//!
//! `tests/create.rs` pins both, and then runs the statement against a real
//! engine.
//!
//! # The cast, and the one case it cannot cover
//!
//! A PostgreSQL parameter in a `SELECT` list has no type to infer from, so
//! each bind is written `$n::bigint` and so on. A `NULL` has no type to cast
//! to, so it is written as the literal `NULL` — safe, because a null is a
//! structural absence rather than user data, and no value ever reaches the
//! text here. The residual edge is a column that is null in the proposed row
//! and compared against a non-text bind: PostgreSQL resolves the unknown
//! literal to `text` and may refuse the comparison. Set the column, or use
//! [`CreateCheck::InMemory`](crate::CreateCheck).

use brocanth_policy::{Column, CompileError, Condition, Dialect, Entity, Query, Value};

/// SQL text, its binds in order, and the dialect it was written for.
///
/// What [`ResourceData`](crate::ResourceData) is handed. A `Statement` is
/// never built from user data: the text comes from `brocanth-policy`'s
/// compiler or from [`create_admission`], and every value in it binds.
#[derive(Clone, Debug, PartialEq)]
pub struct Statement {
    sql: String,
    binds: Vec<Value>,
    dialect: Dialect,
}

impl Statement {
    /// The statement.
    pub fn sql(&self) -> &str {
        &self.sql
    }

    /// The values, in the order the placeholders appear.
    pub fn binds(&self) -> &[Value] {
        &self.binds
    }

    /// The dialect this was written for.
    pub fn dialect(&self) -> Dialect {
        self.dialect
    }
}

impl From<&Query> for Statement {
    fn from(query: &Query) -> Self {
        Statement {
            sql: query.sql().to_owned(),
            binds: query.binds().to_vec(),
            dialect: query.dialect(),
        }
    }
}

/// `SELECT 1` against a row that does not exist yet.
///
/// Returns the statement to run. One row means the policy admits the proposed
/// row; none means it does not. See the module documentation for the shape and
/// for the bind order.
///
/// # Errors
///
/// [`CompileError::DialectMismatch`] when the condition was compiled for
/// another dialect, and [`CompileError::BadIdentifier`] when a column name is
/// not a bare SQL identifier.
pub fn create_admission<E: Entity>(
    dialect: Dialect,
    condition: &Condition,
    row: &[(E::Column, Value)],
) -> Result<Statement, CompileError> {
    if condition.dialect() != dialect {
        return Err(CompileError::DialectMismatch {
            checked: condition.dialect(),
            writing: dialect,
        });
    }

    let mut cte = String::new();
    let mut values: Vec<Value> = Vec::with_capacity(E::Column::ALL.len());
    // PostgreSQL continues the condition's numbering; SQLite binds by
    // position and the CTE is written first, so it starts at one.
    let first = match dialect {
        Dialect::Postgres => condition.binds().len() + 1,
        Dialect::Sqlite => 1,
    };

    cte.push_str("WITH ");
    push_ident(&mut cte, E::TABLE.as_str());
    cte.push_str(" AS (SELECT ");
    for (index, column) in E::Column::ALL.iter().enumerate() {
        if index > 0 {
            cte.push_str(", ");
        }
        let value = row
            .iter()
            .find(|(c, _)| c == column)
            .map(|(_, v)| v.clone())
            .unwrap_or(Value::Null);
        if value.is_null() {
            cte.push_str("NULL");
        } else {
            cte.push_str(&dialect.placeholder(first + values.len()));
            if let Some(cast) = pg_cast(dialect, &value) {
                cte.push_str(cast);
            }
            values.push(value);
        }
        cte.push_str(" AS ");
        push_ident(&mut cte, column.ident()?.as_str());
    }
    cte.push_str(") SELECT 1 FROM ");
    push_ident(&mut cte, E::TABLE.as_str());
    cte.push_str(" WHERE (");
    cte.push_str(condition.sql());
    cte.push_str(") LIMIT 1");

    let binds = match dialect {
        Dialect::Postgres => {
            let mut binds = condition.binds().to_vec();
            binds.extend(values);
            binds
        }
        Dialect::Sqlite => {
            values.extend(condition.binds().iter().cloned());
            values
        }
    };

    Ok(Statement {
        sql: cte,
        binds,
        dialect,
    })
}

/// A PostgreSQL parameter in a `SELECT` list has nothing to infer a type
/// from, so it is told. SQLite has no static column types and needs none.
fn pg_cast(dialect: Dialect, value: &Value) -> Option<&'static str> {
    if dialect != Dialect::Postgres {
        return None;
    }
    Some(match value {
        Value::Null => return None,
        Value::Bool(_) => "::boolean",
        Value::Int(_) => "::bigint",
        Value::Float(_) => "::double precision",
        Value::Text(_) => "::text",
        Value::Bytes(_) => "::bytea",
    })
}

/// Identifiers reach the text already checked against
/// `[A-Za-z_][A-Za-z0-9_]*` by `brocanth_policy::Ident`, and are quoted here
/// the way that crate quotes them.
fn push_ident(out: &mut String, name: &str) {
    out.push('"');
    out.push_str(name);
    out.push('"');
}
