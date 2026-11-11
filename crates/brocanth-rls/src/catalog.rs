//! [`drift`]: the live catalog against a snapshot, for `rls check`.
//!
//! It compares structure only: roles and their attributes and members,
//! `relrowsecurity` and `relforcerowsecurity`, policy names, `permissive`,
//! `cmd`, `roles`, table privileges, and that each function exists. It does
//! not compare `USING` and `WITH CHECK` text: Postgres rewrites it
//! (`docs/rls.md` §3).

use std::collections::BTreeSet;

use brocanth_db::sqlx;
use brocanth_db::{Db, DbError};

use crate::error::RlsError;
use crate::ident::{qualified, quote};
use crate::snapshot::{PolicyKind, Snapshot, KEYWORDS};

/// Each difference between the database and `snapshot`, one line each. An
/// empty list means the database has every definition.
///
/// # Errors
/// [`RlsError::Db`] when a catalog query failed.
pub async fn drift(db: &Db, snapshot: &Snapshot) -> Result<Vec<String>, RlsError> {
    let pool = db.pool();
    let q = |e: sqlx::Error| RlsError::Db(DbError::from(e));
    let mut out = Vec::new();

    for (name, spec) in &snapshot.roles {
        let row: Option<(bool, bool, bool, bool)> = sqlx::query_as(
            "SELECT rolinherit, rolcreatedb, rolcreaterole, rolcanlogin FROM pg_catalog.pg_roles WHERE rolname = $1",
        )
        .bind(name)
        .fetch_optional(pool)
        .await
        .map_err(q)?;
        let Some((inherit, createdb, createrole, login)) = row else {
            out.push(format!("role {name}: missing"));
            continue;
        };
        if (inherit, createdb, createrole, login)
            != (spec.inherit, spec.createdb, spec.createrole, false)
        {
            out.push(format!(
                "role {name}: inherit={inherit} createdb={createdb} createrole={createrole} login={login}; the schema says inherit={} createdb={} createrole={} login=false",
                spec.inherit, spec.createdb, spec.createrole
            ));
        }
        let members: Vec<(String,)> = sqlx::query_as(
            "SELECT m.rolname::text FROM pg_catalog.pg_auth_members a \
             JOIN pg_catalog.pg_roles r ON r.oid = a.roleid \
             JOIN pg_catalog.pg_roles m ON m.oid = a.member WHERE r.rolname = $1",
        )
        .bind(name)
        .fetch_all(pool)
        .await
        .map_err(q)?;
        let live: BTreeSet<String> = members.into_iter().map(|(m,)| m).collect();
        for missing in spec.members.difference(&live) {
            out.push(format!("role {name}: {missing} is not a member"));
        }
    }

    for (key, f) in &snapshot.functions {
        let args: Vec<&str> = f.args.iter().map(|a| a.as_sql()).collect();
        let signature = format!(
            "{}({})",
            qualified(f.schema.as_deref(), &f.name),
            args.join(", ")
        );
        let (found,): (bool,) = sqlx::query_as("SELECT to_regprocedure($1) IS NOT NULL")
            .bind(&signature)
            .fetch_one(pool)
            .await
            .map_err(q)?;
        if !found {
            out.push(format!("function {key}: missing"));
        }
    }

    for (key, t) in &snapshot.tables {
        let name = qualified(t.schema.as_deref(), &t.name);
        let row: Option<(bool, bool)> = sqlx::query_as(
            "SELECT relrowsecurity, relforcerowsecurity FROM pg_catalog.pg_class WHERE oid = to_regclass($1)",
        )
        .bind(&name)
        .fetch_optional(pool)
        .await
        .map_err(q)?;
        let Some((enabled, forced)) = row else {
            out.push(format!("table {key}: missing"));
            continue;
        };
        if !enabled {
            out.push(format!("table {key}: row-level security is off"));
        }
        if forced != t.force {
            out.push(format!(
                "table {key}: FORCE is {}, the schema says {}",
                on(forced),
                on(t.force)
            ));
        }

        for (grantee, privileges) in &t.grants {
            for p in privileges {
                let who = if grantee == "public" {
                    "public".to_string()
                } else {
                    quote(grantee)
                };
                // `has_table_privilege` takes the role as text, so the name
                // is passed quoted, as a literal-free bind.
                let (has,): (bool,) = sqlx::query_as(
                    "SELECT EXISTS (SELECT 1 FROM aclexplode((SELECT relacl FROM pg_catalog.pg_class WHERE oid = to_regclass($1))) a \
                     WHERE a.privilege_type = $2 AND a.grantee = CASE WHEN $3 = 'public' THEN 0 ELSE to_regrole($3)::oid END)",
                )
                .bind(&name)
                .bind(p.as_sql())
                .bind(&who)
                .fetch_one(pool)
                .await
                .map_err(q)?;
                if !has {
                    out.push(format!("table {key}: {grantee} lacks {}", p.as_sql()));
                }
            }
        }

        let policies: Vec<(String, String, String, Vec<String>)> = sqlx::query_as(
            "SELECT p.polname::text, \
                    CASE WHEN p.polpermissive THEN 'PERMISSIVE' ELSE 'RESTRICTIVE' END, \
                    CASE p.polcmd WHEN 'r' THEN 'SELECT' WHEN 'a' THEN 'INSERT' WHEN 'w' THEN 'UPDATE' WHEN 'd' THEN 'DELETE' ELSE 'ALL' END, \
                    ARRAY(SELECT CASE WHEN r = 0 THEN 'public' ELSE pg_catalog.pg_get_userbyid(r)::text END FROM unnest(p.polroles) r ORDER BY 1) \
             FROM pg_catalog.pg_policy p WHERE p.polrelid = to_regclass($1)",
        )
        .bind(&name)
        .fetch_all(pool)
        .await
        .map_err(q)?;
        let live: BTreeSet<&str> = policies.iter().map(|p| p.0.as_str()).collect();
        for (policy, spec) in &t.policies {
            let Some((_, kind, cmd, roles)) = policies.iter().find(|p| &p.0 == policy) else {
                out.push(format!("policy {policy} on {key}: missing"));
                continue;
            };
            let want_kind = match spec.kind {
                PolicyKind::Permissive => "PERMISSIVE",
                PolicyKind::Restrictive => "RESTRICTIVE",
            };
            if kind != want_kind {
                out.push(format!(
                    "policy {policy} on {key}: {kind}, the schema says {want_kind}"
                ));
            }
            if cmd != spec.command.as_sql() {
                out.push(format!(
                    "policy {policy} on {key}: FOR {cmd}, the schema says FOR {}",
                    spec.command
                ));
            }
            // `TO CURRENT_USER` is stored as the role it named when the
            // policy was made, so only a list of names compares.
            let comparable = spec
                .to
                .iter()
                .all(|r| r == "public" || !KEYWORDS.contains(&r.as_str()));
            if comparable && roles != &spec.to {
                out.push(format!(
                    "policy {policy} on {key}: TO {}, the schema says TO {}",
                    roles.join(", "),
                    spec.to.join(", ")
                ));
            }
        }
        for extra in live.iter().filter(|p| !t.policies.contains_key(**p)) {
            out.push(format!("policy {extra} on {key}: not in the schema"));
        }
    }
    Ok(out)
}

fn on(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}
