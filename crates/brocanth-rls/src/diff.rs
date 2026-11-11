//! [`diff`]: two snapshots into the SQL that turns one into the other.
//!
//! Each node is unchanged, created, dropped, altered in place, or recreated
//! (dropped, then created). A recreate spreads to the nodes that need the
//! recreated one: `DROP FUNCTION` fails while a policy calls it. Drops run
//! in the reverse order of the old graph, creates and alters in the order of
//! the new one. `docs/rls.md` §3.

use std::collections::BTreeMap;
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

    /// The statements, each after a comment that names its step.
    pub fn sql(&self) -> String {
        let mut out = String::new();
        for step in &self.steps {
            out.push_str(&format!("-- {} {}\n", step.action, step.node));
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
/// `new`. The down migration is `diff(new, old)`.
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

    // A node that needs a recreated node is recreated too: Postgres refuses
    // to drop a function a policy still calls.
    loop {
        let mut spread = Vec::new();
        for key in old_nodes.keys() {
            if !matches!(fates[key], Fate::Same | Fate::Alter) {
                continue;
            }
            if graph::deps(key, old)
                .iter()
                .any(|d| fates.get(d) == Some(&Fate::Recreate))
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
            Fate::Alter => (Action::Alter, alter_sql(&old_nodes[key], &new_nodes[key])),
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

fn role_attributes(r: &RoleSpec) -> String {
    format!(
        "NOLOGIN {} {} {}",
        if r.inherit { "INHERIT" } else { "NOINHERIT" },
        if r.createdb { "CREATEDB" } else { "NOCREATEDB" },
        if r.createrole {
            "CREATEROLE"
        } else {
            "NOCREATEROLE"
        },
    )
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

fn create_sql(node: &Node<'_>) -> Vec<String> {
    match node {
        Node::Role(name, r) => {
            // Roles are cluster-wide: the role may exist from another
            // database. Create it when it is missing, then set every
            // attribute, so both paths end in the same role.
            let mut out = vec![
                format!(
                    "DO {tag} BEGIN IF NOT EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = {lit}) THEN CREATE ROLE {q} NOLOGIN; END IF; END {tag}",
                    tag = dollar_tag(name),
                    lit = literal(name),
                    q = quote(name),
                ),
                format!("ALTER ROLE {} WITH {}", quote(name), role_attributes(r)),
            ];
            for member in &r.members {
                out.push(format!("GRANT {} TO {}", quote(name), quote(member)));
            }
            out
        }
        Node::Function(f) => vec![function_create(f)],
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

fn function_create(f: &FunctionSpec) -> String {
    let args: Vec<&str> = f.args.iter().map(|a| a.as_sql()).collect();
    let security = match &f.definer {
        Some(path) => {
            let path: Vec<String> = path.iter().map(|s| quote(s)).collect();
            format!(" SECURITY DEFINER SET search_path = {}", path.join(", "))
        }
        None => String::new(),
    };
    let tag = dollar_tag(&f.body);
    format!(
        "CREATE OR REPLACE FUNCTION {}({}) RETURNS {} LANGUAGE sql {}{security} AS {tag} SELECT {} {tag}",
        function_sql(f),
        args.join(", "),
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
        Node::Role(name, _) => vec![format!("DROP ROLE IF EXISTS {}", quote(name))],
        Node::Function(f) => {
            let args: Vec<&str> = f.args.iter().map(|a| a.as_sql()).collect();
            vec![format!(
                "DROP FUNCTION IF EXISTS {}({})",
                function_sql(f),
                args.join(", ")
            )]
        }
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

fn alter_sql(old: &Node<'_>, new: &Node<'_>) -> Vec<String> {
    match (old, new) {
        (Node::Role(_, a), Node::Role(name, b)) => {
            let mut out = Vec::new();
            if (a.inherit, a.createdb, a.createrole) != (b.inherit, b.createdb, b.createrole) {
                out.push(format!(
                    "ALTER ROLE {} WITH {}",
                    quote(name),
                    role_attributes(b)
                ));
            }
            for gone in a.members.difference(&b.members) {
                out.push(format!("REVOKE {} FROM {}", quote(name), quote(gone)));
            }
            for added in b.members.difference(&a.members) {
                out.push(format!("GRANT {} TO {}", quote(name), quote(added)));
            }
            out
        }
        (Node::Function(_), Node::Function(f)) => vec![function_create(f)],
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
