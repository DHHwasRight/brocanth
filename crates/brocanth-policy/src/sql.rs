//! The first interpretation: SQL text and an ordered bind list, and nothing
//! else.
//!
//! # What this crate hands back, and what it refuses to be
//!
//! A [`Condition`] is a `WHERE` fragment plus a `Vec<Value>`. It is not a
//! `sea_orm::Condition`, and this crate does not depend on SeaORM, on `sqlx`,
//! or on a driver. PLAN.md §8: the arrow points from plugin to core and a
//! plugin that needs another depends on it in the open. An algebra over column
//! names has no reason to know what an ORM is, and taking the dependency here
//! would put a SQL driver in the tree of every application that writes a
//! policy. The adapter that turns a [`Condition`] into a `sea_orm::Condition`
//! is a later, separate crate that depends on both, and it is the only thing
//! in the tree that has to.
//!
//! It also keeps `brocanth-jobs`' second dialect honest. That crate already
//! supports PostgreSQL and SQLite behind features, so the workspace is not
//! single-dialect and this crate must not assume it is.
//!
//! # SQL injection, structurally
//!
//! Two rules, and together they are the whole defence:
//!
//! 1. **A value is never SQL text.** [`Value`] reaches a statement as a bind
//!    parameter or it does not reach it. There is no code path from a `Value`
//!    to the `String` being built — `Builder::bind` pushes a placeholder and
//!    appends to the bind list, and nothing else writes a value.
//!    `tests/injection.rs` throws the usual payloads at every predicate shape
//!    and asserts the generated SQL contains none of them.
//! 2. **An identifier is `&'static str`, and checked anyway.** See
//!    [`crate::Ident`].
//!
//! The one escape hatch, [`crate::Opaque`], obeys both: its SQL is
//! `&'static str`, it may contain no quote at all, and its values go through
//! the same bind list.
//!
//! # Dialect: PostgreSQL is the base, SQLite is the smaller setup
//!
//! Brocanth stores its queues, cache and blobs in PostgreSQL, so PostgreSQL is
//! what this compiler is written for and [`Dialect::Postgres`] is
//! [`Dialect::default`]. SQLite stays reachable, because a smaller deployment
//! is a supported target, but it is the fallback rather than the shape the
//! design follows.
//!
//! [`Dialect`] is still an **argument** at every entry point, never taken from
//! the default. A default that the compiler could reach for is a dialect a
//! reviewer cannot see; a default you have to name is documentation. Four
//! things differ, and all four are here rather than assumed:
//!
//! | | PostgreSQL (base) | SQLite (smaller setups) |
//! |---|---|---|
//! | placeholder | `$1`, `$2`, … | `?` |
//! | null-safe `=` | `IS NOT DISTINCT FROM` | `IS` |
//! | null-safe `<>` | `IS DISTINCT FROM` | `IS NOT` |
//! | constants | `TRUE`, `FALSE` | `1 = 1`, `1 = 0` |
//!
//! `IS DISTINCT FROM` is the one worth naming. PLAN.md §9's whole subject is
//! what `!=` means when a side is absent, and PocketBase answers it with
//! `COALESCE` and grants access on a typo. PostgreSQL has the operator the
//! question deserves: `IS DISTINCT FROM` is two-valued, never answers unknown,
//! and treats `NULL` as a value. It is what [`Op::IsNot`] compiles to, and
//! SQLite's `IS NOT` means exactly the same thing, so the author's decision
//! survives the smaller setup unchanged. [`Op::Ne`] remains three-valued in
//! both, because an author who writes `<>` asked for SQL's `<>`.
//!
//! **Numbered placeholders make bind order load-bearing**, which `?` lets you
//! ignore. A [`Condition`] compiled on its own starts at `$1`; spliced after
//! an id bind it has to start at `$2`. That renumbering is in `splice`, and
//! `tests/snapshots.rs` asserts it, because it is the bug SQLite would have
//! hidden until the day somebody moved.
//!
//! A third dialect is a variant and four match arms.
//!
//! # The one assumption a [`Condition`] makes about its host query
//!
//! Columns are qualified with the entity's table name: `"posts"."author_id"`.
//! A correlated `EXISTS` needs that to refer to the outer row unambiguously.
//! So a condition must be spliced into a query where the table appears under
//! its own name. **Alias the table and the condition is wrong**, which is
//! stated here rather than discovered at three in the morning.

