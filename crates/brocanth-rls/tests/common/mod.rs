//! The schema the tests share, and the Postgres setup around it.
#![allow(dead_code)]

use std::path::PathBuf;
use std::str::FromStr;

use brocanth_db::sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use brocanth_db::sqlx::{self, AssertSqlSafe};
use brocanth_db::Db;
use brocanth_rls::{
    crud, Command, Function, Policy, Privilege, Role, Schema, Setting, SqlExpr, SqlType, Table,
};

/// The names of one test: its own schema and roles, so tests run in
/// parallel on one server. Roles are cluster-wide, so each carries the
/// test's prefix.
#[derive(Debug, Clone)]
pub struct Names {
    pub schema: String,
    pub member: String,
    pub staff: String,
    pub login: String,
    pub owner: String,
}

impl Names {
    /// Names with this process's id, so two `cargo test` processes on one
    /// server do not share a role: roles are cluster-wide.
    pub fn unique(prefix: &str) -> Names {
        Names::new(&format!("{prefix}_{}", std::process::id()))
    }

    /// Fixed names, for the golden files.
    pub fn new(prefix: &str) -> Names {
        Names {
            schema: prefix.to_string(),
            member: format!("{prefix}_member"),
            staff: format!("{prefix}_staff"),
            login: format!("{prefix}_login"),
            owner: format!("{prefix}_owner"),
        }
    }
}

pub fn tenant() -> Setting {
    Setting::new("tenant_id", SqlType::BigInt)
}

pub fn user() -> Setting {
    Setting::new("user_id", SqlType::BigInt)
}

pub fn member(n: &Names) -> Role {
    Role::new(&n.member).member(&n.login)
}

pub fn posts(n: &Names) -> Table {
    Table::in_schema(&n.schema, "posts")
        .force()
        .grant(&member(n), Privilege::CRUD)
}

pub fn notes(n: &Names) -> Table {
    Table::in_schema(&n.schema, "notes").grant(&member(n), Privilege::CRUD)
}

pub fn current_tenant(n: &Names) -> Function {
    Function::new("current_tenant", SqlType::BigInt)
        .in_schema(&n.schema)
        .body(tenant().get())
}

/// Posts: one tenant's rows, through a function, forced for the owner.
/// Notes: one user's rows, four policies from `crud`, not forced.
pub fn example(n: &Names) -> Schema {
    let member = member(n);
    let posts = posts(n);
    let notes = notes(n);
    let current_tenant = current_tenant(n);
    let mine = SqlExpr::col("tenant_id").eq(current_tenant.call([]));
    let own = SqlExpr::col("owner_id").eq(user().get());
    Schema::new()
        .role(member.clone())
        .role(Role::existing(&n.login))
        .role(Role::existing(&n.owner))
        .function(current_tenant)
        .table(posts.clone())
        .table(notes.clone())
        .policy(
            Policy::new("posts_tenant", &posts)
                .to(&member)
                .to(&Role::existing(&n.owner))
                .using(mine.clone())
                .with_check(mine),
        )
        .policies(crud(&notes, &member, Some(own.clone()), Some(own)))
}

/// `BROCANTH_TEST_DATABASE_URL`, or `None`: the test is skipped.
pub fn url() -> Option<String> {
    let url = std::env::var("BROCANTH_TEST_DATABASE_URL").ok()?;
    (!url.is_empty()).then_some(url)
}

/// A pool of one connection as `user`, or as the URL's user.
pub async fn connect(url: &str, user: Option<&str>) -> Db {
    let mut options = PgConnectOptions::from_str(url).expect("a Postgres URL");
    if let Some(user) = user {
        options = options.username(user).password("pw");
    }
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("connect");
    Db::from_pool(pool)
}

/// Runs `sql` (many statements) in one transaction, as a migration does.
pub async fn apply(db: &Db, sql: &str) {
    let mut tx = db.pool().begin().await.expect("begin");
    sqlx::raw_sql(AssertSqlSafe(sql.to_string()))
        .execute(&mut *tx)
        .await
        .unwrap_or_else(|e| panic!("{e}\n--- sql ---\n{sql}"));
    tx.commit().await.expect("commit");
}

/// [`apply`], with the error as text instead of a panic.
pub async fn try_apply(db: &Db, sql: &str) -> Result<(), String> {
    let mut tx = db.pool().begin().await.map_err(|e| e.to_string())?;
    sqlx::raw_sql(AssertSqlSafe(sql.to_string()))
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())
}

