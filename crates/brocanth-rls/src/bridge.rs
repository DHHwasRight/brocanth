//! The bridge from `brocanth-policy`: a policy's [`Rules`] as database
//! policies (docs/policy-rls-bridge.md §1 and §3).
//!
//! [`SqlExpr::from_rule`] writes one rule with the translation table of §1:
//! each subject attribute is a typed setting read, each guard a boolean
//! over settings. [`Policy::from_rules`] writes the four commands of a
//! [`DbPolicy`], with their grants.
//!
//! A shape the database cannot enforce faithfully is refused with a `help:`
//! line, never written open or closed by default: `else allow` on an
//! attribute, `is` against an attribute, a guard on an attribute that is
//! not a `bool`, an attribute the subject does not declare, and a rule
//! marked app-only.

use brocanth_db::sea_query::prelude::{DateTime, Utc, Uuid};
use brocanth_db::sea_query::{Expr, Query, Value as SeaValue};
use brocanth_policy::{
    Attr, AttrKind, Column, DbPolicy, Entity, Exists, Guard, Missing, Op, Operand, Rule, Subject,
    Term, Value,
};

use crate::def::{Policy, Role, Table};
use crate::error::RlsError;
use crate::expr::SqlExpr;
use crate::schema::Schema;
use crate::snapshot::{Command, Privilege};

/// A shape the database cannot enforce, with what to change.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Refusal {
    message: String,
    help: String,
}

impl Refusal {
    fn new(message: impl Into<String>, help: impl Into<String>) -> Refusal {
        Refusal {
            message: message.into(),
            help: help.into(),
        }
    }

    fn into_error(self) -> RlsError {
        RlsError::definition(self.message, self.help)
    }
}

/// An expression with its constants folded: `TRUE` and `FALSE` drop out of
/// `AND` and `OR`, as `Pred::all` and `Pred::any` fold them in the app.
#[derive(Debug, Clone)]
enum Folded {
    True,
    False,
    Expr(SqlExpr),
}

impl Folded {
    fn all(parts: impl IntoIterator<Item = Folded>) -> Folded {
        let mut out: Option<SqlExpr> = None;
        for part in parts {
            match part {
                Folded::True => {}
                Folded::False => return Folded::False,
                Folded::Expr(e) => {
                    out = Some(match out {
                        None => e,
                        Some(acc) => acc.and(e),
                    })
                }
            }
        }
        out.map_or(Folded::True, Folded::Expr)
    }

    fn any(parts: impl IntoIterator<Item = Folded>) -> Folded {
        let mut out: Option<SqlExpr> = None;
        for part in parts {
            match part {
                Folded::False => {}
                Folded::True => return Folded::True,
                Folded::Expr(e) => {
                    out = Some(match out {
                        None => e,
                        Some(acc) => acc.or(e),
                    })
                }
            }
        }
        out.map_or(Folded::False, Folded::Expr)
    }

    fn not(self) -> Folded {
        match self {
            Folded::True => Folded::False,
            Folded::False => Folded::True,
            Folded::Expr(e) => Folded::Expr(e.not()),
        }
    }

    fn into_expr(self) -> SqlExpr {
        match self {
            Folded::True => SqlExpr::always(),
            Folded::False => SqlExpr::never(),
            Folded::Expr(e) => e,
        }
    }
}

/// What a translation reads: the policy's table, for a column an `exists`
/// subquery names from outside, and the subject's attributes.
struct Ctx<'a> {
    table: &'a str,
    attrs: &'a [Attr],
}

