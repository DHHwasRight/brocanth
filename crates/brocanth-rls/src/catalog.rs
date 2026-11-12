//! [`drift`]: the live catalog against a snapshot, for `rls check`.
//!
//! It compares structure only: roles and their attributes and members,
//! `relrowsecurity` and `relforcerowsecurity`, policy names, `permissive`,
//! `cmd`, `roles`, table privileges, and that each function exists. It
//! reports what the database lacks and what it has beyond the snapshot: an
//! extra policy, an extra privilege on a managed table (`PUBLIC` included),
//! an extra member of a managed role. It does not compare `USING` and
//! `WITH CHECK` text: Postgres rewrites it (`docs/rls.md` §3).

use std::collections::BTreeSet;

use brocanth_db::sqlx;
use brocanth_db::{Db, DbError};

use crate::error::RlsError;
use crate::ident::qualified;
use crate::snapshot::{PolicyKind, Snapshot, KEYWORDS};

/// Each difference between the database and `snapshot`, one line each. An
/// empty list means the database matches it.
///
/// # Errors
/// [`RlsError::Db`] when a catalog query failed.
pub async fn drift(db: &Db, snapshot: &Snapshot) -> Result<Vec<String>, RlsError> {
    let pool = db.pool();
    let q = |e: sqlx::Error| RlsError::Db(DbError::from(e));
    let mut out = Vec::new();

    // Postgres 16 grants the creator of a role ADMIN OPTION on it, with no
    // INHERIT and no SET. That grant is not a member in the schema's sense.
    let (version,): (i32,) = sqlx::query_as("SELECT current_setting('server_version_num')::int")
        .fetch_one(pool)
        .await
        .map_err(q)?;
    let members_sql = if version >= 160_000 {
        "SELECT m.rolname::text FROM pg_catalog.pg_auth_members a \
         JOIN pg_catalog.pg_roles r ON r.oid = a.roleid \
         JOIN pg_catalog.pg_roles m ON m.oid = a.member \
         WHERE r.rolname = $1 AND NOT (a.admin_option AND NOT a.inherit_option AND NOT a.set_option)"
    } else {
        "SELECT m.rolname::text FROM pg_catalog.pg_auth_members a \
         JOIN pg_catalog.pg_roles r ON r.oid = a.roleid \
         JOIN pg_catalog.pg_roles m ON m.oid = a.member WHERE r.rolname = $1"
    };

    for (name, spec) in &snapshot.roles {
        let row: Option<(bool, bool, bool, bool, bool, bool)> = sqlx::query_as(
            "SELECT rolinherit, rolcreatedb, rolcreaterole, rolcanlogin, rolsuper, rolbypassrls FROM pg_catalog.pg_roles WHERE rolname = $1",
        )
        .bind(name)
        .fetch_optional(pool)
        .await
        .map_err(q)?;
        let Some((inherit, createdb, createrole, login, superuser, bypassrls)) = row else {
            out.push(format!("role {name}: missing"));
            continue;
        };
        if superuser || bypassrls {
            out.push(format!(
                "role {name}: SUPERUSER={superuser} BYPASSRLS={bypassrls}; it bypasses every policy"
            ));
        }
        if (inherit, createdb, createrole, login)
            != (spec.inherit, spec.createdb, spec.createrole, false)
        {
            out.push(format!(
                "role {name}: inherit={inherit} createdb={createdb} createrole={createrole} login={login}; the schema says inherit={} createdb={} createrole={} login=false",
                spec.inherit, spec.createdb, spec.createrole
            ));
        }
        let members: Vec<(String,)> = sqlx::query_as(members_sql)
            .bind(name)
            .fetch_all(pool)
            .await
            .map_err(q)?;
        let live: BTreeSet<String> = members.into_iter().map(|(m,)| m).collect();
        for missing in spec.members.difference(&live) {
            out.push(format!("role {name}: {missing} is not a member"));
        }
        for extra in live.difference(&spec.members) {
            out.push(format!(
                "role {name}: {extra} is a member; the schema does not grant it"
            ));
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

        // Every privilege on the table but the owner's own, by grantee.
        let granted: Vec<(String, String)> = sqlx::query_as(
            "SELECT CASE WHEN a.grantee = 0 THEN 'public' ELSE pg_catalog.pg_get_userbyid(a.grantee)::text END, a.privilege_type::text \
             FROM pg_catalog.pg_class c, aclexplode(c.relacl) a \
             WHERE c.oid = to_regclass($1) AND a.grantee <> c.relowner",
        )
        .bind(&name)
        .fetch_all(pool)
        .await
        .map_err(q)?;
        let live: BTreeSet<(String, String)> = granted.into_iter().collect();
        let wanted: BTreeSet<(String, String)> = t
            .grants
            .iter()
            .flat_map(|(grantee, privileges)| {
                privileges
                    .iter()
                    .map(move |p| (grantee.clone(), p.as_sql().to_string()))
            })
            .collect();
        for (grantee, p) in wanted.difference(&live) {
            out.push(format!("table {key}: {grantee} lacks {p}"));
        }
        for (grantee, p) in live.difference(&wanted) {
            out.push(format!(
                "table {key}: {grantee} has {p}; the schema does not grant it"
            ));
        }

        let policies: Vec<(String, String, String, Vec<String>)> = sqlx::query_as(
            "SELECT p.polname::text, \
                    CASE WHEN p.polpermissive THEN 'PERMISSIVE' ELSE 'RESTRICTIVE' END, \
                    CASE p.polcmd WHEN 'r' THEN 'SELECT' WHEN 'a' THEN 'INSERT' WHEN 'w' THEN 'UPDATE' WHEN 'd' THEN 'DELETE' ELSE 'ALL' END, \
                    ARRAY(SELECT CASE WHEN r = 0 THEN 'public' ELSE pg_catalog.pg_get_userbyid(r)::text END FROM unnest(p.polroles) r) \
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
            // policy was made, so only a list of names compares. Both
            // sides sorted in Rust's byte order: the database's collation
            // may sort `_` and capitals otherwise.
            let comparable = spec
                .to
                .iter()
                .all(|r| r == "public" || !KEYWORDS.contains(&r.as_str()));
            let mut roles = roles.clone();
            roles.sort();
            let mut to = spec.to.clone();
            to.sort();
            if comparable && roles != to {
                out.push(format!(
                    "policy {policy} on {key}: TO {}, the schema says TO {}",
                    roles.join(", "),
                    to.join(", ")
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
