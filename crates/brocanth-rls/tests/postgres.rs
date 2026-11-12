//! On a real Postgres: migrations apply and reverse, the catalog matches,
//! and the policies hold for a role that is not the owner, for the owner
//! with FORCE, and across requests on one pooled connection.
//!
//! Skipped without `BROCANTH_TEST_DATABASE_URL` (a superuser URL). Each test
//! has its own schema and roles named after it.

mod common;

use brocanth_db::sea_query::{Expr, ExprTrait, Query};
use brocanth_db::{sqlx, Db, DbError, DbIdentity, Exec};
use brocanth_rls::migration::{self, Format};
use brocanth_rls::{catalog, diff, Snapshot};
use common::{apply, changed, connect, example, grant_usage, setup, teardown, url, Names};

macro_rules! require_db {
    () => {
        match url() {
            Some(url) => url,
            None => {
                eprintln!("skipped: BROCANTH_TEST_DATABASE_URL is not set");
                return;
            }
        }
    };
}

fn titles(n: &Names) -> brocanth_db::sea_query::SelectStatement {
    Query::select()
        .column("title")
        .from((n.schema.clone(), "posts".to_string()))
        .order_by("id", brocanth_db::sea_query::Order::Asc)
        .to_owned()
}

fn member(n: &Names, tenant: i64, user: i64) -> DbIdentity {
    DbIdentity::new()
        .role(&n.member)
        .set("tenant_id", tenant)
        .set("user_id", user)
}

async fn assert_no_drift(admin: &Db, snapshot: &Snapshot) {
    let drift = catalog::drift(admin, snapshot).await.unwrap();
    assert!(drift.is_empty(), "drift: {drift:#?}");
}

#[tokio::test]
async fn a_migration_applies_reverses_and_the_catalog_matches_each_side() {
    let url = require_db!();
    let n = Names::new("rls_diff");
    let admin = connect(&url, None).await;
    setup(&admin, &n).await;

    let empty = Snapshot::default();
    let v1 = example(&n).snapshot().unwrap();
    let v2 = changed(&n).snapshot().unwrap();

    // create, change, change back, drop: each step leaves the catalog as
    // its snapshot says.
    for (from, to) in [(&empty, &v1), (&v1, &v2), (&v2, &v1), (&v1, &empty)] {
        let plan = diff(from, to).unwrap();
        assert!(!plan.is_empty());
        apply(&admin, &plan.sql()).await;
        assert_no_drift(&admin, to).await;
        assert!(diff(to, to).unwrap().is_empty());
    }
    // Every managed role is gone with the empty snapshot.
    let (left,): (i64,) = sqlx::query_as("SELECT count(*) FROM pg_roles WHERE rolname = ANY($1)")
        .bind(vec![n.member.clone(), n.staff.clone()])
        .fetch_one(admin.pool())
        .await
        .unwrap();
    assert_eq!(left, 0);
    teardown(&admin, &n).await;
}

#[tokio::test]
async fn drift_names_what_the_database_lacks() {
    let url = require_db!();
    let n = Names::new("rls_drift");
    let admin = connect(&url, None).await;
    setup(&admin, &n).await;
    let v1 = example(&n).snapshot().unwrap();
    apply(&admin, &diff(&Snapshot::default(), &v1).unwrap().sql()).await;
    apply(
        &admin,
        &format!(
            "DROP POLICY posts_tenant ON {s}.posts; ALTER TABLE {s}.posts NO FORCE ROW LEVEL SECURITY; \
             CREATE POLICY stray ON {s}.notes USING (true)",
            s = n.schema
        ),
    )
    .await;
    let drift = catalog::drift(&admin, &v1).await.unwrap();
    let text = drift.join("\n");
    assert!(
        text.contains("policy posts_tenant on rls_drift.posts: missing"),
        "{text}"
    );
    assert!(
        text.contains("table rls_drift.posts: FORCE is off"),
        "{text}"
    );
    assert!(
        text.contains("policy stray on rls_drift.notes: not in the schema"),
        "{text}"
    );
    teardown(&admin, &n).await;
}

#[tokio::test]
async fn brocanth_db_migrate_applies_and_rolls_back_the_file_rls_make_wrote() {
    let url = require_db!();
    let n = Names::new("rls_make");
    let admin = connect(&url, None).await;
    setup(&admin, &n).await;
    // `brocanth_migrations` in this test's schema, not in `public`.
    let mut config = brocanth_db::DbConfig::new(&url);
    config.search_path = vec![n.schema.clone()];
    config.max_connections = 1;
    let db = Db::connect(&config).await.unwrap();

    let dir = common::temp_dir("rls_make");
    migration::make(&example(&n), &dir, "add_rls", Format::Sql)
        .unwrap()
        .unwrap();
    let files = brocanth_db::migrate::read_dir(&dir).unwrap();
    let migrator = brocanth_db::migrate::Migrator::new(&db, files);
    assert_eq!(migrator.up().await.unwrap().len(), 1);
    assert_no_drift(&admin, &example(&n).snapshot().unwrap()).await;

    // The next change is a second file, from the snapshot the first wrote.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    migration::make(&changed(&n), &dir, "change_rls", Format::Sql)
        .unwrap()
        .unwrap();
    let files = brocanth_db::migrate::read_dir(&dir).unwrap();
    let migrator = brocanth_db::migrate::Migrator::new(&db, files);
    assert_eq!(migrator.up().await.unwrap().len(), 1);
    assert_no_drift(&admin, &changed(&n).snapshot().unwrap()).await;

    assert_eq!(migrator.down(2).await.unwrap().len(), 2);
    let drift = catalog::drift(&admin, &example(&n).snapshot().unwrap())
        .await
        .unwrap();
    assert!(drift.iter().any(|d| d.contains("missing")), "{drift:#?}");
    db.close().await;
    let _ = std::fs::remove_dir_all(&dir);
    teardown(&admin, &n).await;
}