impl Ctx<'_> {
    fn attr(&self, key: &str) -> Result<&Attr, Refusal> {
        self.attrs.iter().find(|a| a.key == key).ok_or_else(|| {
            Refusal::new(
                format!("the rule reads `who.{key}`, which the subject does not declare"),
                format!("add #[attr] to the subject's field `{key}`"),
            )
        })
    }

    /// `s(key)`, typed by the attribute's kind, and whether the subject can
    /// lack it.
    fn setting(&self, key: &str) -> Result<(SqlExpr, bool), Refusal> {
        let attr = self.attr(key)?;
        Ok((crate::identity::setting_of(attr).get(), attr.optional))
    }

    fn rule<C: Column>(&self, rule: &Rule<C>) -> Result<Folded, Refusal> {
        if rule.arms.is_empty() {
            return self.term(&rule.fallback);
        }
        let guards = rule
            .arms
            .iter()
            .map(|(g, _)| self.guard(g))
            .collect::<Result<Vec<_>, _>>()?;
        let terms = rule
            .arms
            .iter()
            .map(|(_, t)| t)
            .chain([&rule.fallback])
            .map(|t| self.term(t))
            .collect::<Result<Vec<_>, _>>()?;
        // `(NOT g1 AND … AND g_i AND t_i)` for each arm, then the fallback
        // with every guard negated. A term that is `FALSE` drops out.
        let mut disjuncts = Vec::new();
        for (i, term) in terms.into_iter().enumerate() {
            if matches!(term, Folded::False) {
                continue;
            }
            let mut conj: Vec<Folded> = guards[..i].iter().cloned().map(Folded::not).collect();
            if let Some(g) = guards.get(i) {
                conj.push(g.clone());
            }
            conj.push(term);
            disjuncts.push(Folded::all(conj));
        }
        let body = Folded::any(disjuncts);
        if matches!(body, Folded::False) {
            return Ok(Folded::False);
        }
        // A NULL guard setting makes the rule FALSE, as an absent guard
        // attribute makes it `Pred::Never` in `Rule::resolve`.
        let mut prefix = Vec::new();
        for key in rule.guard_keys() {
            prefix.push(Folded::Expr(self.setting(key)?.0.is_not_null()));
        }
        prefix.push(body);
        Ok(Folded::all(prefix))
    }

    fn guard(&self, guard: &Guard) -> Result<Folded, Refusal> {
        Ok(match guard {
            Guard::Attr(key) => {
                if self.attr(key)?.kind != AttrKind::Boolean {
                    return Err(Refusal::new(
                        format!("the guard `who.{key}` reads an attribute that is not a bool"),
                        format!("compare it with a literal: who.{key} == <value>"),
                    ));
                }
                Folded::Expr(self.setting(key)?.0)
            }
            Guard::Eq(key, value) => {
                Folded::Expr(self.setting(key)?.0.eq(SqlExpr::val(literal(value)?)))
            }
            Guard::Not(inner) => self.guard(inner)?.not(),
            Guard::All(parts) => Folded::all(
                parts
                    .iter()
                    .map(|g| self.guard(g))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Guard::Any(parts) => Folded::any(
                parts
                    .iter()
                    .map(|g| self.guard(g))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
        })
    }

    fn term<C: Column>(&self, term: &Term<C>) -> Result<Folded, Refusal> {
        Ok(match term {
            Term::Always => Folded::True,
            Term::Never => Folded::False,
            Term::Cmp {
                column,
                op,
                operand,
            } => {
                let (present, cmp) =
                    self.compare(SqlExpr::col_named(column.name()), *op, operand)?;
                Folded::Expr(match present {
                    Some(p) => p.and(cmp),
                    None => cmp,
                })
            }
            Term::In {
                column,
                values,
                negated,
            } => {
                if values.is_empty() {
                    // `Pred::one_of([])` is `Never`, `Pred::none_of([])`
                    // is `Always`.
                    return Ok(if *negated {
                        Folded::True
                    } else {
                        Folded::False
                    });
                }
                let values = values.iter().map(literal).collect::<Result<Vec<_>, _>>()?;
                let col = SqlExpr::col_named(column.name());
                Folded::Expr(if *negated {
                    col.is_not_in(values)
                } else {
                    col.is_in(values)
                })
            }
            Term::All(parts) => Folded::all(
                parts
                    .iter()
                    .map(|t| self.term(t))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Term::Any(parts) => Folded::any(
                parts
                    .iter()
                    .map(|t| self.term(t))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            // `policy!` writes a negated membership as one term, whose absent
            // `else deny` value is FALSE. `NOT` around it would make it TRUE.
            Term::Not(inner) if matches!(**inner, Term::Exists(_)) => return Err(Refusal::new(
                "`!` around an `exists` term: an absent `else deny` value would admit every row",
                "negate the membership itself: Exists { negated: true, .. }",
            )),
            Term::Not(inner) => self.term(inner)?.not(),
            Term::Exists(exists) => self.exists(exists)?,
        })
    }

    /// `EXISTS (SELECT 1 FROM "t" WHERE "t"."foreign" = "table"."local" AND
    /// …)`. An `else deny` filter value is tested outside the subquery, so
    /// an absent value makes the whole term FALSE, negated or not, as
    /// `Exists::resolve` makes it `Pred::Never`.
    fn exists<C: Column>(&self, exists: &Exists<C>) -> Result<Folded, Refusal> {
        let link = exists.table.as_str();
        let mut cond = SqlExpr::table_col_named(link, exists.foreign.as_str())
            .eq(SqlExpr::table_col_named(self.table, exists.local.name()));
        let mut present = Vec::new();
        for (column, op, operand) in &exists.filter {
            let (p, cmp) = self.compare(
                SqlExpr::table_col_named(link, column.as_str()),
                *op,
                operand,
            )?;
            present.extend(p.map(Folded::Expr));
            cond = cond.and(cmp);
        }
        let select = Query::select()
            .expr(Expr::val(1))
            .from(link.to_string())
            .and_where(cond.sea())
            .to_owned();
        let mut e = SqlExpr::exists(select).uses(&cond);
        if exists.negated {
            e = e.not();
        }
        present.push(Folded::Expr(e));
        Ok(Folded::all(present))
    }

    /// `col op operand`, and for an `else deny` (or non-`Option`)
    /// attribute the test `s IS NOT NULL` that goes before it.
    fn compare(
        &self,
        col: SqlExpr,
        op: Op,
        operand: &Operand,
    ) -> Result<(Option<SqlExpr>, SqlExpr), Refusal> {
        match operand {
            Operand::Value(value) => {
                let cmp = match (op, value.is_null()) {
                    (Op::Is, true) => col.is_null(),
                    (Op::IsNot, true) => col.is_not_null(),
                    _ => apply(col, op, SqlExpr::val(literal(value)?)),
                };
                Ok((None, cmp))
            }
            Operand::Attr { key, missing } => {
                if matches!(op, Op::Is | Op::IsNot) {
                    return Err(Refusal::new(
                        format!("`is` compares a column with `who.{key}`"),
                        "use == with else deny; IS NOT DISTINCT FROM matches NULL to a missing setting",
                    ));
                }
                if *missing == Missing::Allow {
                    return Err(Refusal::new(
                        format!(
                            "`who.{key} else allow` admits every row when the setting is missing"
                        ),
                        "put the fact in a bool #[attr] field and guard on it",
                    ));
                }
                let (s, optional) = self.setting(key)?;
                let cmp = apply(col, op, s.clone());
                // `else deny`, or a field that is always there: a missing
                // setting is FALSE, not NULL, so `NOT` of it is TRUE as in
                // the app. `else null` keeps three-valued logic.
                if *missing == Missing::Deny || !optional {
                    Ok((Some(s.is_not_null()), cmp))
                } else {
                    Ok((None, cmp))
                }
            }
        }
    }
}

fn apply(col: SqlExpr, op: Op, other: SqlExpr) -> SqlExpr {
    match op {
        Op::Eq => col.eq(other),
        Op::Ne => col.ne(other),
        Op::Lt => col.lt(other),
        Op::Le => col.le(other),
        Op::Gt => col.gt(other),
        Op::Ge => col.ge(other),
        Op::Is => col.is(other),
        Op::IsNot => col.is_not(other),
    }
}

/// A rule's literal as a sea-query value, which sea-query's Postgres writer
/// escapes.
fn literal(value: &Value) -> Result<SeaValue, Refusal> {
    Ok(match value {
        Value::Null => SeaValue::String(None),
        Value::Bool(b) => SeaValue::Bool(Some(*b)),
        Value::Int(i) => SeaValue::BigInt(Some(*i)),
        Value::Float(f) => SeaValue::Double(Some(*f)),
        Value::Text(t) => SeaValue::String(Some(t.clone())),
        Value::Bytes(b) => SeaValue::Bytes(Some(b.clone())),
        Value::Uuid(b) => SeaValue::Uuid(Some(Uuid::from_bytes(*b))),
        Value::Timestamptz(us) => match DateTime::<Utc>::from_timestamp_micros(*us) {
            Some(t) => SeaValue::ChronoDateTimeUtc(Some(t)),
            None => {
                return Err(Refusal::new(
                    format!("the timestamp {us} µs is out of range"),
                    "write a time between the years -262143 and 262142",
                ))
            }
        },
    })
}

impl SqlExpr {
    /// One rule of a [`DbPolicy`] as a database expression, by the
    /// translation table of docs/policy-rls-bridge.md §1.
    ///
    /// `table` is the policy's table: an `exists` subquery names its
    /// columns as `"table"."column"`. `attrs` is the subject's
    /// [`Subject::ATTRS`]: each attribute the rule reads becomes a
    /// [`Setting`](crate::Setting) of its kind. A rule that admits no row
    /// is `FALSE`.
    ///
    /// A guard chain `g1 => r1, g2 => r2, f` becomes
    /// `s(g1) IS NOT NULL AND s(g2) IS NOT NULL AND ((g1 AND r1) OR (NOT g1
    /// AND g2 AND r2) OR (NOT g1 AND NOT g2 AND f))`, an arm whose term is
    /// `never` left out.
    ///
    /// # Errors
    /// [`RlsError::Definition`], with a `help:` line, for a rule the
    /// database cannot enforce as the application does: `else allow` on an
    /// attribute, `is` against an attribute, a guard on an attribute that
    /// is not a `bool`, an attribute `attrs` lacks, or an app-only rule.
    pub fn from_rule<C: Column>(
        rule: &Rule<C>,
        table: &Table,
        attrs: &[Attr],
    ) -> Result<SqlExpr, RlsError> {
        if let Some(reason) = rule.app_only {
            return Err(Refusal::new(
                format!("the rule on {} is app-only ({reason})", table.name),
                "write its SqlExpr by hand",
            )
            .into_error());
        }
        let ctx = Ctx {
            table: &table.name,
            attrs,
        };
        ctx.rule(rule)
            .map(Folded::into_expr)
            .map_err(Refusal::into_error)
    }
}

/// One command's database rule, as [`Policy::from_rules`] found it.
// Four per policy, built once when the schema is: the size does not matter.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
enum Slot {
    /// The rule admits no row: no policy, no grant.
    Never,
    /// `USING` and `WITH CHECK`, as the command takes them.
    Rule {
        using: Option<SqlExpr>,
        check: Option<SqlExpr>,
    },
    /// The database cannot enforce it; `.manual(…)` can replace it.
    Refused(Refusal),
}

/// The database policies of one [`DbPolicy`] for one role: what
/// [`Policy::from_rules`] returns. Give it to [`Schema::policies`], which
/// adds the table, its grants and the policies.
///
/// A command whose rule the database cannot enforce (an app-only rule, or
/// a shape decision 3 refuses) fails [`Schema::snapshot`], and so
/// `rls make`, until [`RulePolicies::manual`] gives it an expression.
#[derive(Debug, Clone)]
pub struct RulePolicies {
    table: Table,
    role: Role,
    slots: [(Command, Slot); 4],
    problems: Vec<Refusal>,
}

impl Policy {
    /// The database policies of `P`'s [`DbPolicy::rules`], for `role`
    /// (docs/policy-rls-bridge.md §3).
    ///
    /// Up to four policies, named `<table>_<role>_<command>` like
    /// [`crate::crud`]:
    ///
    /// | Rule | Policy |
    /// |---|---|
    /// | `list` OR `view` | `FOR SELECT USING (list OR view)`; `list` alone when there is no `view` |
    /// | `create` | `FOR INSERT WITH CHECK (create)` |
    /// | `update` | `FOR UPDATE USING (update) WITH CHECK (update_check)`; `update_check` defaults to `update` |
    /// | `delete` | `FOR DELETE USING (delete)` |
    ///
    /// A rule that admits no row writes no policy and no grant: Postgres
    /// refuses the command with `permission denied`. The table is
    /// `Table::named(<entity's table>)`, forced, and `role` is granted each
    /// command that has a policy. With [`RulePolicies::sequence`], the role
    /// also gets `USAGE` on the table's sequence when it may insert.
    pub fn from_rules<P: DbPolicy>(role: &Role) -> RulePolicies {
        let rules = P::rules();
        let table = Table::named(<P::Entity as Entity>::TABLE.as_str());
        let ctx = Ctx {
            table: &table.name,
            attrs: <P::Subject as Subject>::ATTRS,
        };
        let name = &table.name;
        let select = command_slot(name, Command::Select, &[&rules.list, rules.view()], || {
            let list = ctx.rule(&rules.list)?;
            let read = match &rules.view {
                None => list,
                Some(view) => Folded::any([list, ctx.rule(view)?]),
            };
            Ok((Some(read), None))
        });
        let insert = command_slot(name, Command::Insert, &[&rules.create], || {
            Ok((None, Some(ctx.rule(&rules.create)?)))
        });
        let update = command_slot(
            name,
            Command::Update,
            &[&rules.update, rules.update_check()],
            || match ctx.rule(&rules.update)? {
                Folded::False => Ok((Some(Folded::False), None)),
                using => Ok((Some(using), Some(ctx.rule(rules.update_check())?))),
            },
        );
        let delete = command_slot(name, Command::Delete, &[&rules.delete], || {
            Ok((Some(ctx.rule(&rules.delete)?), None))
        });
        RulePolicies {
            slots: [
                (Command::Select, select),
                (Command::Insert, insert),
                (Command::Update, update),
                (Command::Delete, delete),
            ],
            table: table.clone(),
            role: role.clone(),
            problems: Vec::new(),
        }
    }
}

/// One command's slot: refused when a rule it reads is app-only or cannot
/// be translated, [`Slot::Never`] when its first clause is `FALSE`.
fn command_slot<C: Column>(
    table: &str,
    command: Command,
    rules: &[&Rule<C>],
    translate: impl FnOnce() -> Result<(Option<Folded>, Option<Folded>), Refusal>,
) -> Slot {
    if let Some(reason) = rules.iter().find_map(|r| r.app_only) {
        return Slot::Refused(Refusal::new(
            format!("the rule for {table} {command} is app-only ({reason}), so the database has no rule for it"),
            format!("write a SqlExpr for {table} {command} in src/plugins/rls/mod.rs with .manual(...)"),
        ));
    }
    match translate() {
        Err(r) => Slot::Refused(rule_refusal(table, command, r)),
        Ok((using, check)) => {
            if matches!(using.as_ref().or(check.as_ref()), Some(Folded::False)) {
                return Slot::Never;
            }
            Slot::Rule {
                using: using.map(Folded::into_expr),
                check: check.map(Folded::into_expr),
            }
        }
    }
}

fn rule_refusal(table: &str, command: Command, r: Refusal) -> Refusal {
    Refusal::new(
        format!("{table} {command}: {}", r.message),
        format!(
            "{}; or write a SqlExpr for {table} {command} in src/plugins/rls/mod.rs with .manual(...)",
            r.help
        ),
    )
}

impl RulePolicies {
    /// Replaces one command's rule with a hand-written expression: `USING`
    /// for `SELECT` and `DELETE`, `WITH CHECK` for `INSERT`, both for
    /// `UPDATE`. The way to give an app-only rule its database floor
    /// (bridge decision 4).
    pub fn manual(mut self, command: Command, expr: SqlExpr) -> RulePolicies {
        let (using, check) = match command {
            Command::Select | Command::Delete => (Some(expr), None),
            Command::Insert => (None, Some(expr)),
            Command::Update => (Some(expr.clone()), Some(expr)),
            Command::All => {
                self.problems.push(Refusal::new(
                    format!("`.manual(Command::All, …)` on {}", self.table.name),
                    "name one command: Select, Insert, Update or Delete",
                ));
                return self;
            }
        };
        for (c, slot) in &mut self.slots {
            if *c == command {
                *slot = Slot::Rule { using, check };
                break;
            }
        }
        self
    }

    /// The table's `serial` sequence ([`Table::sequence`]): the role gets
    /// `USAGE` on it when it may insert.
    pub fn sequence(mut self, name: impl Into<String>) -> RulePolicies {
        self.table = self.table.sequence(name);
        self
    }

    /// The table, with its grants, and the policies.
    ///
    /// # Errors
    /// [`RlsError::Definition`] for the first command the database cannot
    /// enforce and no [`RulePolicies::manual`] covers.
    pub fn build(self) -> Result<(Table, Vec<Policy>), RlsError> {
        if let Some(problem) = self.problems.into_iter().next() {
            return Err(problem.into_error());
        }
        let base = format!("{}_{}", self.table.name, self.role.name);
        let mut privileges = Vec::new();
        let mut policies = Vec::new();
        for (command, slot) in self.slots {
            let (using, check) = match slot {
                Slot::Never => continue,
                Slot::Refused(r) => return Err(r.into_error()),
                Slot::Rule { using, check } => (using, check),
            };
            let (suffix, privilege) = match command {
                Command::Select => ("select", Privilege::Select),
                Command::Insert => ("insert", Privilege::Insert),
                Command::Update => ("update", Privilege::Update),
                Command::Delete => ("delete", Privilege::Delete),
                Command::All => continue,
            };
            let mut policy = Policy::new(format!("{base}_{suffix}"), &self.table)
                .command(command)
                .to(&self.role);
            policy.using = using;
            policy.with_check = check;
            privileges.push(privilege);
            policies.push(policy);
        }
        let mut table = self.table.force();
        if !privileges.is_empty() {
            table = table.grant(&self.role, privileges);
        }
        Ok((table, policies))
    }
}

/// What [`Schema::policies`] takes: policies, such as [`crate::crud`]
/// returns, or the [`RulePolicies`] of [`Policy::from_rules`].
pub trait IntoPolicies {
    /// Adds them, and what they need, to `schema`.
    fn add_to(self, schema: Schema) -> Schema;
}

impl<I: IntoIterator<Item = Policy>> IntoPolicies for I {
    fn add_to(self, schema: Schema) -> Schema {
        schema.extend_policies(self)
    }
}

impl IntoPolicies for RulePolicies {
    fn add_to(self, schema: Schema) -> Schema {
        match self.build() {
            Ok((table, policies)) => schema.table(table).extend_policies(policies),
            Err(e) => schema.refuse(e),
        }
    }
}
