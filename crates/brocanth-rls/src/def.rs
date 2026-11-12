//! The definitions: [`Role`], [`Table`], [`Policy`], [`Function`], and
//! [`crud`].

use std::collections::BTreeSet;

use crate::expr::SqlExpr;
use crate::ident::{self, SqlType};
use crate::snapshot::{Command, PolicyKind, Privilege, Volatility};

/// A Postgres role.
///
/// `Role::new` is a role this crate creates and drops: `NOLOGIN`, a group
/// for policies and grants. `Role::existing` is a role made elsewhere, such
/// as the login the application connects with; a policy can name it, and no
/// migration touches it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Role {
    pub(crate) name: String,
    pub(crate) existing: bool,
    pub(crate) inherit: bool,
    pub(crate) createdb: bool,
    pub(crate) createrole: bool,
    pub(crate) members: BTreeSet<String>,
}

impl Role {
    /// A role this crate manages: `CREATE ROLE "name" NOLOGIN`.
    pub fn new(name: impl Into<String>) -> Role {
        Role {
            name: name.into(),
            existing: false,
            inherit: true,
            createdb: false,
            createrole: false,
            members: BTreeSet::new(),
        }
    }

    /// A role made elsewhere. Named in policies and grants, never created or
    /// dropped.
    pub fn existing(name: impl Into<String>) -> Role {
        Role {
            existing: true,
            ..Role::new(name)
        }
    }

    /// `NOINHERIT`: a member does not get this role's privileges without
    /// `SET ROLE`.
    pub fn noinherit(mut self) -> Role {
        self.inherit = false;
        self
    }

    /// `CREATEDB`.
    pub fn createdb(mut self) -> Role {
        self.createdb = true;
        self
    }

    /// `CREATEROLE`.
    pub fn createrole(mut self) -> Role {
        self.createrole = true;
        self
    }

    /// `GRANT "this" TO "login"`: `login` may `SET ROLE` to this role. The
    /// application's login needs it for `DbIdentity::role`.
    pub fn member(mut self, login: impl Into<String>) -> Role {
        self.members.insert(login.into());
        self
    }

    /// The name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// A table with row-level security on.
///
/// `Table::new` writes `ENABLE ROW LEVEL SECURITY`: with no policy, the
/// table shows no row to anyone but its owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub(crate) schema: Option<String>,
    pub(crate) name: String,
    pub(crate) force: bool,
    pub(crate) grants: Vec<(String, Vec<Privilege>)>,
}

impl Table {
    /// A table in the first schema of `search_path`.
    pub fn new(name: impl Into<String>) -> Table {
        Table {
            schema: None,
            name: name.into(),
            force: false,
            grants: Vec::new(),
        }
    }

    /// A table in `schema`.
    pub fn in_schema(schema: impl Into<String>, name: impl Into<String>) -> Table {
        Table {
            schema: Some(schema.into()),
            ..Table::new(name)
        }
    }

    /// `FORCE ROW LEVEL SECURITY`: the table's owner obeys the policies too.
    /// Use it on every table with tenant data. A superuser still bypasses.
    pub fn force(mut self) -> Table {
        self.force = true;
        self
    }

    /// `GRANT <privileges> ON <table> TO "role"`. A policy filters rows; a
    /// role still needs the privilege to touch the table at all.
    pub fn grant(mut self, role: &Role, privileges: impl IntoIterator<Item = Privilege>) -> Table {
        self.grants
            .push((role.name.clone(), privileges.into_iter().collect()));
        self
    }

    /// `GRANT <privileges> ON <table> TO PUBLIC`.
    pub fn grant_public(mut self, privileges: impl IntoIterator<Item = Privilege>) -> Table {
        self.grants
            .push(("public".to_string(), privileges.into_iter().collect()));
        self
    }

    /// `schema.name` or `name`: the key in the snapshot.
    pub fn key(&self) -> String {
        ident::key(self.schema.as_deref(), &self.name)
    }
}

/// One `TO` entry of a policy.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Grantee {
    /// `PUBLIC`: every role.
    Public,
    /// `CURRENT_USER`.
    CurrentUser,
    /// `CURRENT_ROLE`.
    CurrentRole,
    /// `SESSION_USER`.
    SessionUser,
    /// A role by name.
    Role(String),
}

impl Grantee {
    pub(crate) fn stored(&self) -> String {
        match self {
            Grantee::Public => "public".to_string(),
            Grantee::CurrentUser => "current_user".to_string(),
            Grantee::CurrentRole => "current_role".to_string(),
            Grantee::SessionUser => "session_user".to_string(),
            Grantee::Role(name) => name.clone(),
        }
    }
}

impl From<&Role> for Grantee {
    fn from(role: &Role) -> Grantee {
        Grantee::Role(role.name.clone())
    }
}

/// `CREATE POLICY`. Defaults as Postgres and Drizzle: `AS PERMISSIVE FOR
/// ALL TO PUBLIC`, no `USING`, no `WITH CHECK`.
#[derive(Debug, Clone)]
pub struct Policy {
    pub(crate) name: String,
    pub(crate) table: String,
    pub(crate) kind: PolicyKind,
    pub(crate) command: Command,
    pub(crate) to: Vec<Grantee>,
    pub(crate) using: Option<SqlExpr>,
    pub(crate) with_check: Option<SqlExpr>,
}

impl Policy {
    /// A policy on `table`. The schema must declare the table.
    pub fn new(name: impl Into<String>, table: &Table) -> Policy {
        Policy {
            name: name.into(),
            table: table.key(),
            kind: PolicyKind::Permissive,
            command: Command::All,
            to: Vec::new(),
            using: None,
            with_check: None,
        }
    }

