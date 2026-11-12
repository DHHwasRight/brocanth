//! [`diff`]: two snapshots into the SQL that turns one into the other.
//!
//! Each node is unchanged, created, dropped, altered in place, or recreated
//! (dropped, then created). A recreate spreads to the nodes that need the
//! recreated one: `DROP FUNCTION` fails while a policy calls it. A node
//! that needs a dropped node is recreated too, so its drop runs before the
//! drop of what it needed. Drops run in the reverse order of the old graph,
//! creates and alters in the order of the new one. `docs/rls.md` §3.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::error::RlsError;
use crate::graph::{self, Node, NodeKey};
use crate::ident::{literal, qualified, quote};
use crate::snapshot::{
    FunctionSpec, PolicySpec, Privilege, RoleSpec, Snapshot, TableSpec, KEYWORDS,
};

/// What a step does to its node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// The node is new.
    Create,
    /// The node is gone, or is dropped to be created again.
    Drop,
    /// The node changed and Postgres changes it in place.
    Alter,
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Action::Create => "create",
            Action::Drop => "drop",
            Action::Alter => "alter",
        })
    }
}

/// One node's statements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// The node.
    pub node: NodeKey,
    /// What happens to it.
    pub action: Action,
    /// The statements, without their `;`.
    pub statements: Vec<String>,
}

/// The steps from one snapshot to another, in the order they run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// The steps.
    pub steps: Vec<Step>,
}

impl Plan {
    /// No step: the two snapshots are equal.
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// One line per step: `create policy posts_tenant on posts`.
    pub fn describe(&self) -> Vec<String> {
        self.steps
            .iter()
            .map(|s| format!("{} {}", s.action, s.node))
            .collect()
    }