/// Sets up the example schema and returns (admin, a pool as the login).
async fn enforced(url: &str, n: &Names) -> (Db, Db) {
    let admin = connect(url, None).await;
    setup(&admin, n).await;
    let sql = example(n).create_sql().unwrap();
    apply(&admin, &sql).await;
    grant_usage(&admin, n).await;
    let login = connect(url, Some(&n.login)).await;
    (admin, login)
}

#[tokio::test]
async fn a_member_sees_and_writes_only_its_own_rows() {
    let url = require_db!();
    let n = Names::new("rls_enforce");
    let (admin, login) = enforced(&url, &n).await;
    let posts = format!("{}.posts", n.schema);

    // SELECT: tenant 1 sees its two posts, never tenant 2's.
    let rows: Vec<(String,)> = login
        .transact_as(&member(&n, 1, 10), async |tx| tx.all(&titles(&n)).await)
        .await
        .unwrap();
    assert_eq!(rows, [("one".to_string(),), ("two".to_string(),)]);
    let rows: Vec<(String,)> = login
        .transact_as(&member(&n, 2, 20), async |tx| tx.all(&titles(&n)).await)
        .await
        .unwrap();
    assert_eq!(rows, [("other tenant".to_string(),)]);

    // INSERT into another tenant: refused by WITH CHECK.
    let insert = Query::insert()
        .into_table((n.schema.clone(), "posts".to_string()))
        .columns(["id", "tenant_id", "title"])
        .values_panic([4.into(), 2.into(), "smuggled".into()])
        .to_owned();
    let e = login
        .transact_as(&member(&n, 1, 10), async |tx| tx.execute(&insert).await)
        .await
        .unwrap_err();
    assert_policy_refused(&e);

    // UPDATE an own row into another tenant: refused by WITH CHECK.
    let move_out = Query::update()
        .table((n.schema.clone(), "posts".to_string()))
        .value("tenant_id", 2)
        .and_where(Expr::col("id").eq(1))
        .to_owned();
    let e = login
        .transact_as(&member(&n, 1, 10), async |tx| tx.execute(&move_out).await)
        .await
        .unwrap_err();
    assert_policy_refused(&e);

    // UPDATE and DELETE of another tenant's row: invisible, so no row.
    let rename = Query::update()
        .table((n.schema.clone(), "posts".to_string()))
        .value("title", "taken")
        .and_where(Expr::col("id").eq(3))
        .to_owned();
    let delete = Query::delete()
        .from_table((n.schema.clone(), "posts".to_string()))
        .and_where(Expr::col("id").eq(3))
        .to_owned();
    let (renamed, deleted) = login
        .transact_as(&member(&n, 1, 10), async |tx| {
            Ok((tx.execute(&rename).await?, tx.execute(&delete).await?))
        })
        .await
        .unwrap();
    assert_eq!((renamed, deleted), (0, 0));

    // Notes, from `crud`: user 10 sees its own note only, and cannot write
    // one for user 20.
    let (count,): (i64,) = login
        .transact_as(&member(&n, 1, 10), async |tx| {
            let q = Query::select()
                .expr(Expr::cust("count(*)"))
                .from((n.schema.clone(), "notes".to_string()))
                .to_owned();
            tx.one(&q).await
        })
        .await
        .unwrap();
    assert_eq!(count, 1);
    let theirs = Query::insert()
        .into_table((n.schema.clone(), "notes".to_string()))
        .columns(["id", "owner_id", "body"])
        .values_panic([3.into(), 20.into(), "forged".into()])
        .to_owned();
    let e = login
        .transact_as(&member(&n, 1, 10), async |tx| tx.execute(&theirs).await)
        .await
        .unwrap_err();
    assert_policy_refused(&e);

    // Nothing changed for the owner's view either.
    let (left,): (i64,) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) FROM {posts} WHERE tenant_id = 2 AND title = 'other tenant'"
    )))
    .fetch_one(admin.pool())
    .await
    .unwrap();
    assert_eq!(left, 1);
    teardown(&admin, &n).await;
}

fn assert_policy_refused(e: &DbError) {
    // 42501: new row violates row-level security policy.
    assert_eq!(e.code(), Some("42501"), "{e}");
    assert!(e.to_string().contains("row-level security"), "{e}");
}

