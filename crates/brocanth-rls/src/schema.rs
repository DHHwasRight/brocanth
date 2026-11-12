//! [`Schema`]: every definition, checked as a whole.

use std::collections::{BTreeMap, BTreeSet};

use brocanth_db::DbIdentity;

use crate::def::{Function, Grantee, Policy, Role, Table};
use crate::error::RlsError;
use crate::expr::{SqlExpr, Uses};
use crate::ident::{self, SqlType};
use crate::snapshot::{Command, FunctionSpec, PolicySpec, RoleSpec, Snapshot, TableSpec, KEYWORDS};

/// The roles, tables, functions and policies of one database.
///
/// Build it in any order. [`Schema::snapshot`] checks it and resolves the
/// order; [`crate::diff`] turns two snapshots into SQL.
#[derive(Debug, Clone, Default)]
pub struct Schema {
    roles: Vec<Role>,
    tables: Vec<Table>,
    functions: Vec<Function>,
    policies: Vec<Policy>,
}

impl Schema {
    /// No definition.
    pub fn new() -> Schema {
        Schema::default()
    }

    /// Adds a role: a managed one (`Role::new`) or one made elsewhere
    /// (`Role::existing`). A policy or a grant may name only a role added
    /// here.
    pub fn role(mut self, role: Role) -> Schema {
        self.roles.push(role);
        self
    }

    /// Adds a table: row-level security on, and its grants.
    pub fn table(mut self, table: Table) -> Schema {
        self.tables.push(table);
        self
    }

    /// Adds a function.
    pub fn function(mut self, function: Function) -> Schema {
        self.functions.push(function);
        self
    }

    /// Adds a policy. Its table must be added too.
    pub fn policy(mut self, policy: Policy) -> Schema {
        self.policies.push(policy);
        self
    }

    /// Adds policies, such as the ones [`crate::crud`] returns.
    pub fn policies(mut self, policies: impl IntoIterator<Item = Policy>) -> Schema {
        self.policies.extend(policies);
        self
    }

    /// The settings the expressions read, by key, with the type each reads.
    pub fn settings(&self) -> BTreeMap<String, SqlType> {
        let mut all = BTreeMap::new();
        for e in self.exprs() {
            for (k, t) in &e.used().settings {
                all.entry(k.clone()).or_insert_with(|| t.clone());
            }
        }
        all
    }

    /// Checks an identity against the schema: each key is a setting a
    /// policy reads, and the role is a role of the schema. A typo in a key
    /// would otherwise read as `NULL`, and admit no row with no error.
    ///
    /// # Errors
    /// [`RlsError::Definition`] naming the unknown key or role.
    pub fn check_identity(&self, identity: &DbIdentity) -> Result<(), RlsError> {
        let settings = self.settings();
        for (key, _) in identity.settings() {
            if !settings.contains_key(key) {
                let known: Vec<&str> = settings.keys().map(String::as_str).collect();
                return Err(RlsError::definition(
                    format!("the identity sets `{key}`, which no policy reads"),
                    format!(
                        "set one of: {}; or use `DbIdentity::setting` with the policy's `Setting`",
                        known.join(", ")
                    ),
                ));
            }
        }
        if let Some(role) = identity.role_name() {
            if !self.roles.iter().any(|r| r.name == role) {
                return Err(RlsError::definition(
                    format!("the identity's role `{role}` is not a role of the schema"),
                    format!("add `Role::new(\"{role}\")` or `Role::existing(\"{role}\")`"),
                ));
            }
        }
        Ok(())
    }

    fn exprs(&self) -> impl Iterator<Item = &SqlExpr> {
        self.policies
            .iter()
            .flat_map(|p| p.using.iter().chain(p.with_check.iter()))
            .chain(self.functions.iter().filter_map(|f| f.body.as_ref()))
    }

