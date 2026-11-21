//! The agreement test: the application's `condition()` and the database
//! policy of one `DbPolicy` admit the same rows, on a real Postgres.
//!
//! docs/policy-rls-bridge.md, decision 10. `policy!`'s `db { … }` clause
//! emits a `#[test]` that calls [`run`]. An application can call [`check`]
//! from its own tests too.
//!
//! # What one case does
//!
//! 1. Creates a schema of its own, the entity's table from its column types
//!    ([`TypedColumn::sql_type`]), each link table an `exists` reads, and
//!    the policies of [`Policy::from_rules`]: the SQL `rls make` writes.
//! 2. Inserts generated rows: the values of
//!    [`brocanth_policy::agreement::pool`], with `NULL`s and near-misses.
//! 3. For each subject, compares:
//!    - `SELECT`: the ids of `SELECT id … WHERE <condition(list OR view)>`
//!      as the URL's user, which must bypass row-level security (a
//!      superuser), against `SELECT id …` with no `WHERE` as the subject's
//!      [`identity()`](crate::identity()).
//!    - `INSERT`: for each candidate row, `create.eval` (the SQL condition
//!      when the rule has an `exists`) against the `INSERT` in a savepoint.
//!    - `UPDATE`: for each row, `update` on the stored row and
//!      `update_check` on the new row, against the `UPDATE` in a savepoint:
//!      done, no row, or refused by `WITH CHECK`. The new row takes the
//!      next row's values in the columns an unguarded `writable` mask
//!      permits (all columns without one).
//!    - `DELETE`: for each row, `delete` against the `DELETE` in a
//!      savepoint.
//! 4. Unsets each setting in turn for one subject, and compares `SELECT`
//!    again (layer 2: a missing attribute denies as in the application).
//! 5. Runs one `SELECT` without an identity, and expects `permission
//!    denied` (layer 1).
//!
//! It fails, and prints the subject, the rows and both SQL texts, when a
//! pair differs. It also fails when `update` or `delete` admit a row
//! outside `list OR view`, or `update_check` admits a new row outside it:
//! Postgres applies the `SELECT` policy to an `UPDATE` or `DELETE` that
//! reads a column, to the stored row and to the new row of an `UPDATE`
//! (bridge §7, answer 4).
//!
//! # The database
//!
//! The URL's user must create roles and schemas, and bypass row-level
//! security: a superuser, as in CI. Each run creates two roles,
//! `brocanth_agree_<pid>_<n>_member` and `…_login`, and drops them at the
//! end. Each case drops its schema.
//!
//! Each link table is created with no row-level security, and its
//! `SELECT` grant comes from [`Policy::from_rules`], as in an application.
//!
//! A rule marked `#[app_only]` has no database rule to compare. Its command
//! is left out, and the run says so on stderr. So is a write command whose
//! unguarded mask permits no column. `INSERT` writes every column, so the
//! test grants it on the whole table: it compares rows; column grants
//! (decision 9) have their own test.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::str::FromStr as _;
use std::sync::atomic::{AtomicU32, Ordering};

use brocanth_db::sea_query::prelude::{DateTime, Utc, Uuid};
use brocanth_db::sqlx::encode::IsNull;
use brocanth_db::sqlx::error::BoxDynError;
use brocanth_db::sqlx::postgres::{
    PgArgumentBuffer, PgArguments, PgConnectOptions, PgPoolOptions, PgTypeInfo, Postgres as Pg,
};
use brocanth_db::sqlx::{self, AssertSqlSafe, Row as _};
use brocanth_db::{Db, DbError, Tx};
use brocanth_policy::typed::TypedColumn;
use brocanth_policy::{
    Attr, Attributes, Column, DbPolicy, Dialect, Entity, Exists, MapRow, MaskRule, Operand, Pred,
    RowView, Rule, Rules, Subject, Term, Value,
};
use proptest::test_runner::{Config, TestCaseError, TestError, TestRunner};

use crate::def::{Policy, Role};
use crate::expr::SqlExpr;
use crate::identity;
use crate::schema::Schema;
use crate::snapshot::Command;

/// The variable that names the test database in this repository.
pub const URL: &str = "BROCANTH_TEST_DATABASE_URL";

/// [`URL`]'s value, or `None` when it is unset or empty.
///
/// The default of the `db { … }` clause's `url`. An application points the
/// clause at its own `[db] url` instead (`url: <expr>`).
pub fn test_database_url() -> Option<String> {
    std::env::var(URL).ok().filter(|u| !u.trim().is_empty())
}

/// What the `db { … }` clause of `policy!` calls.
///
/// With no `url`, prints `!! SKIPPED` and returns: the test passes. Else
/// runs [`check`] and panics with its report.
///
/// # Panics
/// When [`check`] fails.
pub fn run<P>(url: Option<String>, subjects: &[P::Subject], cases: u32)
where
    P: DbPolicy,
    <P::Entity as Entity>::Column: TypedColumn,
{
    let name = std::any::type_name::<P>();
    let Some(url) = url else {
        skipped(&format!(
            "{name}: no database URL; set {URL} (or the app's [db] url) to compare the policy with the database"
        ));
        return;
    };
    if let Err(report) = check::<P>(&url, subjects, cases) {
        panic!("the database policy and condition() disagree for `{name}`:\n{report}");
    }
}