/// Drops what an earlier run left, then makes the schema, the two tables
/// and the two login roles. The tables belong to the owner.
pub async fn setup(admin: &Db, n: &Names) {
    teardown(admin, n).await;
    let s = &n.schema;
    apply(
        admin,
        &format!(
            "CREATE ROLE {owner} LOGIN PASSWORD 'pw';
             CREATE ROLE {login} LOGIN PASSWORD 'pw' NOINHERIT;
             CREATE SCHEMA {s} AUTHORIZATION {owner};
             SET LOCAL ROLE {owner};
             CREATE TABLE {s}.posts (id bigint PRIMARY KEY, tenant_id bigint NOT NULL, title text NOT NULL);
             CREATE TABLE {s}.notes (id bigint PRIMARY KEY, owner_id bigint NOT NULL, body text NOT NULL);
             INSERT INTO {s}.posts VALUES (1, 1, 'one'), (2, 1, 'two'), (3, 2, 'other tenant');
             INSERT INTO {s}.notes VALUES (1, 10, 'mine'), (2, 20, 'theirs');",
            owner = n.owner,
            login = n.login,
        ),
    )
    .await;
}

/// The grant `brocanth-rls` does not manage: `USAGE` on the schema.
pub async fn grant_usage(admin: &Db, n: &Names) {
    apply(
        admin,
        &format!("GRANT USAGE ON SCHEMA {} TO {}", n.schema, n.member),
    )
    .await;
}

pub async fn teardown(admin: &Db, n: &Names) {
    let mut sql = format!("DROP SCHEMA IF EXISTS {} CASCADE;", n.schema);
    let migrator = format!("{}_migrator", n.schema);
    let group = format!("{}_group", n.schema);
    for role in [&n.member, &n.staff, &n.login, &n.owner, &migrator, &group] {
        sql.push_str(&format!(
            "DO $$ BEGIN IF EXISTS (SELECT FROM pg_roles WHERE rolname = '{role}') THEN
                 EXECUTE 'DROP OWNED BY {role} CASCADE'; EXECUTE 'DROP ROLE {role}';
             END IF; END $$;"
        ));
    }
    apply(admin, &sql).await;
}

/// A directory of its own under the system's temporary directory.
pub fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("brocanth-rls-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// Compares `text` with the golden file, or writes it under
/// `BROCANTH_BLESS=1`.
pub fn golden(name: &str, text: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os("BROCANTH_BLESS").is_some() {
        std::fs::write(&path, text).expect("write the golden file");
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e}\nhelp: run `BROCANTH_BLESS=1 cargo test -p brocanth-rls` and read the diff",
            path.display()
        )
    });
    assert!(
        want == text,
        "{} differs\n--- want ---\n{want}\n--- got ---\n{text}\nhelp: run `BROCANTH_BLESS=1 cargo test -p brocanth-rls` and read the diff",
        path.display()
    );
}

/// The changes the Postgres tests apply, as SQL, both ways.
pub fn changed(n: &Names) -> Schema {
    let member = member(n);
    let staff = Role::new(&n.staff).noinherit();
    let posts = posts(n);
    let notes = notes(n);
    let current_tenant = current_tenant(n);
    let mine = SqlExpr::col("tenant_id").eq(current_tenant.call([]));
    let own = SqlExpr::col("owner_id").eq(user().get());
    Schema::new()
        // added: a role
        .role(member.clone().createdb())
        .role(staff.clone())
        .role(Role::existing(&n.login))
        .role(Role::existing(&n.owner))
        .function(current_tenant)
        .table(posts.clone())
        // changed: notes is forced now, and staff may read it
        .table(notes.clone().force().grant(&staff, [Privilege::Select]))
        // changed: posts_tenant reads only (FOR SELECT): a recreate
        .policy(
            Policy::new("posts_tenant", &posts)
                .command(Command::Select)
                .to(&member)
                .to(&Role::existing(&n.owner))
                .using(mine),
        )
        // changed: the select policy gets another USING: an alter
        .policy(
            Policy::new(format!("notes_{}_select", n.member), &notes)
                .command(Command::Select)
                .to(&member)
                .using(own.clone().or(SqlExpr::col("id").lt(SqlExpr::val(0)))),
        )
        // dropped: notes_app_member_insert, _update and _delete
        // added: staff reads every note
        .policy(
            Policy::new("notes_staff", &notes)
                .command(Command::Select)
                .to(&staff)
                .using(SqlExpr::always()),
        )
}

/// An altered policy that named a dropped role or called a
/// dropped function ran its ALTER after the drop, and Postgres refused the
/// drop. The policy is recreated, so its drop runs first.
pub fn dropped_dependency(n: &Names) -> (Schema, Schema) {
    let member = member(n);
    let staff = Role::new(&n.staff);
    let posts = posts(n);
    let f = current_tenant(n);
    let v1 = Schema::new()
        .role(member.clone())
        .role(staff.clone())
        .role(Role::existing(&n.login))
        .function(f.clone())
        .table(posts.clone())
        .policy(
            Policy::new("posts_tenant", &posts)
                .to(&member)
                .to(&staff)
                .using(SqlExpr::col("tenant_id").eq(f.call([]))),
        );
    let v2 = Schema::new()
        .role(member.clone())
        .role(Role::existing(&n.login))
        .table(posts.clone())
        .policy(
            Policy::new("posts_tenant", &posts)
                .to(&member)
                .using(SqlExpr::col("tenant_id").eq(tenant().get())),
        );
    (v1, v2)
}