use std::fmt::Write as _;

use crate::entity::{Column, Entity, Ident};
use crate::errors::CompileError;
use crate::pred::{Membership, Op, Opaque, Pred};
use crate::value::Value;

/// Which database the SQL is for.
///
/// `Postgres` is [`Default`] because brocanth's base deployment is PostgreSQL
/// — queues, cache and blobs all live there. Every compile entry point still
/// takes a `Dialect` argument explicitly, so the default is a statement about
/// which dialect gets the care rather than one the compiler reaches for behind
/// a call site.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Dialect {
    /// PostgreSQL. The base: numbered placeholders, real booleans, and
    /// `IS DISTINCT FROM` for the null-safe comparisons PLAN.md §9 is about.
    #[default]
    Postgres,
    /// SQLite. The smaller-setup target, kept reachable and compiled with the
    /// same meaning, not the same text.
    Sqlite,
}

impl Dialect {
    /// How this dialect spells the `n`th placeholder, counting from one.
    pub fn placeholder(self, index: usize) -> String {
        match self {
            Dialect::Sqlite => "?".to_owned(),
            Dialect::Postgres => format!("${index}"),
        }
    }

    fn op(self, op: Op) -> &'static str {
        match (self, op) {
            (_, Op::Eq) => "=",
            (_, Op::Ne) => "<>",
            (_, Op::Lt) => "<",
            (_, Op::Le) => "<=",
            (_, Op::Gt) => ">",
            (_, Op::Ge) => ">=",
            (Dialect::Sqlite, Op::Is) => "IS",
            (Dialect::Sqlite, Op::IsNot) => "IS NOT",
            (Dialect::Postgres, Op::Is) => "IS NOT DISTINCT FROM",
            (Dialect::Postgres, Op::IsNot) => "IS DISTINCT FROM",
        }
    }
}

/// A `WHERE` fragment and its binds, in declaration order.
///
/// This is what PLAN.md §9 promises `brocanth-admin`'s `ListRequest` will
/// carry. That struct is `Copy`, so it carries an `Option<&Condition>` rather
/// than the value — one field, one borrow, and the admin keeps its pagination
/// because the predicate is in the `WHERE` clause where §9 says it belongs.
#[derive(Clone, Debug, PartialEq)]
pub struct Condition {
    sql: String,
    binds: Vec<Value>,
    dialect: Dialect,
}

impl Condition {
    /// The fragment, with no `WHERE` keyword and no outer parentheses beyond
    /// the ones the predicate needed.
    pub fn sql(&self) -> &str {
        &self.sql
    }

    /// The values, in the order the placeholders appear.
    pub fn binds(&self) -> &[Value] {
        &self.binds
    }

    /// The dialect this was compiled for.
    ///
    /// Carried so that splicing a condition into a statement built for another
    /// dialect is a visible mistake rather than a silent one: `$1` in SQLite
    /// is a syntax error, and `?` in PostgreSQL is worse.
    pub fn dialect(&self) -> Dialect {
        self.dialect
    }

    /// The parts, consumed.
    pub fn into_parts(self) -> (String, Vec<Value>) {
        (self.sql, self.binds)
    }
}

/// A complete statement and its binds.
#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    sql: String,
    binds: Vec<Value>,
    dialect: Dialect,
}

impl Query {
    pub(crate) fn new(sql: String, binds: Vec<Value>, dialect: Dialect) -> Self {
        Query {
            sql,
            binds,
            dialect,
        }
    }

    /// The statement.
    pub fn sql(&self) -> &str {
        &self.sql
    }

    /// The values, in the order the placeholders appear.
    pub fn binds(&self) -> &[Value] {
        &self.binds
    }