/// Compares `P`'s rules in the application and in the database, for each
/// of `subjects`, over `cases` generated tables.
///
/// Runs on a thread of its own with its own runtime, so a `#[test]` and a
/// `#[tokio::test]` can both call it.
///
/// # Errors
/// The report of the smallest failing case: the command, the subject, the
/// rows and both SQL texts. Or why the run could not start, with a `help:`
/// line.
pub fn check<P>(url: &str, subjects: &[P::Subject], cases: u32) -> Result<(), String>
where
    P: DbPolicy,
    <P::Entity as Entity>::Column: TypedColumn,
{
    if subjects.is_empty() {
        return Err("no subjects to test\nhelp: list at least one subject".to_string());
    }
    let subjects: Vec<Attributes> = subjects.iter().map(Subject::attributes).collect();
    let url = url.to_string();
    std::thread::scope(|s| {
        s.spawn(move || check_attributes::<P>(&url, subjects, cases))
            .join()
            .unwrap_or_else(|e| std::panic::resume_unwind(e))
    })
}

/// Print a skip line that cargo's output capture cannot swallow.
fn skipped(reason: &str) {
    use std::io::Write as _;
    let line = format!("!! SKIPPED {reason}\n");
    if let Ok(mut fd2) = std::fs::OpenOptions::new().write(true).open("/dev/stderr") {
        if fd2.write_all(line.as_bytes()).is_ok() {
            return;
        }
    }
    eprint!("{line}");
}

/// Everything a case reads, computed once.
struct Plan<E: Entity> {
    rules: Rules<E>,
    table: &'static str,
    key: E::Column,
    /// The columns other than the key, in declaration order.
    data: Vec<E::Column>,
    attrs: &'static [Attr],
    subjects: Vec<Resolved<E::Column>>,
    links: Vec<Link>,
    /// The policies, grants and `ENABLE`/`FORCE`, for one case's schema.
    policy_sql: String,
    /// Commands left out, and why: an `#[app_only]` rule, or a write mask
    /// that permits no column.
    skipped: Vec<(Command, &'static str)>,
    /// The columns an `UPDATE` sets: the `writable` mask's, when it has no
    /// guard (decision 9), or every column but the key.
    update_columns: Vec<E::Column>,
    member: String,
    login: String,
}

/// One subject's rules, resolved.
struct Resolved<C: Column> {
    attrs: Attributes,
    select: Pred<C>,
    create: Pred<C>,
    update: Pred<C>,
    update_check: Pred<C>,
    delete: Pred<C>,
}

impl<E: Entity> Plan<E> {
    fn resolve(&self, attrs: Attributes) -> Resolved<E::Column> {
        resolve(&self.rules, attrs)
    }

