//! The dependency graph: each definition is a node, each edge points at a
//! node it needs. `docs/rls.md` §2.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::error::RlsError;
use crate::snapshot::{FunctionSpec, PolicySpec, Privilege, RoleSpec, Snapshot, TableSpec};

/// A node's identity. The variant order is the tie-break between nodes
/// with no edge: roles, functions, tables, grants, policies.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NodeKey {
    /// A managed role, by name.
    Role(String),
    /// A function, by key.
    Function(String),
    /// A table's `ENABLE`/`FORCE`, by key.
    Table(String),
    /// The privileges of a grantee on a table: table key, grantee.
    Grant(String, String),
    /// A policy: table key, name.
    Policy(String, String),
}

impl fmt::Display for NodeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NodeKey::Role(name) => write!(f, "role {name}"),
            NodeKey::Function(key) => write!(f, "function {key}"),
            NodeKey::Table(key) => write!(f, "table {key}"),
            NodeKey::Grant(table, grantee) => write!(f, "grant on {table} to {grantee}"),
            NodeKey::Policy(table, name) => write!(f, "policy {name} on {table}"),
        }
    }
}

/// A node's definition, borrowed from its snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Node<'a> {
    Role(&'a str, &'a RoleSpec),
    Function(&'a FunctionSpec),
    Table(&'a TableSpec),
    Grant(&'a TableSpec, &'a str, &'a BTreeSet<Privilege>),
    Policy(&'a TableSpec, &'a str, &'a PolicySpec),
}

/// Every node of a snapshot.
pub(crate) fn nodes(s: &Snapshot) -> BTreeMap<NodeKey, Node<'_>> {
    let mut out = BTreeMap::new();
    for (name, role) in &s.roles {
        out.insert(NodeKey::Role(name.clone()), Node::Role(name, role));
    }
    for (key, function) in &s.functions {
        out.insert(NodeKey::Function(key.clone()), Node::Function(function));
    }
    for (key, table) in &s.tables {
        out.insert(NodeKey::Table(key.clone()), Node::Table(table));
        for (grantee, privileges) in &table.grants {
            out.insert(
                NodeKey::Grant(key.clone(), grantee.clone()),
                Node::Grant(table, grantee, privileges),
            );
        }
        for (name, policy) in &table.policies {
            out.insert(
                NodeKey::Policy(key.clone(), name.clone()),
                Node::Policy(table, name, policy),
            );
        }
    }
    out
}

/// The nodes `key` needs, among the nodes of `s`. A `Role::existing` and a
/// keyword such as `public` are not nodes.
pub(crate) fn deps(key: &NodeKey, s: &Snapshot) -> Vec<NodeKey> {
    let role = |name: &String| {
        s.roles
            .contains_key(name)
            .then(|| NodeKey::Role(name.clone()))
    };
    match key {
        NodeKey::Table(_) => Vec::new(),
        // `GRANT "role" TO "member"` needs the member to exist.
        NodeKey::Role(name) => s
            .roles
            .get(name)
            .map(|r| r.members.iter().filter_map(role).collect())
            .unwrap_or_default(),
        NodeKey::Function(k) => s
            .functions
            .get(k)
            .map(|f| {
                let mut out: Vec<NodeKey> = f.uses.iter().cloned().map(NodeKey::Function).collect();
                out.extend(f.execute.iter().filter_map(role));
                out
            })
            .unwrap_or_default(),
        NodeKey::Grant(table, grantee) => {
            let mut out = vec![NodeKey::Table(table.clone())];
            out.extend(role(grantee));
            out
        }
        NodeKey::Policy(table, name) => {
            let mut out = vec![NodeKey::Table(table.clone())];
            if let Some(policy) = s.tables.get(table).and_then(|t| t.policies.get(name)) {
                out.extend(policy.to.iter().filter_map(role));
                out.extend(policy.uses.iter().cloned().map(NodeKey::Function));
            }
            out
        }
    }
}

/// The nodes of `s`, each after every node it needs. Ties break by
/// [`NodeKey`]'s order, so the result is the same on every run.
///
/// # Errors
/// [`RlsError::Definition`] naming the nodes of a cycle: functions that
/// call each other, or roles that are members of each other.
pub(crate) fn order(s: &Snapshot) -> Result<Vec<NodeKey>, RlsError> {
    let all = nodes(s);
    let mut waiting: BTreeMap<NodeKey, usize> = BTreeMap::new();
    let mut needed_by: BTreeMap<NodeKey, Vec<NodeKey>> = BTreeMap::new();
    for key in all.keys() {
        let deps: BTreeSet<NodeKey> = deps(key, s)
            .into_iter()
            .filter(|d| all.contains_key(d))
            .collect();
        waiting.insert(key.clone(), deps.len());
        for dep in deps {
            needed_by.entry(dep).or_default().push(key.clone());
        }
    }
    let mut ready: BTreeSet<NodeKey> = waiting
        .iter()
        .filter(|(_, n)| **n == 0)
        .map(|(k, _)| k.clone())
        .collect();
    let mut out = Vec::with_capacity(all.len());
    while let Some(next) = ready.pop_first() {
        waiting.remove(&next);
        for user in needed_by.get(&next).into_iter().flatten() {
            if let Some(n) = waiting.get_mut(user) {
                *n -= 1;
                if *n == 0 {
                    ready.insert(user.clone());
                }
            }
        }
        out.push(next);
    }
    if !waiting.is_empty() {
        let names: Vec<String> = waiting.keys().map(ToString::to_string).collect();
        return Err(RlsError::definition(
            format!("these definitions need each other: {}", names.join(", ")),
            "break the cycle: a function cannot call itself through another, and a role cannot be a member of itself through another",
        ));
    }
    Ok(out)
}