    /// The dialect this was compiled for.
    pub fn dialect(&self) -> Dialect {
        self.dialect
    }
}

/// The only thing that writes SQL text.
pub(crate) struct Builder {
    dialect: Dialect,
    sql: String,
    binds: Vec<Value>,
}

impl Builder {
    pub(crate) fn new(dialect: Dialect) -> Self {
        Builder {
            dialect,
            sql: String::new(),
            binds: Vec::new(),
        }
    }

    pub(crate) fn dialect(&self) -> Dialect {
        self.dialect
    }

    pub(crate) fn push(&mut self, text: &str) {
        self.sql.push_str(text);
    }

    /// The only path from a [`Value`] into a statement, and it does not go
    /// through the text.
    pub(crate) fn bind(&mut self, value: Value) {
        self.binds.push(value);
        let placeholder = self.dialect.placeholder(self.binds.len());
        self.sql.push_str(&placeholder);
    }

    pub(crate) fn ident(&mut self, ident: Ident) {
        self.sql.push('"');
        self.sql.push_str(ident.as_str());
        self.sql.push('"');
    }

    pub(crate) fn qualified(&mut self, table: Ident, column: Ident) {
        self.ident(table);
        self.sql.push('.');
        self.ident(column);
    }

    pub(crate) fn into_query(self) -> Query {
        Query::new(self.sql, self.binds, self.dialect)
    }

    fn into_condition(self) -> Condition {
        Condition {
            sql: self.sql,
            binds: self.binds,
            dialect: self.dialect,
        }
    }
}

impl<C: Column> Pred<C> {
    /// Compile to a `WHERE` fragment for the entity's table.
    ///
    /// This is the interpretation PLAN.md §9 makes the default, because it is
    /// the one that cannot disagree with itself: the list query, the by-id
    /// guard, the admission test and the realtime re-check are all the same
    /// text evaluated by the same engine.
    pub fn condition<E: Entity<Column = C>>(
        &self,
        dialect: Dialect,
    ) -> Result<Condition, CompileError> {
        let mut builder = Builder::new(dialect);
        write_pred::<E, C>(&mut builder, self)?;
        Ok(builder.into_condition())
    }

    /// `SELECT <pk> FROM <table> WHERE <condition>`.
    ///
    /// The list interpretation, and the query [`crate::agreement`] runs to ask
    /// the database which rows the predicate admits.
    pub fn matching_ids<E: Entity<Column = C>>(
        &self,
        dialect: Dialect,
    ) -> Result<Query, CompileError> {
        let mut builder = Builder::new(dialect);
        builder.push("SELECT ");
        builder.qualified(E::TABLE, E::PRIMARY_KEY.ident()?);
        builder.push(" FROM ");
        builder.ident(E::TABLE);
        builder.push(" WHERE ");
        write_pred::<E, C>(&mut builder, self)?;
        Ok(builder.into_query())
    }
}

pub(crate) fn write_pred<E: Entity<Column = C>, C: Column>(
    builder: &mut Builder,
    pred: &Pred<C>,
) -> Result<(), CompileError> {
    match pred {
        // PostgreSQL has real booleans and gets them. SQLite has no boolean
        // literal, so it gets the numeric form, which means the same thing.
        Pred::Always => builder.push(match builder.dialect() {
            Dialect::Postgres => "TRUE",
            Dialect::Sqlite => "1 = 1",
        }),
        Pred::Never => builder.push(match builder.dialect() {
            Dialect::Postgres => "FALSE",
            Dialect::Sqlite => "1 = 0",
        }),
        Pred::Cmp { column, op, value } => {
            builder.qualified(E::TABLE, column.ident()?);
            // `IS NULL` rather than `IS ?` bound to NULL. Same meaning in both
            // dialects, no untyped parameter for PostgreSQL to puzzle over,
            // and a snapshot a reviewer recognises.
            if value.is_null() && op.is_null_safe() {
                builder.push(if *op == Op::Is {
                    " IS NULL"
                } else {
                    " IS NOT NULL"
                });
            } else {
                builder.push(" ");
                builder.push(builder.dialect().op(*op));
                builder.push(" ");
                builder.bind(value.clone());
            }
        }
        Pred::In {
            column,
            values,
            negated,
        } => {
            builder.qualified(E::TABLE, column.ident()?);
            builder.push(if *negated { " NOT IN (" } else { " IN (" });
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    builder.push(", ");
                }
                builder.bind(value.clone());
            }
            builder.push(")");
        }
        Pred::All(parts) => write_join::<E, C>(builder, parts, " AND ", &Pred::Always)?,
        Pred::Any(parts) => write_join::<E, C>(builder, parts, " OR ", &Pred::Never)?,
        Pred::Not(inner) => {
            builder.push("NOT (");
            write_pred::<E, C>(builder, inner)?;
            builder.push(")");
        }
        Pred::Exists(membership) => write_exists::<E, C>(builder, membership)?,
        Pred::Opaque(fragment) => write_opaque(builder, fragment)?,
    }
    Ok(())
}