    fn skips(&self, command: Command) -> bool {
        self.skipped.iter().any(|(c, _)| *c == command)
    }
}

fn resolve<E: Entity>(rules: &Rules<E>, attrs: Attributes) -> Resolved<E::Column> {
    Resolved {
        select: rules.list.resolve(&attrs).or(rules.view().resolve(&attrs)),
        create: rules.create.resolve(&attrs),
        update: rules.update.resolve(&attrs),
        update_check: rules.update_check().resolve(&attrs),
        delete: rules.delete.resolve(&attrs),
        attrs,
    }
}

/// A link table an `exists` reads: its name, and each column with its type
/// and the values generated rows draw from.
struct Link {
    table: &'static str,
    columns: Vec<(&'static str, String, Vec<Value>)>,
}

fn check_attributes<P>(url: &str, subjects: Vec<Attributes>, cases: u32) -> Result<(), String>
where
    P: DbPolicy,
    <P::Entity as Entity>::Column: TypedColumn,
{
    static RUN: AtomicU32 = AtomicU32::new(0);
    let prefix = format!(
        "brocanth_agree_{}_{}",
        std::process::id(),
        RUN.fetch_add(1, Ordering::Relaxed)
    );
    let member = format!("{prefix}_member");
    let login = format!("{prefix}_login");
    let password = format!("pw_{prefix}");

    let plan = plan::<P>(subjects, &member, &login)?;
    for (command, why) in &plan.skipped {
        eprintln!(
            "note: {}: {why}, so the agreement test leaves {command} out",
            std::any::type_name::<P>()
        );
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("a runtime: {e}"))?;
    let admin = runtime.block_on(connect(url, None))?;
    let roles = format!(
        "CREATE ROLE {m} NOLOGIN; CREATE ROLE {l} LOGIN PASSWORD '{password}' NOINHERIT; GRANT {m} TO {l};",
        m = quote(&member),
        l = quote(&login),
    );
    let result = match runtime.block_on(raw(&admin, &roles)) {
        Err(e) => Err(format!(
            "{e}\nhelp: the URL's user must create roles and bypass row-level security: use a superuser for this test"
        )),
        Ok(()) => match runtime.block_on(connect(url, Some((&login, &password)))) {
            Err(e) => Err(e),
            Ok(login_db) => {
                let r = run_cases(&runtime, &plan, &admin, &login_db, &prefix, cases);
                runtime.block_on(login_db.close());
                r
            }
        },
    };
    runtime.block_on(async {
        let drop = format!(
            "DROP OWNED BY {m} CASCADE; DROP ROLE IF EXISTS {l}; DROP ROLE IF EXISTS {m};",
            m = quote(&member),
            l = quote(&login),
        );
        // A role that was never created: nothing to drop.
        let _ = raw(&admin, &drop).await;
        admin.close().await;
    });
    result
}

/// The plan: resolved rules, link tables, and the policy SQL.
fn plan<P>(subjects: Vec<Attributes>, member: &str, login: &str) -> Result<Plan<P::Entity>, String>
where
    P: DbPolicy,
    <P::Entity as Entity>::Column: TypedColumn,
{
    let rules = P::rules();
    let attrs = <P::Subject as Subject>::ATTRS;
    let key = <P::Entity as Entity>::PRIMARY_KEY;

    // An `#[app_only]` command has no database rule. Give it one that does
    // not get in the way of the others: `SELECT` reads every row (Postgres
    // applies it to the other commands too), the rest none.
    let role = Role::existing(member);
    let mut policies = Policy::from_rules::<P>(&role);
    let mut skipped = Vec::new();
    for (command, app_only) in [
        (
            Command::Select,
            rules.list.app_only.or(rules.view().app_only),
        ),
        (Command::Insert, rules.create.app_only),
        (
            Command::Update,
            rules.update.app_only.or(rules.update_check().app_only),
        ),
        (Command::Delete, rules.delete.app_only),
    ] {
        if app_only.is_some() {
            let expr = match command {
                Command::Select => SqlExpr::always(),
                _ => SqlExpr::never(),
            };
            policies = policies.manual(command, expr);
            skipped.push((command, "its rule is #[app_only]"));
        }
    }
    // A write mask with no guard and no column: the application writes
    // nothing, whatever the rule says (decision 9).
    let fixed = |mask: Option<&MaskRule<<P::Entity as Entity>::Column>>| match mask {
        Some(MaskRule::Fixed(_)) => mask.and_then(MaskRule::columns),
        _ => None,
    };
    for (command, mask) in [
        (Command::Insert, rules.writable_on_create()),
        (Command::Update, rules.writable.as_ref()),
    ] {
        if fixed(mask).is_some_and(|c| c.is_empty()) && !skipped.iter().any(|(c, _)| *c == command)
        {
            skipped.push((command, "its write mask permits no column"));
        }
    }
    let schema = Schema::new()
        .role(role)
        .role(Role::existing(login))
        .policies(policies);
    let policy_sql = schema.create_sql().map_err(|e| {
        format!("{e}\nhelp: the database cannot hold this policy, and `rls make` refuses it too")
    })?;

    let subjects: Vec<_> = subjects.into_iter().map(|a| resolve(&rules, a)).collect();
    let data: Vec<_> = <P::Entity as Entity>::Column::ALL
        .iter()
        .copied()
        .filter(|c| *c != key)
        .collect();
    let update_columns = match fixed(rules.writable.as_ref()) {
        Some(columns) => columns.into_iter().filter(|c| *c != key).collect(),
        None => data.clone(),
    };
    let links = links(&rules, attrs, &subjects, key);
    Ok(Plan {
        rules,
        table: <P::Entity as Entity>::TABLE.as_str(),
        key,
        data,
        attrs,
        subjects,
        links,
        policy_sql,
        skipped,
        update_columns,
        member: member.to_string(),
        login: login.to_string(),
    })
}

/// The link tables of every `exists` in `rules`.
fn links<E: Entity>(
    rules: &Rules<E>,
    attrs: &[Attr],
    subjects: &[Resolved<E::Column>],
    key: E::Column,
) -> Vec<Link>
where
    E::Column: TypedColumn,
{
    let mut found = Vec::new();
    for rule in [
        &rules.list,
        rules.view(),
        &rules.create,
        &rules.update,
        rules.update_check(),
        &rules.delete,
    ] {
        collect_exists(rule, &mut found);
    }
    let mut out: Vec<Link> = Vec::new();
    for exists in found {
        let table = exists.table.as_str();
        if table == E::TABLE.as_str() {
            continue;
        }
        let index = match out.iter().position(|l| l.table == table) {
            Some(i) => i,
            None => {
                out.push(Link {
                    table,
                    columns: Vec::new(),
                });
                out.len() - 1
            }
        };
        let link = &mut out[index];
        // The column that points at this row: the local column's type, and
        // the values the local column takes.
        // The key: the stored rows' keys, and the candidates' of `INSERT`.
        let mut local = if exists.local == key {
            (1..=5)
                .chain(CANDIDATE..CANDIDATE + 5)
                .map(|n| key.key(n))
                .collect()
        } else {
            brocanth_policy::agreement::pool(exists.local, subjects.iter().flat_map(preds_of))
        };
        push_value(&mut local, Value::Null);
        add_column(
            link,
            exists.foreign.as_str(),
            exists.local.sql_type(),
            local,
        );
        for (column, _, operand) in &exists.filter {
            let (ty, mut values) = match operand {
                Operand::Value(v) => (value_type(v), vec![v.clone()]),
                Operand::Attr { key, .. } => (
                    attrs
                        .iter()
                        .find(|a| a.key == *key)
                        .map(|a| a.kind.sql_type()),
                    subjects
                        .iter()
                        .filter_map(|s| s.attrs.get(key).cloned())
                        .collect(),
                ),
            };
            push_value(&mut values, Value::Null);
            add_column(link, column.as_str(), ty.unwrap_or("text"), values);
        }
    }
    out
}

fn preds_of<C: Column>(r: &Resolved<C>) -> [&Pred<C>; 5] {
    [&r.select, &r.create, &r.update, &r.update_check, &r.delete]
}

fn add_column(link: &mut Link, name: &'static str, ty: &str, values: Vec<Value>) {
    match link.columns.iter_mut().find(|(n, _, _)| *n == name) {
        Some((_, _, pool)) => values.into_iter().for_each(|v| push_value(pool, v)),
        None => link.columns.push((name, ty.to_string(), values)),
    }
}

fn push_value(out: &mut Vec<Value>, value: Value) {
    if !out.contains(&value) {
        out.push(value);
    }
}

/// The type of a link column compared with a literal.
fn value_type(v: &Value) -> Option<&'static str> {
    Some(match v {
        Value::Null => return None,
        Value::Bool(_) => "boolean",
        Value::Int(_) => "bigint",
        Value::Float(_) => "double precision",
        Value::Text(_) => "text",
        Value::Bytes(_) => "bytea",
        Value::Uuid(_) => "uuid",
        Value::Timestamptz(_) => "timestamptz",
    })
}

fn collect_exists<C: Column>(rule: &Rule<C>, out: &mut Vec<Exists<C>>) {
    if rule.app_only.is_some() {
        return;
    }
    for (_, term) in &rule.arms {
        collect_term(term, out);
    }
    collect_term(&rule.fallback, out);
}

fn collect_term<C: Column>(term: &Term<C>, out: &mut Vec<Exists<C>>) {
    match term {
        Term::Exists(e) => out.push((**e).clone()),
        Term::All(parts) | Term::Any(parts) => parts.iter().for_each(|t| collect_term(t, out)),
        Term::Not(inner) => collect_term(inner, out),
        _ => {}
    }
}

/// The key of the first `INSERT` candidate: the `n`th candidate has
/// `CANDIDATE + n`, the `n`th stored row `1 + n`.
const CANDIDATE: i64 = 1000;

/// One generated case: the rows' cells, and the rows of each link table.
type Generated = (Vec<Vec<Value>>, Vec<Vec<Vec<Value>>>);

fn run_cases<E: Entity>(
    runtime: &tokio::runtime::Runtime,
    plan: &Plan<E>,
    admin: &Db,
    login: &Db,
    prefix: &str,
    cases: u32,
) -> Result<(), String>
where
    E::Column: TypedColumn,
{
    let row = plan
        .data
        .iter()
        .map(|c| {
            let pool =
                brocanth_policy::agreement::pool(*c, plan.subjects.iter().flat_map(preds_of));
            proptest::sample::select(pool)
        })
        .collect::<Vec<_>>();
    let links = plan
        .links
        .iter()
        .map(|link| {
            let row = link
                .columns
                .iter()
                .map(|(_, _, pool)| proptest::sample::select(pool.clone()))
                .collect::<Vec<_>>();
            proptest::collection::vec(row, 0..6)
        })
        .collect::<Vec<_>>();
    let strategy = (proptest::collection::vec(row, 1..6), links);

    let config = Config {
        cases,
        failure_persistence: None,
        ..Config::default()
    };
    let mut runner = TestRunner::new(config);
    let n = AtomicU32::new(0);
    let result = runner.run(&strategy, |(bodies, links): Generated| {
        let number = n.fetch_add(1, Ordering::Relaxed);
        let schema = format!("{prefix}_c{number}");
        let rows: Vec<MapRow<E::Column>> = bodies
            .into_iter()
            .enumerate()
            .map(|(i, cells)| {
                let mut row = MapRow::new().with(plan.key, plan.key.key(i as i64 + 1));
                for (column, value) in plan.data.iter().zip(cells) {
                    row = row.with(*column, value);
                }
                row
            })
            .collect();
        runtime
            .block_on(async {
                let case = Case {
                    plan,
                    admin,
                    login,
                    schema: &schema,
                    rows: &rows,
                    links: &links,
                    ids: Vec::new(),
                };
                let out = case.run(number).await;
                let _ = raw(
                    admin,
                    &format!("DROP SCHEMA IF EXISTS {} CASCADE", quote(&schema)),
                )
                .await;
                out
            })
            .map_err(TestCaseError::fail)
    });
    // The report already holds the shrunk rows; proptest's dump of the
    // input would repeat them as `Value`s.
    result.map_err(|e| match e {
        TestError::Fail(reason, _) => reason.message().to_string(),
        TestError::Abort(reason) => reason.message().to_string(),
    })
}

/// The outcome of one write statement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    /// One row written.
    Done,
    /// No row: the `USING` clause or the `SELECT` policy hid it, or the role
    /// has no grant (`permission denied`).
    Nothing,
    /// `new row violates row-level security policy`: `WITH CHECK` refused.
    Refused,
}