    /// The statements, each after a comment that names its step. A control
    /// character in a name is written as `?` in the comment, so a name
    /// cannot end the comment line.
    pub fn sql(&self) -> String {
        let mut out = String::new();
        for step in &self.steps {
            let node = step.node.to_string().replace(char::is_control, "?");
            out.push_str(&format!("-- {} {node}\n", step.action));
            for statement in &step.statements {
                out.push_str(statement);
                out.push_str(";\n");
            }
        }
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fate {
    Same,
    Create,
    Drop,
    Alter,
    Recreate,
}

/// The plan that turns a database described by `old` into one described by
/// `new`: the up part of a migration. [`down`] is the other way.
///
/// # Errors
/// [`RlsError::Definition`] when either snapshot has a cycle.
pub fn diff(old: &Snapshot, new: &Snapshot) -> Result<Plan, RlsError> {
    let old_order = graph::order(old)?;
    let new_order = graph::order(new)?;
    let old_nodes = graph::nodes(old);
    let new_nodes = graph::nodes(new);

    let mut fates: BTreeMap<NodeKey, Fate> = BTreeMap::new();
    for key in old_nodes.keys().chain(new_nodes.keys()) {
        let fate = match (old_nodes.get(key), new_nodes.get(key)) {
            (Some(_), None) => Fate::Drop,
            (None, Some(_)) => Fate::Create,
            (Some(a), Some(b)) if same(a, b) => Fate::Same,
            (Some(a), Some(b)) if recreates(a, b) => Fate::Recreate,
            _ => Fate::Alter,
        };
        fates.insert(key.clone(), fate);
    }

    // A node that needs a recreated or dropped node is recreated: Postgres
    // refuses to drop a function a policy still calls, or a role a policy
    // still names, and drops run before alters. A role is never recreated
    // (that would lose its grants); its membership of a dropped role ends
    // with `DROP ROLE`.
    loop {
        let mut spread = Vec::new();
        for key in old_nodes.keys() {
            if matches!(key, NodeKey::Role(_)) || !matches!(fates[key], Fate::Same | Fate::Alter) {
                continue;
            }
            if graph::deps(key, old)
                .iter()
                .any(|d| matches!(fates.get(d), Some(Fate::Recreate | Fate::Drop)))
            {
                spread.push(key.clone());
            }
        }
        if spread.is_empty() {
            break;
        }
        for key in spread {
            fates.insert(key, Fate::Recreate);
        }
    }
    let dropped_roles: BTreeSet<&str> = fates
        .iter()
        .filter_map(|(k, f)| match (k, f) {
            (NodeKey::Role(name), Fate::Drop) => Some(name.as_str()),
            _ => None,
        })
        .collect();

    let mut steps = Vec::new();
    for key in old_order.iter().rev() {
        if matches!(fates[key], Fate::Drop | Fate::Recreate) {
            steps.push(Step {
                node: key.clone(),
                action: Action::Drop,
                statements: drop_sql(&old_nodes[key]),
            });
        }
    }
    for key in &new_order {
        let (action, statements) = match fates[key] {
            Fate::Create | Fate::Recreate => (Action::Create, create_sql(&new_nodes[key])),
            Fate::Alter => (
                Action::Alter,
                alter_sql(&old_nodes[key], &new_nodes[key], &dropped_roles),
            ),
            Fate::Same | Fate::Drop => continue,
        };
        if statements.is_empty() {
            continue;
        }
        steps.push(Step {
            node: key.clone(),
            action,
            statements,
        });
    }
    Ok(Plan { steps })
}

/// The down part of a migration: [`diff`] from `new` back to `old`, with no
/// `DROP ROLE`.
///
/// A role is cluster-wide. Another database on the server, or a deploy
/// that runs the up part again, may use it, so a rollback leaves it. The
/// next up part finds it and adopts it. `docs/rls.md` §3.
///
/// # Errors
/// [`RlsError::Definition`] when either snapshot has a cycle.
pub fn down(new: &Snapshot, old: &Snapshot) -> Result<Plan, RlsError> {
    let mut plan = diff(new, old)?;
    plan.steps
        .retain(|s| !(s.action == Action::Drop && matches!(s.node, NodeKey::Role(_))));
    Ok(plan)
}

/// Whether two nodes with one key define the same thing. A table node
/// carries its whole `TableSpec`, so it compares only its own fields: a
/// changed policy does not change the table.
fn same(a: &Node<'_>, b: &Node<'_>) -> bool {
    match (a, b) {
        (Node::Table(a), Node::Table(b)) => a.force == b.force,
        (Node::Grant(_, _, a), Node::Grant(_, _, b)) => a == b,
        (Node::Policy(_, _, a), Node::Policy(_, _, b)) => a == b,
        _ => a == b,
    }
}

/// Whether Postgres cannot change `a` into `b` in place.
fn recreates(a: &Node<'_>, b: &Node<'_>) -> bool {
    match (a, b) {
        (Node::Function(a), Node::Function(b)) => a.args != b.args || a.returns != b.returns,
        (Node::Policy(_, _, a), Node::Policy(_, _, b)) => {
            a.kind != b.kind
                || a.command != b.command
                // `ALTER POLICY` sets a clause; it cannot remove one.
                || (a.using.is_some() && b.using.is_none())
                || (a.with_check.is_some() && b.with_check.is_none())
        }
        _ => false,
    }
}

fn table_sql(t: &TableSpec) -> String {
    qualified(t.schema.as_deref(), &t.name)
}

fn function_sql(f: &FunctionSpec) -> String {
    qualified(f.schema.as_deref(), &f.name)
}

fn signature_sql(f: &FunctionSpec) -> String {
    let args: Vec<&str> = f.args.iter().map(|a| a.as_sql()).collect();
    format!("{}({})", function_sql(f), args.join(", "))
}

fn grantee_sql(name: &str) -> String {
    if KEYWORDS.contains(&name) {
        name.to_ascii_uppercase()
    } else {
        quote(name)
    }
}

fn privileges_sql<'a>(privileges: impl IntoIterator<Item = &'a Privilege>) -> String {
    privileges
        .into_iter()
        .map(|p| p.as_sql())
        .collect::<Vec<_>>()
        .join(", ")
}

/// A dollar-quote tag that the body does not contain, so the body cannot
/// end the quote.
fn dollar_tag(body: &str) -> String {
    let mut n = 0;
    loop {
        let tag = if n == 0 {
            "$rls$".to_string()
        } else {
            format!("$rls{n}$")
        };
        if !body.contains(&tag) {
            return tag;
        }
        n += 1;
    }
}

/// A `DO` block that makes the role match `r`, with the least privilege
/// the migration role can have.
///
/// - Missing: `CREATE ROLE … NOLOGIN` with only the attributes that differ
///   from the `CREATE ROLE` defaults, so a migration role without
///   `CREATEDB` can create a role without it.
/// - Present (another database made it, or an earlier run): refused when
///   it is `SUPERUSER` or `BYPASSRLS`, which bypass every policy. Then each
///   attribute that differs is altered, one `ALTER ROLE` each, and only
///   those: an attribute that matches needs no privilege.
///
/// A refused `ALTER` fails the migration with a `help:` line that names
/// the privilege.
fn role_sql(name: &str, r: &RoleSpec) -> String {
    let lit = literal(name);
    let q = quote(name);
    let mut create = format!("CREATE ROLE {q} NOLOGIN");
    for (wanted, default, word) in [
        (r.inherit, true, "NOINHERIT"),
        (r.createdb, false, "CREATEDB"),
        (r.createrole, false, "CREATEROLE"),
    ] {
        if wanted != default {
            create.push(' ');
            create.push_str(word);
        }
    }
    let mut alters = String::new();
    for (column, wanted, on, off) in [
        ("rolcanlogin", false, "LOGIN", "NOLOGIN"),
        ("rolinherit", r.inherit, "INHERIT", "NOINHERIT"),
        ("rolcreatedb", r.createdb, "CREATEDB", "NOCREATEDB"),
        ("rolcreaterole", r.createrole, "CREATEROLE", "NOCREATEROLE"),
    ] {
        alters.push_str(&format!(
            " IF EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = {lit} AND {column} <> {wanted}) THEN ALTER ROLE {q} {}; END IF;",
            if wanted { on } else { off }
        ));
    }
    // `chr(10)` and not a line break, so each statement stays on one line.
    let bypass = format!(
        "{} || chr(10) || {}",
        literal(&format!(
            "the role \"{name}\" exists with SUPERUSER or BYPASSRLS, so it bypasses every policy"
        )),
        literal(&format!(
            "help: use another role name, or ALTER ROLE \"{name}\" NOSUPERUSER NOBYPASSRLS as a superuser"
        )),
    );
    let refused = literal(&format!(
        "the migration role cannot set the attributes of the role \"{name}\": "
    ));
    let help = literal(
        "help: the migration role needs CREATEROLE, ADMIN OPTION on a role it did not create, and CREATEDB to change CREATEDB (docs/rls.md §3); or run the ALTER ROLE as a superuser",
    );
    let body = format!(
        "BEGIN \
         IF NOT EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = {lit}) THEN {create}; \
         ELSIF EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = {lit} AND (rolsuper OR rolbypassrls)) THEN RAISE EXCEPTION USING MESSAGE = {bypass}; \
         END IF;{alters} \
         EXCEPTION WHEN insufficient_privilege THEN RAISE EXCEPTION USING MESSAGE = {refused} || SQLERRM || chr(10) || {help}; \
         END"
    );
    let tag = dollar_tag(&body);
    format!("DO {tag} {body} {tag}")
}

/// A `DO` block that drops the role when nothing on the server depends on
/// it. `pg_shdepend` is cluster-wide: a grant or a policy in another
/// database keeps the role, with a notice, rather than failing the
/// migration.
fn role_drop_sql(name: &str) -> String {
    let lit = literal(name);
    let body = format!(
        "BEGIN \
         IF EXISTS (SELECT FROM pg_catalog.pg_shdepend d JOIN pg_catalog.pg_roles r ON d.refclassid = 'pg_catalog.pg_authid'::regclass AND d.refobjid = r.oid WHERE r.rolname = {lit}) \
         THEN RAISE NOTICE 'role % kept: objects on this server still depend on it', {lit}; \
         ELSE DROP ROLE IF EXISTS {q}; \
         END IF; \
         END",
        q = quote(name),
    );
    let tag = dollar_tag(&body);
    format!("DO {tag} {body} {tag}")
}

fn create_sql(node: &Node<'_>) -> Vec<String> {
    match node {
        Node::Role(name, r) => {
            let mut out = vec![role_sql(name, r)];
            for member in &r.members {
                out.push(format!("GRANT {} TO {}", quote(name), quote(member)));
            }
            out
        }
        Node::Function(f) => {
            let mut out = vec![function_create(f)];
            let grantees = execute_grantees(f);
            if !grantees.contains("public") {
                out.push(format!(
                    "REVOKE EXECUTE ON FUNCTION {} FROM PUBLIC",
                    signature_sql(f)
                ));
            }
            for grantee in grantees.iter().filter(|g| *g != "public") {
                out.push(format!(
                    "GRANT EXECUTE ON FUNCTION {} TO {}",
                    signature_sql(f),
                    grantee_sql(grantee)
                ));
            }
            out
        }
        Node::Table(t) => {
            let mut out = vec![format!(
                "ALTER TABLE {} ENABLE ROW LEVEL SECURITY",
                table_sql(t)
            )];
            if t.force {
                out.push(format!(
                    "ALTER TABLE {} FORCE ROW LEVEL SECURITY",
                    table_sql(t)
                ));
            }
            out
        }
        Node::Grant(t, grantee, privileges) => vec![format!(
            "GRANT {} ON {} TO {}",
            privileges_sql(privileges.iter()),
            table_sql(t),
            grantee_sql(grantee)
        )],
        Node::Policy(t, name, p) => vec![policy_create(t, name, p)],
    }
}

/// Who may call the function. A `SECURITY INVOKER` function keeps the
/// default, `PUBLIC`. A `SECURITY DEFINER` one runs with its owner's
/// rights, so only the roles it names may call it.
fn execute_grantees(f: &FunctionSpec) -> BTreeSet<String> {
    let mut out = f.execute.clone();
    if f.definer.is_none() {
        out.insert("public".to_string());
    }
    out
}

fn function_create(f: &FunctionSpec) -> String {
    let security = match &f.definer {
        Some(path) => {
            // `pg_temp` last: without it, the caller's temporary schema is
            // searched first, and a temporary table could stand in for one
            // the body reads.
            let mut path: Vec<String> = path.iter().map(|s| quote(s)).collect();
            path.push("pg_temp".to_string());
            format!(" SECURITY DEFINER SET search_path = {}", path.join(", "))
        }
        None => String::new(),
    };
    let tag = dollar_tag(&f.body);
    format!(
        "CREATE OR REPLACE FUNCTION {} RETURNS {} LANGUAGE sql {}{security} AS {tag} SELECT {} {tag}",
        signature_sql(f),
        f.returns.as_sql(),
        f.volatility.as_sql(),
        f.body,
    )
}

fn to_sql(p: &PolicySpec) -> String {
    p.to.iter()
        .map(|r| grantee_sql(r))
        .collect::<Vec<_>>()
        .join(", ")
}

fn policy_create(t: &TableSpec, name: &str, p: &PolicySpec) -> String {
    let mut sql = format!(
        "CREATE POLICY {} ON {} AS {} FOR {} TO {}",
        quote(name),
        table_sql(t),
        match p.kind {
            crate::snapshot::PolicyKind::Permissive => "PERMISSIVE",
            crate::snapshot::PolicyKind::Restrictive => "RESTRICTIVE",
        },
        p.command.as_sql(),
        to_sql(p),
    );
    clauses(&mut sql, p);
    sql
}

fn clauses(sql: &mut String, p: &PolicySpec) {
    if let Some(using) = &p.using {
        sql.push_str(&format!(" USING ({using})"));
    }
    if let Some(check) = &p.with_check {
        sql.push_str(&format!(" WITH CHECK ({check})"));
    }
}

fn drop_sql(node: &Node<'_>) -> Vec<String> {
    match node {
        Node::Role(name, _) => vec![role_drop_sql(name)],
        Node::Function(f) => vec![format!("DROP FUNCTION IF EXISTS {}", signature_sql(f))],
        Node::Table(t) => {
            let mut out = Vec::new();
            if t.force {
                out.push(format!(
                    "ALTER TABLE IF EXISTS {} NO FORCE ROW LEVEL SECURITY",
                    table_sql(t)
                ));
            }
            out.push(format!(
                "ALTER TABLE IF EXISTS {} DISABLE ROW LEVEL SECURITY",
                table_sql(t)
            ));
            out
        }
        Node::Grant(t, grantee, privileges) => vec![format!(
            "REVOKE {} ON {} FROM {}",
            privileges_sql(privileges.iter()),
            table_sql(t),
            grantee_sql(grantee)
        )],
        Node::Policy(t, name, _) => vec![format!(
            "DROP POLICY IF EXISTS {} ON {}",
            quote(name),
            table_sql(t)
        )],
    }
}

fn alter_sql(old: &Node<'_>, new: &Node<'_>, dropped_roles: &BTreeSet<&str>) -> Vec<String> {
    match (old, new) {
        (Node::Role(_, a), Node::Role(name, b)) => {
            let mut out = Vec::new();
            if (a.inherit, a.createdb, a.createrole) != (b.inherit, b.createdb, b.createrole) {
                out.push(role_sql(name, b));
            }
            // A member that is dropped in this plan leaves with its role.
            for gone in a
                .members
                .difference(&b.members)
                .filter(|m| !dropped_roles.contains(m.as_str()))
            {
                out.push(format!("REVOKE {} FROM {}", quote(name), quote(gone)));
            }
            for added in b.members.difference(&a.members) {
                out.push(format!("GRANT {} TO {}", quote(name), quote(added)));
            }
            out
        }
        (Node::Function(a), Node::Function(f)) => {
            let mut out = Vec::new();
            if (&a.volatility, &a.definer, &a.body) != (&f.volatility, &f.definer, &f.body) {
                out.push(function_create(f));
            }
            let (was, now) = (execute_grantees(a), execute_grantees(f));
            for gone in was.difference(&now) {
                out.push(format!(
                    "REVOKE EXECUTE ON FUNCTION {} FROM {}",
                    signature_sql(f),
                    grantee_sql(gone)
                ));
            }
            for added in now.difference(&was) {
                out.push(format!(
                    "GRANT EXECUTE ON FUNCTION {} TO {}",
                    signature_sql(f),
                    grantee_sql(added)
                ));
            }
            out
        }
        (Node::Table(a), Node::Table(b)) => {
            if a.force == b.force {
                return Vec::new();
            }
            vec![format!(
                "ALTER TABLE {} {} ROW LEVEL SECURITY",
                table_sql(b),
                if b.force { "FORCE" } else { "NO FORCE" }
            )]
        }
        (Node::Grant(_, _, a), Node::Grant(t, grantee, b)) => {
            let mut out = Vec::new();
            let gone: Vec<&Privilege> = a.difference(b).collect();
            if !gone.is_empty() {
                out.push(format!(
                    "REVOKE {} ON {} FROM {}",
                    privileges_sql(gone),
                    table_sql(t),
                    grantee_sql(grantee)
                ));
            }
            let added: Vec<&Privilege> = b.difference(a).collect();
            if !added.is_empty() {
                out.push(format!(
                    "GRANT {} ON {} TO {}",
                    privileges_sql(added),
                    table_sql(t),
                    grantee_sql(grantee)
                ));
            }
            out
        }
        (Node::Policy(..), Node::Policy(t, name, p)) => {
            let mut sql = format!(
                "ALTER POLICY {} ON {} TO {}",
                quote(name),
                table_sql(t),
                to_sql(p)
            );
            clauses(&mut sql, p);
            vec![sql]
        }
        // A key names one kind of node, so the two sides are the same kind.
        _ => create_sql(new),
    }
}