fn write_join<E: Entity<Column = C>, C: Column>(
    builder: &mut Builder,
    parts: &[Pred<C>],
    separator: &str,
    empty: &Pred<C>,
) -> Result<(), CompileError> {
    if parts.is_empty() {
        // The empty conjunction is true and the empty disjunction is false,
        // written in whichever dialect's spelling of those the caller asked
        // for.
        return write_pred::<E, C>(builder, empty);
    }
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            builder.push(separator);
        }
        builder.push("(");
        write_pred::<E, C>(builder, part)?;
        builder.push(")");
    }
    Ok(())
}

fn write_exists<E: Entity<Column = C>, C: Column>(
    builder: &mut Builder,
    membership: &Membership<C>,
) -> Result<(), CompileError> {
    builder.push(if membership.negated {
        "NOT EXISTS (SELECT 1 FROM "
    } else {
        "EXISTS (SELECT 1 FROM "
    });
    builder.ident(membership.table);
    builder.push(" WHERE ");
    builder.qualified(membership.table, membership.foreign);
    builder.push(" = ");
    builder.qualified(E::TABLE, membership.local.ident()?);
    for (column, op, value) in &membership.filter {
        builder.push(" AND ");
        builder.qualified(membership.table, *column);
        if value.is_null() && op.is_null_safe() {
            builder.push(if *op == Op::Is {
                " IS NULL"
            } else {
                " IS NOT NULL"
            });
        } else {
            builder.push(" ");
            builder.push(builder.dialect().op(*op));
            builder.push(" ");
            builder.bind(value.clone());
        }
    }
    builder.push(")");
    Ok(())
}

fn write_opaque<C: Column>(
    builder: &mut Builder,
    fragment: &Opaque<C>,
) -> Result<(), CompileError> {
    if fragment.sql.contains('\'') || fragment.sql.contains('"') {
        return Err(CompileError::LiteralInFragment { sql: fragment.sql });
    }
    if fragment.sql.contains('$') {
        return Err(CompileError::PlaceholderSyntax { sql: fragment.sql });
    }
    let placeholders = fragment.sql.matches('?').count();
    if placeholders != fragment.binds.len() {
        return Err(CompileError::PlaceholderMismatch {
            sql: fragment.sql,
            placeholders,
            binds: fragment.binds.len(),
        });
    }
    builder.push("(");
    let mut values = fragment.binds.iter();
    for (index, chunk) in fragment.sql.split('?').enumerate() {
        if index > 0 {
            // `values` has exactly as many items as there are separators; the
            // count was checked above.
            if let Some(value) = values.next() {
                builder.bind(value.clone());
            }
        }
        builder.push(chunk);
    }
    builder.push(")");
    Ok(())
}