#[tokio::test]
async fn force_makes_the_owner_obey_and_without_force_the_owner_sees_all() {
    let url = require_db!();
    let n = Names::new("rls_force");
    let (admin, _login) = enforced(&url, &n).await;
    let owner = connect(&url, Some(&n.owner)).await;
    let count = |table: &'static str| {
        let s = n.schema.clone();
        Query::select()
            .expr(Expr::cust("count(*)"))
            .from((s, table.to_string()))
            .to_owned()
    };

    // posts is forced: the owner sees its tenant's rows, and none without
    // a tenant.
    let as_tenant_1 = DbIdentity::new().set("tenant_id", 1);
    let (n1,): (i64,) = owner
        .transact_as(&as_tenant_1, async |tx| tx.one(&count("posts")).await)
        .await
        .unwrap();
    assert_eq!(n1, 2);
    let (none,): (i64,) = owner
        .transact(async |tx| tx.one(&count("posts")).await)
        .await
        .unwrap();
    assert_eq!(none, 0);

    // notes is not forced: the owner bypasses its policies.
    let (all,): (i64,) = owner
        .transact(async |tx| tx.one(&count("notes")).await)
        .await
        .unwrap();
    assert_eq!(all, 2);
    owner.close().await;
    teardown(&admin, &n).await;
}

#[tokio::test]
async fn the_principal_never_reaches_the_next_transaction_on_the_connection() {
    let url = require_db!();
    let n = Names::new("rls_leak");
    let (admin, login) = enforced(&url, &n).await;
    // The pool has one connection, so each transaction below reuses it.
    assert_eq!(login.pool().options().get_max_connections(), 1);
    let who = "SELECT current_user::text, coalesce(current_setting('brocanth.tenant_id', true), '<null>')";

    let (role, tenant): (String, String) = login
        .transact_as(&member(&n, 1, 10), async |tx| {
            Ok(sqlx::query_as(who).fetch_one(tx.executor()).await?)
        })
        .await
        .unwrap();
    assert_eq!((role.as_str(), tenant.as_str()), (n.member.as_str(), "1"));

    // The next request, committed or not, starts as the login with no value.
    let next = async || -> (String, String) {
        login
            .transact(async |tx| Ok(sqlx::query_as(who).fetch_one(tx.executor()).await?))
            .await
            .unwrap()
    };
    let (role, tenant) = next().await;
    assert_eq!(role, n.login);
    assert!(tenant.is_empty() || tenant == "<null>", "{tenant}");

    // A transaction that fails rolls the principal back too.
    let failed: Result<(), DbError> = login
        .transact_as(&member(&n, 2, 20), async |tx| {
            tx.one::<(i32,)>(&Query::select().expr(Expr::cust("1/0")).to_owned())
                .await
                .map(drop)
        })
        .await;
    assert!(failed.is_err());
    let (role, tenant) = next().await;
    assert_eq!(role, n.login);
    assert!(tenant.is_empty() || tenant == "<null>", "{tenant}");

    // And with no principal, the login reads nothing: it has no grant, and
    // as the member with no tenant the policy admits no row.
    let e = login
        .transact(async |tx| tx.all::<(String,)>(&titles(&n)).await)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("42501"), "{e}");
    let rows: Vec<(String,)> = login
        .transact_as(&DbIdentity::new().role(&n.member), async |tx| {
            tx.all(&titles(&n)).await
        })
        .await
        .unwrap();
    assert!(rows.is_empty());
    teardown(&admin, &n).await;
}

#[tokio::test]
async fn a_role_the_login_is_not_granted_is_refused_with_help() {
    let url = require_db!();
    let n = Names::new("rls_role");
    let (admin, login) = enforced(&url, &n).await;
    let e = login
        .transact_as(&DbIdentity::new().role(&n.owner), async |_| Ok(()))
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("help:"), "{e}");
    teardown(&admin, &n).await;
}

#[tokio::test]
async fn a_second_identity_replaces_the_first_and_does_not_merge() {
    let url = require_db!();
    let n = Names::new("rls_assume");
    let (admin, login) = enforced(&url, &n).await;
    let who = "SELECT current_user::text, \
               coalesce(nullif(current_setting('brocanth.user_id', true), ''), '<null>'), \
               coalesce(nullif(current_setting('brocanth.tenant_id', true), ''), '<null>')";
    let seen: (String, String, String) = login
        .transact(async |tx| {
            tx.assume(&member(&n, 1, 10)).await?;
            // A savepoint shares the record: its assume replaces too.
            let mut inner = tx.begin().await?;
            inner.assume(&DbIdentity::new().set("tenant_id", 2)).await?;
            let seen = sqlx::query_as(who).fetch_one(inner.executor()).await?;
            inner.commit().await?;
            Ok(seen)
        })
        .await
        .unwrap();
    // No role: back to the login. No user_id: cleared, not kept from the
    // first identity.
    assert_eq!(
        seen,
        (n.login.clone(), "<null>".to_string(), "2".to_string())
    );
    teardown(&admin, &n).await;
}