/// A set of ids as text, and the SQL that found them.
struct Ids {
    ids: BTreeSet<String>,
    sql: String,
}

/// One generated case, in its own schema.
struct Case<'a, E: Entity> {
    plan: &'a Plan<E>,
    admin: &'a Db,
    login: &'a Db,
    schema: &'a str,
    rows: &'a [MapRow<E::Column>],
    links: &'a [Vec<Vec<Value>>],
    /// Each row's key, as Postgres writes it as text.
    ids: Vec<String>,
}

impl<E: Entity> Case<'_, E>
where
    E::Column: TypedColumn,
{
    async fn run(mut self, case: u32) -> Result<(), String> {
        self.setup().await?;
        let plan = self.plan;

        // Layer 1: no identity, no grant.
        let mut tx = self.login.begin().await.map_err(text)?;
        search_path(&mut tx, self.schema).await?;
        let sql = format!("SELECT 1 FROM {}", quote(plan.table));
        let got = sqlx::query(AssertSqlSafe(sql.clone()))
            .fetch_all(tx.connection())
            .await;
        drop(tx);
        match got {
            Err(e)
                if is_code(&e, "42501")
                    && e.to_string().contains("permission denied for table") => {}
            other => {
                return Err(format!(
                    "layer 1: `{sql}` as the login {} with no identity: want `permission denied for table`, got {}\n{}",
                    plan.login,
                    match other {
                        Ok(rows) => format!("{} rows", rows.len()),
                        Err(e) => e.to_string(),
                    },
                    self.show_rows()
                ));
            }
        }

        for (index, subject) in plan.subjects.iter().enumerate() {
            let who = format!("subject {index} {}", show_attrs(&subject.attrs));
            self.compare(&who, subject).await?;
        }

        // Layer 2: each setting unset in turn, for one subject.
        if !plan.skips(Command::Select) {
            let index = case as usize % plan.subjects.len();
            for attr in plan.attrs {
                let mut attrs = plan.subjects[index].attrs.clone();
                attrs.set(attr.key, None);
                let who = format!(
                    "subject {index} with `{}` unset {}",
                    attr.key,
                    show_attrs(&attrs)
                );
                let unset = plan.resolve(attrs);
                self.compare_select(&who, &unset).await?;
            }
        }
        Ok(())
    }

    /// The schema, the tables, the rows, the grants and the policies.
    async fn setup(&mut self) -> Result<(), String> {
        let plan = self.plan;
        let s = quote(self.schema);
        let (m, l) = (quote(&plan.member), quote(&plan.login));
        let mut ddl = format!(
            "CREATE SCHEMA {s};\nSET LOCAL search_path TO {s};\nGRANT USAGE ON SCHEMA {s} TO {m}, {l};\n"
        );
        let columns: Vec<String> = std::iter::once(plan.key)
            .chain(plan.data.iter().copied())
            .map(|c| {
                let nullable = c.samples().iter().any(Value::is_null);
                format!(
                    "{} {}{}{}",
                    quote(c.name()),
                    c.sql_type(),
                    if nullable { "" } else { " NOT NULL" },
                    if c == plan.key { " PRIMARY KEY" } else { "" },
                )
            })
            .collect();
        let _ = writeln!(
            ddl,
            "CREATE TABLE {} ({});",
            quote(plan.table),
            columns.join(", ")
        );
        for link in &plan.links {
            let columns: Vec<String> = link
                .columns
                .iter()
                .map(|(name, ty, _)| format!("{} {ty}", quote(name)))
                .collect();
            // No grant here: `from_rules` grants the link table, and the
            // policy SQL below holds it.
            let _ = writeln!(
                ddl,
                "CREATE TABLE {} ({});",
                quote(link.table),
                columns.join(", "),
            );
        }
        ddl.push_str(&plan.policy_sql);
        // `INSERT` writes every column, so it needs the table, not the
        // columns of an unguarded `writable_on_create` (decision 9). Column
        // grants have their own test; this one compares rows.
        if !plan.skips(Command::Insert) {
            let _ = writeln!(ddl, "\nGRANT INSERT ON {} TO {m};", quote(plan.table));
        }

        let mut tx = self.admin.begin().await.map_err(text)?;
        sqlx::raw_sql(AssertSqlSafe(ddl.clone()))
            .execute(tx.connection())
            .await
            .map_err(|e| format!("the case's schema did not apply: {e}\n--- sql ---\n{ddl}"))?;
        for row in self.rows {
            insert(&mut tx, plan, row)
                .await
                .map_err(|e| e.to_string())?;
            let id = id_text(&mut tx, plan, &row.get(plan.key)).await?;
            self.ids.push(id);
        }
        for (link, rows) in plan.links.iter().zip(self.links) {
            let names: Vec<String> = link.columns.iter().map(|(n, _, _)| quote(n)).collect();
            let sql = format!(
                "INSERT INTO {} ({}) VALUES ({})",
                quote(link.table),
                names.join(", "),
                placeholders(names.len(), 0)
            );
            for row in rows {
                bind_all(sqlx::query(AssertSqlSafe(sql.clone())), row)
                    .execute(tx.connection())
                    .await
                    .map_err(|e| format!("{sql}: {e}"))?;
            }
        }
        tx.commit().await.map_err(text)
    }

    /// `SELECT`, then `INSERT`, `UPDATE` and `DELETE`, for one subject.
    async fn compare(&self, who: &str, subject: &Resolved<E::Column>) -> Result<(), String> {
        let plan = self.plan;
        if !plan.skips(Command::Select) {
            let select = self.compare_select(who, subject).await?;
            for (command, pred) in [
                (Command::Update, &subject.update),
                (Command::Delete, &subject.delete),
            ] {
                if plan.skips(command) {
                    continue;
                }
                let app = self.app(pred).await?;
                let outside: Vec<&String> = app.ids.difference(&select.ids).collect();
                if !outside.is_empty() {
                    let name = command.to_string().to_lowercase();
                    return Err(format!(
                        "`{name}` admits rows that `list OR view` does not, for {who}: ids {outside:?}\n\
                         Postgres applies the SELECT policy to every {command} that reads a column, so the database hides these rows from it.\n\
                         help: keep `{name}` inside `list` (or `view`)\n{}  {name}: {}\n  list OR view: {}",
                        self.show_rows(),
                        app.sql,
                        select.sql,
                    ));
                }
            }
        }

        let me = identity::of_attributes(&Role::existing(&plan.member), plan.attrs, &subject.attrs);
        let mut tx = self.login.begin().await.map_err(text)?;
        tx.assume(&me).await.map_err(text)?;
        search_path(&mut tx, self.schema).await?;
        if !plan.skips(Command::Insert) {
            self.compare_insert(who, subject, &mut tx).await?;
        }
        if !plan.skips(Command::Update) {
            self.compare_update(who, subject, &mut tx).await?;
        }
        if !plan.skips(Command::Delete) {
            self.compare_delete(who, subject, &mut tx).await?;
        }
        let _ = tx.rollback().await;
        Ok(())
    }

    /// The application's rows against the database's, for `SELECT`.
    async fn compare_select(
        &self,
        who: &str,
        subject: &Resolved<E::Column>,
    ) -> Result<Ids, String> {
        let plan = self.plan;
        let app = self.app(&subject.select).await?;
        let me = identity::of_attributes(&Role::existing(&plan.member), plan.attrs, &subject.attrs);
        let mut tx = self.login.begin().await.map_err(text)?;
        tx.assume(&me).await.map_err(text)?;
        search_path(&mut tx, self.schema).await?;
        let sql = format!(
            "SELECT CAST({} AS text) FROM {}",
            quote(plan.key.name()),
            quote(plan.table)
        );
        let db: BTreeSet<String> = sqlx::query(AssertSqlSafe(sql.clone()))
            .fetch_all(tx.connection())
            .await
            .map_err(|e| format!("{sql} as {who}: {e}"))?
            .iter()
            .map(|r| r.get::<String, _>(0))
            .collect();
        let _ = tx.rollback().await;
        if app.ids != db {
            return Err(format!(
                "SELECT disagrees for {who}\n{}  app (condition, as the URL's user): {}\n    binds {}\n    ids {:?}\n  database (as {}): {sql}\n    ids {db:?}\n  {}",
                self.show_rows(),
                app.sql,
                self.binds(&subject.select),
                app.ids,
                plan.member,
                self.policy("SELECT"),
            ));
        }
        Ok(app)
    }

    /// Each row as a candidate, with a fresh key: `create.eval` against the
    /// `INSERT`.
    async fn compare_insert(
        &self,
        who: &str,
        subject: &Resolved<E::Column>,
        tx: &mut Tx<'_>,
    ) -> Result<(), String> {
        let plan = self.plan;
        for (i, row) in self.rows.iter().enumerate() {
            let key = plan.key.key(CANDIDATE + i as i64);
            let candidate = with_key(plan, row, key.clone());
            let app = match subject.create.eval(&candidate) {
                Ok(truth) => truth.admits(),
                // An `exists` has no in-memory reading: ask the condition,
                // with the candidate in the table, as the app's admission
                // would see it.
                Err(_) => {
                    let mut a = self.admin.begin().await.map_err(text)?;
                    search_path(&mut a, self.schema).await?;
                    insert(&mut a, plan, &candidate)
                        .await
                        .map_err(|e| e.to_string())?;
                    let id = id_text(&mut a, plan, &key).await?;
                    let ids = ids_in::<E>(&mut a, &subject.create).await?;
                    ids.ids.contains(&id)
                }
            };
            let mut sp = tx.begin().await.map_err(text)?;
            let db = outcome(insert(&mut sp, plan, &candidate).await)?;
            let _ = sp.rollback().await;
            let agree = match db {
                Outcome::Done => app,
                Outcome::Nothing | Outcome::Refused => !app,
            };
            if !agree {
                return Err(format!(
                    "INSERT disagrees for {who} on the candidate {}\n{}  app: create.eval admits it: {app}\n    {}\n    binds {}\n  database (as {}): {db:?}\n  {}",
                    show_row(plan, &candidate),
                    self.show_rows(),
                    self.sql(&subject.create),
                    self.binds(&subject.create),
                    plan.member,
                    self.policy("INSERT"),
                ));
            }
        }
        Ok(())
    }

    /// Each row, set to the values of the next row: `update` on the stored
    /// row and `update_check` on the new row, against the `UPDATE`.
    async fn compare_update(
        &self,
        who: &str,
        subject: &Resolved<E::Column>,
        tx: &mut Tx<'_>,
    ) -> Result<(), String> {
        let plan = self.plan;
        let update = self.app(&subject.update).await?;
        for (i, row) in self.rows.iter().enumerate() {
            let id = &self.ids[i];
            // The next row's values, in the columns the mask lets an update
            // set; the stored values elsewhere.
            let next = &self.rows[(i + 1) % self.rows.len()];
            let mut new_row = row.clone();
            for c in &plan.update_columns {
                new_row = new_row.with(*c, next.get(*c));
            }
            let stored = update.ids.contains(id);
            let (check, visible) = {
                let mut a = self.admin.begin().await.map_err(text)?;
                search_path(&mut a, self.schema).await?;
                set_row(&mut a, plan, &new_row)
                    .await
                    .map_err(|e| e.to_string())?;
                let check = ids_in::<E>(&mut a, &subject.update_check).await?;
                let visible = ids_in::<E>(&mut a, &subject.select).await?;
                (check.ids.contains(id), visible.ids.contains(id))
            };
            if stored && check && !visible && !plan.skips(Command::Select) {
                return Err(format!(
                    "`update_check` admits a new row that `list OR view` does not, for {who}: row {id} to {}\n\
                     Postgres checks the new row of an UPDATE that reads a column against the SELECT policy too, so it refuses this update.\n\
                     help: keep `update_check` inside `list` (or `view`)\n{}  update_check: {}\n  list OR view: {}",
                    show_row(plan, &new_row),
                    self.show_rows(),
                    self.sql(&subject.update_check),
                    self.sql(&subject.select),
                ));
            }
            let want = match (stored, check) {
                (false, _) => Outcome::Nothing,
                (true, false) => Outcome::Refused,
                (true, true) => Outcome::Done,
            };
            let mut sp = tx.begin().await.map_err(text)?;
            let db = outcome(set_row(&mut sp, plan, &new_row).await)?;
            let _ = sp.rollback().await;
            if db != want {
                return Err(format!(
                    "UPDATE disagrees for {who} on row {id} to {}\n{}  app: update admits the stored row: {stored}; update_check admits the new row: {check}; want {want:?}\n    update: {}\n      binds {}\n    update_check: {}\n      binds {}\n  database (as {}): {db:?}\n  {}",
                    show_row(plan, &new_row),
                    self.show_rows(),
                    update.sql,
                    self.binds(&subject.update),
                    self.sql(&subject.update_check),
                    self.binds(&subject.update_check),
                    plan.member,
                    self.policy("UPDATE"),
                ));
            }
        }
        Ok(())
    }

    /// Each row: `delete` against the `DELETE`.
    async fn compare_delete(
        &self,
        who: &str,
        subject: &Resolved<E::Column>,
        tx: &mut Tx<'_>,
    ) -> Result<(), String> {
        let plan = self.plan;
        let delete = self.app(&subject.delete).await?;
        let sql = format!(
            "DELETE FROM {} WHERE {} = $1",
            quote(plan.table),
            quote(plan.key.name())
        );
        for (row, id) in self.rows.iter().zip(&self.ids) {
            let app = delete.ids.contains(id);
            let mut sp = tx.begin().await.map_err(text)?;
            let done = bind_all(
                sqlx::query(AssertSqlSafe(sql.clone())),
                &[row.get(plan.key)],
            )
            .execute(sp.connection())
            .await
            .map(|d| d.rows_affected());
            let db = outcome(done)?;
            let _ = sp.rollback().await;
            let want = if app { Outcome::Done } else { Outcome::Nothing };
            if db != want {
                return Err(format!(
                    "DELETE disagrees for {who} on row {id}\n{}  app: delete admits it: {app}\n    {}\n    binds {}\n  database (as {}): {db:?}\n  {}",
                    self.show_rows(),
                    delete.sql,
                    self.binds(&subject.delete),
                    plan.member,
                    self.policy("DELETE"),
                ));
            }
        }
        Ok(())
    }

    /// The ids `pred` admits, as the URL's user, which bypasses the
    /// policies.
    async fn app(&self, pred: &Pred<E::Column>) -> Result<Ids, String> {
        let mut tx = self.admin.begin().await.map_err(text)?;
        search_path(&mut tx, self.schema).await?;
        ids_in::<E>(&mut tx, pred).await
    }

    fn sql(&self, pred: &Pred<E::Column>) -> String {
        match pred.matching_ids::<E>(Dialect::Postgres) {
            Ok(q) => q.sql().to_string(),
            Err(e) => format!("<{e}>"),
        }
    }

    fn binds(&self, pred: &Pred<E::Column>) -> String {
        match pred.matching_ids::<E>(Dialect::Postgres) {
            Ok(q) => {
                let binds: Vec<String> = q.binds().iter().map(ToString::to_string).collect();
                format!("[{}]", binds.join(", "))
            }
            Err(e) => format!("<{e}>"),
        }
    }

    fn show_rows(&self) -> String {
        let mut out = format!("  rows of {}:\n", self.plan.table);
        for row in self.rows {
            let _ = writeln!(out, "    {}", show_row(self.plan, row));
        }
        for (link, rows) in self.plan.links.iter().zip(self.links) {
            let _ = writeln!(out, "  rows of {}:", link.table);
            for row in rows {
                let cells: Vec<String> = link
                    .columns
                    .iter()
                    .zip(row)
                    .map(|((name, _, _), v)| format!("{name}={v}"))
                    .collect();
                let _ = writeln!(out, "    {}", cells.join(" "));
            }
        }
        out
    }

    /// The `CREATE POLICY` statement of `command`, as the case applied it.
    fn policy(&self, command: &str) -> String {
        let want = format!(" FOR {command} ");
        let lines: Vec<&str> = self
            .plan
            .policy_sql
            .lines()
            .filter(|l| l.starts_with("CREATE POLICY") && l.contains(&want))
            .collect();
        if lines.is_empty() {
            format!("no {command} policy, so no grant")
        } else {
            lines.join("\n  ")
        }
    }
}