/// `UPDATE <table> SET … WHERE <pk> = ? AND (<condition>)`.
///
/// Used only by [`crate::Verified`], which is the point: PLAN.md §9
/// obligation 2 says the condition travels into the write, and there is no way
/// to build this statement without holding the witness.
pub(crate) fn write_update<E: Entity<Column = C>, C: Column>(
    dialect: Dialect,
    id: &Value,
    assignments: &[(C, Value)],
    condition: &Condition,
) -> Result<Query, CompileError> {
    if assignments.is_empty() {
        return Err(CompileError::EmptyUpdate);
    }
    let mut builder = Builder::new(dialect);
    builder.push("UPDATE ");
    builder.ident(E::TABLE);
    builder.push(" SET ");
    for (index, (column, value)) in assignments.iter().enumerate() {
        if index > 0 {
            builder.push(", ");
        }
        builder.ident(column.ident()?);
        builder.push(" = ");
        builder.bind(value.clone());
    }
    builder.push(" WHERE ");
    builder.qualified(E::TABLE, E::PRIMARY_KEY.ident()?);
    builder.push(" = ");
    builder.bind(id.clone());
    builder.push(" AND (");
    splice(&mut builder, condition);
    builder.push(")");
    Ok(builder.into_query())
}

/// `DELETE FROM <table> WHERE <pk> = ? AND (<condition>)`.
pub(crate) fn write_delete<E: Entity<Column = C>, C: Column>(
    dialect: Dialect,
    id: &Value,
    condition: &Condition,
) -> Result<Query, CompileError> {
    let mut builder = Builder::new(dialect);
    builder.push("DELETE FROM ");
    builder.ident(E::TABLE);
    builder.push(" WHERE ");
    builder.qualified(E::TABLE, E::PRIMARY_KEY.ident()?);
    builder.push(" = ");
    builder.bind(id.clone());
    builder.push(" AND (");
    splice(&mut builder, condition);
    builder.push(")");
    Ok(builder.into_query())
}

/// `SELECT 1 FROM <table> WHERE <pk> = ? AND (<condition>) LIMIT 1`.
///
/// Literally PocketBase's `CanAccessRecord`, and PLAN.md §9 obligation 1 asks
/// for exactly that as the default: one interpretation, so there is nothing
/// for a second one to disagree with.
pub(crate) fn write_admission<E: Entity<Column = C>, C: Column>(
    dialect: Dialect,
    id: &Value,
    condition: &Condition,
) -> Result<Query, CompileError> {
    let mut builder = Builder::new(dialect);
    builder.push("SELECT 1 FROM ");
    builder.ident(E::TABLE);
    builder.push(" WHERE ");
    builder.qualified(E::TABLE, E::PRIMARY_KEY.ident()?);
    builder.push(" = ");
    builder.bind(id.clone());
    builder.push(" AND (");
    splice(&mut builder, condition);
    builder.push(") LIMIT 1");
    Ok(builder.into_query())
}

/// Copy a compiled condition into a statement, renumbering its placeholders.
///
/// A `Condition` was compiled on its own, so a PostgreSQL one starts at `$1`.
/// Spliced after an id bind it has to start at `$2`, and this is where that
/// happens. SQLite's `?` needs no renumbering, which is why the bug would hide
/// there until the day somebody turned on PostgreSQL.
fn splice(builder: &mut Builder, condition: &Condition) {
    match builder.dialect() {
        Dialect::Sqlite => {
            builder.push(condition.sql());
            builder.binds.extend(condition.binds().iter().cloned());
        }
        Dialect::Postgres => {
            let offset = builder.binds.len();
            let mut rewritten = String::with_capacity(condition.sql().len());
            let mut rest = condition.sql();
            while let Some(at) = rest.find('$') {
                rewritten.push_str(&rest[..at]);
                let digits: String = rest[at + 1..]
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect();
                // `$` with no digits cannot occur: a fragment containing one is
                // rejected, and nothing else writes a `$`.
                let n: usize = digits.parse().unwrap_or(0);
                let _ = write!(rewritten, "${}", n + offset);
                rest = &rest[at + 1 + digits.len()..];
            }
            rewritten.push_str(rest);
            builder.push(&rewritten);
            builder.binds.extend(condition.binds().iter().cloned());
        }
    }
}