    /// `AS RESTRICTIVE`: AND-ed with the permissive policies, so it can only
    /// take rows away.
    pub fn restrictive(mut self) -> Policy {
        self.kind = PolicyKind::Restrictive;
        self
    }

    /// `FOR <command>`.
    pub fn command(mut self, command: Command) -> Policy {
        self.command = command;
        self
    }

    /// Adds a `TO` entry: a [`Role`], or a [`Grantee`]. With none, `PUBLIC`.
    pub fn to(mut self, grantee: impl Into<Grantee>) -> Policy {
        self.to.push(grantee.into());
        self
    }

    /// `USING (expr)`: the rows a command sees.
    pub fn using(mut self, expr: SqlExpr) -> Policy {
        self.using = Some(expr);
        self
    }

    /// `WITH CHECK (expr)`: the rows an `INSERT` or `UPDATE` may write.
    pub fn with_check(mut self, expr: SqlExpr) -> Policy {
        self.with_check = Some(expr);
        self
    }

    /// The name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Drizzle's `crudPolicy`: up to four permissive policies for `role` on
/// `table`.
///
/// - `read`: `<table>_<role>_select`, `FOR SELECT USING (read)`.
/// - `modify`: `_insert` (`WITH CHECK`), `_update` (`USING` and
///   `WITH CHECK`), `_delete` (`USING`).
///
/// `None` writes no policy for that part, so Postgres refuses it.
/// `Some(SqlExpr::never())` writes a policy that admits no row: the same
/// effect, said in the schema.
///
/// ```
/// use brocanth_rls::{crud, Role, SqlExpr, Table};
///
/// let member = Role::new("member");
/// let posts = Table::new("posts");
/// let policies = crud(&posts, &member, Some(SqlExpr::always()), None);
/// assert_eq!(policies.len(), 1);
/// assert_eq!(policies[0].name(), "posts_member_select");
/// ```
pub fn crud(
    table: &Table,
    role: &Role,
    read: Option<SqlExpr>,
    modify: Option<SqlExpr>,
) -> Vec<Policy> {
    let base = format!("{}_{}", table.name, role.name);
    let policy = |suffix: &str, command| {
        Policy::new(format!("{base}_{suffix}"), table)
            .command(command)
            .to(role)
    };
    let mut out = Vec::new();
    if let Some(read) = read {
        out.push(policy("select", Command::Select).using(read));
    }
    if let Some(modify) = modify {
        out.push(policy("insert", Command::Insert).with_check(modify.clone()));
        out.push(
            policy("update", Command::Update)
                .using(modify.clone())
                .with_check(modify.clone()),
        );
        out.push(policy("delete", Command::Delete).using(modify));
    }
    out
}

/// A `LANGUAGE sql` function, `SELECT <body>`. For a test a policy repeats,
/// such as a membership lookup.
///
/// ```
/// use brocanth_rls::{Function, SqlExpr, SqlType};
///
/// let is_even = Function::new("is_even", SqlType::Boolean)
///     .arg(SqlType::BigInt)
///     .immutable()
///     .body(SqlExpr::raw(brocanth_db::sea_query::Expr::cust("$1 % 2 = 0")));
/// assert_eq!(is_even.call([SqlExpr::col("id")]).to_sql(), r#""is_even"("id")"#);
/// ```
#[derive(Debug, Clone)]
pub struct Function {
    pub(crate) schema: Option<String>,
    pub(crate) name: String,
    pub(crate) args: Vec<SqlType>,
    pub(crate) returns: SqlType,
    pub(crate) volatility: Volatility,
    pub(crate) definer: Option<Vec<String>>,
    pub(crate) body: Option<SqlExpr>,
}

impl Function {
    /// A function returning `returns`. `STABLE`, `SECURITY INVOKER`.
    pub fn new(name: impl Into<String>, returns: SqlType) -> Function {
        Function {
            schema: None,
            name: name.into(),
            args: Vec::new(),
            returns,
            volatility: Volatility::Stable,
            definer: None,
            body: None,
        }
    }

    /// In `schema`.
    pub fn in_schema(mut self, schema: impl Into<String>) -> Function {
        self.schema = Some(schema.into());
        self
    }

    /// Adds an argument. The body reads the `n`th as [`Function::param`].
    pub fn arg(mut self, ty: SqlType) -> Function {
        self.args.push(ty);
        self
    }

    /// `IMMUTABLE`.
    pub fn immutable(mut self) -> Function {
        self.volatility = Volatility::Immutable;
        self
    }

    /// `VOLATILE`.
    pub fn volatile(mut self) -> Function {
        self.volatility = Volatility::Volatile;
        self
    }

    /// `SECURITY DEFINER SET search_path = <schemas>`: the body runs with
    /// the owner's rights, so a lookup is not filtered by the policies of
    /// the table it reads. The fixed `search_path` stops a caller from
    /// putting its own objects first.
    pub fn security_definer(mut self, search_path: &[&str]) -> Function {
        self.definer = Some(search_path.iter().map(|s| (*s).to_string()).collect());
        self
    }

    /// The expression the function returns.
    pub fn body(mut self, body: SqlExpr) -> Function {
        self.body = Some(body);
        self
    }

    /// `$n`, the `n`th argument (from 1), for the body.
    pub fn param(n: usize) -> SqlExpr {
        SqlExpr::param(n)
    }

    /// `"name"(args…)`. A policy that calls it depends on it.
    pub fn call(&self, args: impl IntoIterator<Item = SqlExpr>) -> SqlExpr {
        SqlExpr::call(
            ident::qualified(self.schema.as_deref(), &self.name),
            self.key(),
            args.into_iter().collect(),
        )
    }

    /// `schema.name` or `name`: the key in the snapshot.
    pub fn key(&self) -> String {
        ident::key(self.schema.as_deref(), &self.name)
    }
}