    /// The schema as data, checked: every name, every reference, no
    /// function cycle.
    ///
    /// # Errors
    /// [`RlsError::Definition`], with a `help:` line, for the first problem.
    pub fn snapshot(&self) -> Result<Snapshot, RlsError> {
        let mut snapshot = Snapshot::default();
        let mut known_roles: BTreeSet<&str> = BTreeSet::new();

        for role in &self.roles {
            ident::check("role", &role.name)?;
            if KEYWORDS.contains(&role.name.to_ascii_lowercase().as_str()) {
                return Err(RlsError::definition(
                    format!("`{}` is a keyword, not a role name", role.name),
                    "name the role otherwise; for every role, use `Grantee::Public`",
                ));
            }
            if !known_roles.insert(&role.name) {
                return Err(RlsError::definition(
                    format!("the role `{}` is added twice", role.name),
                    "add each role once",
                ));
            }
            for member in &role.members {
                ident::check("role", member)?;
            }
            if !role.existing {
                snapshot.roles.insert(
                    role.name.clone(),
                    RoleSpec {
                        inherit: role.inherit,
                        createdb: role.createdb,
                        createrole: role.createrole,
                        members: role.members.clone(),
                    },
                );
            } else if !role.members.is_empty() {
                return Err(RlsError::definition(
                    format!("the existing role `{}` has members", role.name),
                    "grant an existing role by hand; `member` is for a role this crate creates",
                ));
            }
        }
        let role_ok = |name: &str| known_roles.contains(name) || KEYWORDS.contains(&name);

        for function in &self.functions {
            if let Some(schema) = &function.schema {
                ident::check("schema", schema)?;
            }
            ident::check("function", &function.name)?;
            for ty in function.args.iter().chain([&function.returns]) {
                ty.check()?;
            }
            for schema in function.definer.iter().flatten() {
                ident::check("schema", schema)?;
            }
            let key = function.key();
            let Some(body) = &function.body else {
                return Err(RlsError::definition(
                    format!("the function `{key}` has no body"),
                    "give it one: `.body(SqlExpr::…)`",
                ));
            };
            problems(&format!("the function `{key}`"), body)?;
            let spec = FunctionSpec {
                schema: function.schema.clone(),
                name: function.name.clone(),
                // `Custom("bigint")` is `BigInt`, as the JSON reads it back.
                args: function
                    .args
                    .iter()
                    .cloned()
                    .map(SqlType::normalized)
                    .collect(),
                returns: function.returns.clone().normalized(),
                volatility: function.volatility,
                definer: function.definer.clone(),
                body: body.to_sql(),
                uses: body.used().functions.clone(),
                execute: function.execute.clone(),
            };
            if let Some(role) = function
                .execute
                .iter()
                .find(|r| !role_ok(r) || (KEYWORDS.contains(&r.as_str()) && *r != "public"))
            {
                return Err(RlsError::definition(
                    format!("the function `{key}` grants EXECUTE to `{role}`, which is not a role of the schema"),
                    format!("add `Role::new(\"{role}\")` or `Role::existing(\"{role}\")` to the schema"),
                ));
            }
            if snapshot.functions.insert(key.clone(), spec).is_some() {
                return Err(RlsError::definition(
                    format!("the function `{key}` is added twice"),
                    "add each function once; overloads are not supported",
                ));
            }
        }

        for table in &self.tables {
            if let Some(schema) = &table.schema {
                ident::check("schema", schema)?;
            }
            ident::check("table", &table.name)?;
            let key = table.key();
            let mut grants: BTreeMap<String, BTreeSet<_>> = BTreeMap::new();
            for (grantee, privileges) in &table.grants {
                if !role_ok(grantee)
                    || (KEYWORDS.contains(&grantee.as_str()) && grantee != "public")
                {
                    return Err(RlsError::definition(
                        format!("the table `{key}` grants to `{grantee}`, which is not a role of the schema"),
                        format!("add `Role::new(\"{grantee}\")` or `Role::existing(\"{grantee}\")` to the schema"),
                    ));
                }
                grants
                    .entry(grantee.clone())
                    .or_default()
                    .extend(privileges.iter().copied());
            }
            grants.retain(|_, p| !p.is_empty());
            let spec = TableSpec {
                schema: table.schema.clone(),
                name: table.name.clone(),
                force: table.force,
                grants,
                policies: BTreeMap::new(),
            };
            if snapshot.tables.insert(key.clone(), spec).is_some() {
                return Err(RlsError::definition(
                    format!("the table `{key}` is added twice"),
                    "add each table once, with all its grants",
                ));
            }
        }

        for policy in &self.policies {
            ident::check("policy", &policy.name)?;
            let what = format!("the policy `{}` on `{}`", policy.name, policy.table);
            let mut to: Vec<String> = if policy.to.is_empty() {
                vec![Grantee::Public.stored()]
            } else {
                policy.to.iter().map(Grantee::stored).collect()
            };
            to.sort();
            to.dedup();
            for role in &to {
                if !role_ok(role) {
                    return Err(RlsError::definition(
                        format!("{what} names the role `{role}`, which is not a role of the schema"),
                        format!("add `Role::new(\"{role}\")` or `Role::existing(\"{role}\")` to the schema"),
                    ));
                }
            }
            let mut uses = BTreeSet::new();
            for expr in policy.using.iter().chain(policy.with_check.iter()) {
                problems(&what, expr)?;
                uses.extend(expr.used().functions.iter().cloned());
            }
            if let Some(missing) = uses.iter().find(|f| !snapshot.functions.contains_key(*f)) {
                return Err(RlsError::definition(
                    format!("{what} calls the function `{missing}`, which the schema does not add"),
                    "add the function with `Schema::function`",
                ));
            }
            // Postgres refuses these clauses for these commands.
            let refused = match policy.command {
                Command::Select | Command::Delete if policy.with_check.is_some() => Some((
                    "WITH CHECK",
                    "a SELECT or DELETE policy filters rows with `using` only",
                )),
                Command::Insert if policy.using.is_some() => Some((
                    "USING",
                    "an INSERT policy checks the new row with `with_check` only",
                )),
                _ => None,
            };
            if let Some((clause, help)) = refused {
                return Err(RlsError::definition(
                    format!("{what} is FOR {} and has {clause}", policy.command),
                    format!("remove it: {help}"),
                ));
            }
            // A SECURITY DEFINER function runs with its owner's rights, so
            // only granted roles may call it, and each role the policy
            // applies to must be one of them.
            for called in &uses {
                let Some(f) = snapshot.functions.get(called) else {
                    continue;
                };
                if f.definer.is_none() || f.execute.contains("public") {
                    continue;
                }
                if let Some(role) = to.iter().find(|r| !f.execute.contains(*r)) {
                    return Err(RlsError::definition(
                        format!("{what} calls the SECURITY DEFINER function `{called}` for `{role}`, which may not execute it"),
                        if KEYWORDS.contains(&role.as_str()) {
                            "add `.grant_execute_public()` to the function, or name roles in the policy's `to`".to_string()
                        } else {
                            format!("add `.grant_execute(&Role::new(\"{role}\"))` to the function")
                        },
                    ));
                }
            }
            let spec = PolicySpec {
                kind: policy.kind,
                command: policy.command,
                to,
                using: policy.using.as_ref().map(SqlExpr::to_sql),
                with_check: policy.with_check.as_ref().map(SqlExpr::to_sql),
                uses,
            };
            let Some(table) = snapshot.tables.get_mut(&policy.table) else {
                return Err(RlsError::definition(
                    format!("{what}: the schema does not add the table"),
                    format!(
                        "add `Table::named(\"{}\")` with `Schema::table`",
                        policy.table
                    ),
                ));
            };
            if table.policies.insert(policy.name.clone(), spec).is_some() {
                return Err(RlsError::definition(
                    format!("{what} is added twice"),
                    "give each policy on a table its own name",
                ));
            }
        }

        // Every function a function calls is added, and no call goes round.
        for (key, function) in &snapshot.functions {
            if let Some(missing) = function
                .uses
                .iter()
                .find(|f| !snapshot.functions.contains_key(*f))
            {
                return Err(RlsError::definition(
                    format!(
                        "the function `{key}` calls `{missing}`, which the schema does not add"
                    ),
                    "add the function with `Schema::function`",
                ));
            }
        }
        crate::graph::order(&snapshot)?;

        // One setting, one type, across every expression.
        // Each expression's own problems were refused above, so a problem
        // here is a setting read as two types by two expressions.
        let mut all = Uses::default();
        for expr in self.exprs() {
            all.merge(expr.used());
        }
        if let Some(problem) = all.problems.first() {
            return Err(RlsError::definition(
                problem.clone(),
                "read each setting as one type in every policy",
            ));
        }
        Ok(snapshot)
    }

    /// The SQL that creates every definition on a database that has none.
    ///
    /// # Errors
    /// What [`Schema::snapshot`] refuses.
    pub fn create_sql(&self) -> Result<String, RlsError> {
        let plan = crate::diff::diff(&Snapshot::default(), &self.snapshot()?)?;
        Ok(plan.sql())
    }
}

fn problems(what: &str, expr: &SqlExpr) -> Result<(), RlsError> {
    let Some(problem) = expr.used().problems.first() else {
        return Ok(());
    };
    let (message, help) = problem
        .split_once("\nhelp: ")
        .unwrap_or((problem, "read each setting as one type in every policy"));
    Err(RlsError::definition(format!("{what}: {message}"), help))
}