/// The ids `pred` admits, in this transaction.
async fn ids_in<E: Entity>(tx: &mut Tx<'_>, pred: &Pred<E::Column>) -> Result<Ids, String> {
    let query = pred
        .matching_ids::<E>(Dialect::Postgres)
        .map_err(|e| e.to_string())?;
    let sql = format!("SELECT CAST(k AS text) FROM ({}) AS q(k)", query.sql());
    let ids = bind_all(sqlx::query(AssertSqlSafe(sql)), query.binds())
        .fetch_all(tx.connection())
        .await
        .map_err(|e| format!("{}: {e}", query.sql()))?
        .iter()
        .map(|r| r.get::<String, _>(0))
        .collect();
    Ok(Ids {
        ids,
        sql: query.sql().to_string(),
    })
}

/// The key as Postgres writes it as text.
async fn id_text<E: Entity>(tx: &mut Tx<'_>, plan: &Plan<E>, key: &Value) -> Result<String, String>
where
    E::Column: TypedColumn,
{
    let sql = format!("SELECT CAST(CAST($1 AS {}) AS text)", plan.key.sql_type());
    bind_all(
        sqlx::query(AssertSqlSafe(sql.clone())),
        std::slice::from_ref(key),
    )
    .fetch_one(tx.connection())
    .await
    .map(|r| r.get::<String, _>(0))
    .map_err(|e| format!("{sql}: {e}"))
}

/// `INSERT` one row, every column.
async fn insert<E: Entity>(
    tx: &mut Tx<'_>,
    plan: &Plan<E>,
    row: &MapRow<E::Column>,
) -> Result<u64, sqlx::Error> {
    let columns: Vec<E::Column> = std::iter::once(plan.key)
        .chain(plan.data.iter().copied())
        .collect();
    let names: Vec<String> = columns.iter().map(|c| quote(c.name())).collect();
    let values: Vec<Value> = columns.iter().map(|c| row.get(*c)).collect();
    let sql = format!(
        "INSERT INTO {} ({}) VALUES ({})",
        quote(plan.table),
        names.join(", "),
        placeholders(names.len(), 0)
    );
    bind_all(sqlx::query(AssertSqlSafe(sql)), &values)
        .execute(tx.connection())
        .await
        .map(|d| d.rows_affected())
}

/// `UPDATE` the row with `row`'s key: set [`Plan::update_columns`] to
/// `row`'s values.
async fn set_row<E: Entity>(
    tx: &mut Tx<'_>,
    plan: &Plan<E>,
    row: &MapRow<E::Column>,
) -> Result<u64, sqlx::Error> {
    let mut values: Vec<Value> = plan.update_columns.iter().map(|c| row.get(*c)).collect();
    let sets: Vec<String> = plan
        .update_columns
        .iter()
        .enumerate()
        .map(|(i, c)| format!("{} = ${}", quote(c.name()), i + 1))
        .collect();
    values.push(row.get(plan.key));
    let sql = format!(
        "UPDATE {} SET {} WHERE {} = ${}",
        quote(plan.table),
        sets.join(", "),
        quote(plan.key.name()),
        values.len()
    );
    bind_all(sqlx::query(AssertSqlSafe(sql)), &values)
        .execute(tx.connection())
        .await
        .map(|d| d.rows_affected())
}

fn placeholders(n: usize, from: usize) -> String {
    (from + 1..=from + n)
        .map(|i| format!("${i}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A write's result as an [`Outcome`]; another error is the harness's.
fn outcome(result: Result<u64, sqlx::Error>) -> Result<Outcome, String> {
    match result {
        Ok(0) => Ok(Outcome::Nothing),
        Ok(_) => Ok(Outcome::Done),
        Err(e) if is_code(&e, "42501") => {
            let message = e.to_string();
            if message.contains("row-level security") {
                Ok(Outcome::Refused)
            } else if message.contains("permission denied") {
                Ok(Outcome::Nothing)
            } else {
                Err(message)
            }
        }
        Err(e) => Err(e.to_string()),
    }
}

fn is_code(e: &sqlx::Error, code: &str) -> bool {
    e.as_database_error()
        .and_then(|d| d.code())
        .is_some_and(|c| c == code)
}

fn text(e: DbError) -> String {
    e.to_string()
}

async fn search_path(tx: &mut Tx<'_>, schema: &str) -> Result<(), String> {
    sqlx::raw_sql(AssertSqlSafe(format!(
        "SET LOCAL search_path TO {}",
        quote(schema)
    )))
    .execute(tx.connection())
    .await
    .map(drop)
    .map_err(|e| e.to_string())
}

/// A pool of one connection, as the URL's user or as `(user, password)`.
async fn connect(url: &str, user: Option<(&str, &str)>) -> Result<Db, String> {
    let mut options = PgConnectOptions::from_str(url)
        .map_err(|e| format!("{e}\nhelp: give a Postgres URL, postgres://user:password@host/db"))?;
    if let Some((user, password)) = user {
        options = options.username(user).password(password);
    }
    PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map(Db::from_pool)
        .map_err(|e| {
            format!(
                "connect to the test database: {e}\nhelp: check the URL, and that Postgres runs"
            )
        })
}

/// Many statements, in one transaction.
async fn raw(db: &Db, sql: &str) -> Result<(), String> {
    let mut tx = db.begin().await.map_err(text)?;
    sqlx::raw_sql(AssertSqlSafe(sql.to_string()))
        .execute(tx.connection())
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(text)
}

/// A double-quoted identifier.
fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn show_attrs(attrs: &Attributes) -> String {
    let parts: Vec<String> = attrs
        .iter()
        .map(|(k, v)| match v {
            Some(v) => format!("{k}={v}"),
            None => format!("{k}=absent"),
        })
        .collect();
    format!("{{{}}}", parts.join(", "))
}

fn show_row<E: Entity>(plan: &Plan<E>, row: &MapRow<E::Column>) -> String {
    std::iter::once(plan.key)
        .chain(plan.data.iter().copied())
        .map(|c| format!("{}={}", c.name(), row.get(c)))
        .collect::<Vec<_>>()
        .join(" ")
}

fn with_key<E: Entity>(plan: &Plan<E>, row: &MapRow<E::Column>, key: Value) -> MapRow<E::Column> {
    let mut out = MapRow::new().with(plan.key, key);
    for c in &plan.data {
        out = out.with(*c, row.get(*c));
    }
    out
}

/// A NULL with no type of its own: Postgres infers it from the column, as
/// for a literal `NULL`. `Option::<i64>::None` would bind as `int8`, and
/// `text_column = $1` would fail.
struct UntypedNull;

impl sqlx::Type<Pg> for UntypedNull {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::with_oid(sqlx::postgres::types::Oid(0))
    }
}

impl sqlx::Encode<'_, Pg> for UntypedNull {
    fn encode_by_ref(&self, _buf: &mut PgArgumentBuffer) -> Result<IsNull, BoxDynError> {
        Ok(IsNull::Yes)
    }
}

type PgQuery<'q> = sqlx::query::Query<'q, Pg, PgArguments>;

fn bind_all<'q>(mut query: PgQuery<'q>, binds: &[Value]) -> PgQuery<'q> {
    for value in binds {
        query = match value {
            Value::Null => query.bind(UntypedNull),
            Value::Bool(b) => query.bind(*b),
            Value::Int(i) => query.bind(*i),
            Value::Float(f) => query.bind(*f),
            Value::Text(t) => query.bind(t.clone()),
            Value::Bytes(b) => query.bind(b.clone()),
            Value::Uuid(b) => query.bind(Uuid::from_bytes(*b)),
            Value::Timestamptz(micros) => match DateTime::<Utc>::from_timestamp_micros(*micros) {
                Some(t) => query.bind(t),
                None => query.bind(UntypedNull),
            },
        };
    }
    query
}
