//! `Policy::from_rules` on a real Postgres: the SQL it writes applies, and
//! a few rows per subject come out as the rules say. The full agreement
//! harness (docs/policy-rls-bridge.md decision 10) is phase 5.
//!
//! Also the owned-`no_force` report of `rls check` (§4, layer 1), and the
//! sequence `USAGE` in the catalog check.
//!
//! Skipped without `BROCANTH_TEST_DATABASE_URL` (a superuser URL). Each test
//! has its own schema and roles.
#![allow(missing_docs)]

mod common;

use std::str::FromStr;

use brocanth_db::sea_query::{Expr, ExprTrait, Order, Query};
use brocanth_db::sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use brocanth_db::{Db, DbError, DbIdentity, Exec};
use brocanth_policy::{Subject, Value};
use brocanth_rls::{catalog, Policy, Role, Schema};
use common::policy::{PostDb, Visitor};
use common::{apply, connect, example, setup, teardown, url, Names};

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

/// A pool of one connection as the URL's user, with the test's schema
/// first in `search_path`: the bridge names the table `posts`, unqualified.
async fn admin_in(url: &str, schema: &str) -> Db {
    let options = PgConnectOptions::from_str(url)
        .expect("a Postgres URL")
        .options([("search_path", schema)]);
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("connect");
    Db::from_pool(pool)
}

fn rls_schema(n: &Names) -> Schema {
    let member = Role::new(&n.member).member(&n.login);
    Schema::new()
        .role(member.clone())
        .role(Role::existing(&n.login))
        .policies(Policy::from_rules::<PostDb>(&member).sequence("posts_id_seq"))
}

/// Every attribute set, `''` for an absent one, as
/// `brocanth_rls::identity` will (bridge decision 6).
fn identity(n: &Names, who: &Visitor) -> DbIdentity {
    who.attributes()
        .iter()
        .fold(DbIdentity::new().role(&n.member), |id, (key, value)| {
            let text = match value {
                None => String::new(),
                Some(Value::Int(i)) => i.to_string(),
                Some(Value::Bool(b)) => b.to_string(),
                Some(Value::Text(t)) => t.clone(),
                Some(other) => panic!("no text for {other:?}"),
            };
            id.set(key, text)
        })
}

fn visitor(user_id: Option<i64>, admin: bool, banned: bool) -> Visitor {
    Visitor {
        user_id,
        tenant_id: 1,
        admin,
        banned,
        role: String::new(),
    }
}

async fn ids(login: &Db, n: &Names, me: &DbIdentity) -> Vec<i64> {
    let q = Query::select()
        .column("id")
        .from((n.schema.clone(), "posts".to_string()))
        .order_by("id", Order::Asc)
        .to_owned();
    let rows: Vec<(i64,)> = login
        .transact_as(me, async |tx| tx.all(&q).await)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    rows.into_iter().map(|(id,)| id).collect()
}

fn assert_refused(e: &DbError, code: &str, text: &str) {
    assert_eq!(e.code(), Some(code), "{e}");
    assert!(e.to_string().contains(text), "{e}");
}

#[tokio::test]
async fn the_rules_hold_in_the_database_for_each_subject() {
    let url = require_db!();
    let n = Names::unique("rls_bridge");
    let admin = connect(&url, None).await;
    teardown(&admin, &n).await;
    let s = &n.schema;
    apply(
        &admin,
        &format!(
            "CREATE ROLE {login} LOGIN PASSWORD 'pw' NOINHERIT;
             CREATE SCHEMA {s};
             CREATE TABLE {s}.posts (id bigserial PRIMARY KEY, author_id bigint, tenant_id bigint NOT NULL, status text NOT NULL, title text NOT NULL);
             INSERT INTO {s}.posts (author_id, tenant_id, status, title) VALUES
               (1, 1, 'published', 'p1'),
               (1, 1, 'draft', 'd1'),
               (2, 1, 'draft', 'd2'),
               (NULL, 1, 'published', 'anonymous'),
               (3, 2, 'published', 'other tenant');",
            login = n.login,
        ),
    )
    .await;
    let sql = rls_schema(&n).create_sql().unwrap();
    apply(&admin, &format!("SET LOCAL search_path = {s};\n{sql}")).await;
    apply(
        &admin,
        &format!("GRANT USAGE ON SCHEMA {s} TO {}", n.member),
    )
    .await;
    let snapshot = rls_schema(&n).snapshot().unwrap();
    let scoped = admin_in(&url, s).await;
    let drift = catalog::drift(&scoped, &snapshot).await.unwrap();
    assert!(drift.is_empty(), "{drift:#?}");

    let login = connect(&url, Some(&n.login)).await;
    // SELECT is `list OR view`. Tenant 1's published posts, and the
    // author's own; the admin sees every row; banned, only the `view` rows.
    for (who, want) in [
        (visitor(None, false, false), vec![1, 4]),
        (visitor(Some(1), false, false), vec![1, 2, 4]),
        (visitor(Some(2), false, false), vec![1, 3, 4]),
        (visitor(None, true, false), vec![1, 2, 3, 4, 5]),
        (visitor(Some(1), false, true), vec![1, 2]),
    ] {
        assert_eq!(ids(&login, &n, &identity(&n, &who)).await, want, "{who:?}");
    }
    // A missing guard setting: the rule is FALSE. Only `view` admits rows,
    // and it reads `user_id`, which is missing too.
    let bare = DbIdentity::new().role(&n.member).set("tenant_id", 1);
    assert!(ids(&login, &n, &bare).await.is_empty());

    let author = identity(&n, &visitor(Some(1), false, false));
    let posts = (n.schema.clone(), "posts".to_string());
    // INSERT: the sequence `USAGE` lets `nextval` run; WITH CHECK refuses
    // a row for another author.
    let insert = |author_id: i64| {
        Query::insert()
            .into_table(posts.clone())
            .columns(["author_id", "tenant_id", "status", "title"])
            .values_panic([author_id.into(), 1.into(), "draft".into(), "new".into()])
            .to_owned()
    };
    let (mine, theirs) = (insert(1), insert(2));
    let added = login
        .transact_as(&author, async |tx| tx.execute(&mine).await)
        .await
        .unwrap();
    assert_eq!(added, 1);
    let e = login
        .transact_as(&author, async |tx| tx.execute(&theirs).await)
        .await
        .unwrap_err();
    assert_refused(&e, "42501", "row-level security");

    // UPDATE: `update_check` lets the author publish a draft, and refuses
    // to give it away; another author's draft is not there to update.
    let set = |column: &str, value: Expr, id: i64| {
        Query::update()
            .table(posts.clone())
            .value(column.to_string(), value)
            .and_where(Expr::col("id").eq(id))
            .to_owned()
    };
    let give = set("author_id", Expr::val(2), 2);
    let e = login
        .transact_as(&author, async |tx| tx.execute(&give).await)
        .await
        .unwrap_err();
    assert_refused(&e, "42501", "row-level security");
    let (publish, steal) = (
        set("status", Expr::val("published"), 2),
        set("title", Expr::val("taken"), 3),
    );
    let changed = login
        .transact_as(&author, async |tx| {
            Ok((tx.execute(&publish).await?, tx.execute(&steal).await?))
        })
        .await
        .unwrap();
    assert_eq!(changed, (1, 0));

    // DELETE: the rule is `never`, so no policy and no grant.
    let delete = Query::delete()
        .from_table(posts.clone())
        .and_where(Expr::col("id").eq(1))
        .to_owned();
    let admin_visitor = identity(&n, &visitor(None, true, false));
    let e = login
        .transact_as(&admin_visitor, async |tx| tx.execute(&delete).await)
        .await
        .unwrap_err();
    assert_refused(&e, "42501", "permission denied");

    // The catalog check sees a revoked sequence grant.
    apply(
        &admin,
        &format!(
            "REVOKE USAGE ON SEQUENCE {s}.posts_id_seq FROM {}",
            n.member
        ),
    )
    .await;
    let drift = catalog::drift(&scoped, &snapshot).await.unwrap();
    assert_eq!(
        drift,
        [format!(
            "sequence posts_id_seq of posts: {} lacks USAGE",
            n.member
        )]
    );

    scoped.close().await;
    login.close().await;
    teardown(&admin, &n).await;
}

#[tokio::test]
async fn rls_check_reports_an_unforced_table_the_login_owns() {
    let url = require_db!();
    let n = Names::unique("rls_owned");
    let admin = connect(&url, None).await;
    setup(&admin, &n).await;
    apply(&admin, &example(&n).create_sql().unwrap()).await;
    let snapshot = example(&n).snapshot().unwrap();

    // The owner of both tables: `notes` is `.no_force()`, `posts` forced.
    let owner = connect(&url, Some(&n.owner)).await;
    let report = catalog::owned_unforced(&owner, &snapshot).await.unwrap();
    assert_eq!(report.len(), 1, "{report:#?}");
    assert!(
        report[0].starts_with(&format!(
            "table {}.notes: not forced, and the login {} owns it",
            n.schema, n.owner
        )),
        "{}",
        report[0]
    );
    assert!(report[0].contains("\nhelp: "), "{}", report[0]);
    // Another login owns nothing.
    assert!(catalog::owned_unforced(&admin, &snapshot)
        .await
        .unwrap()
        .is_empty());
    owner.close().await;
    teardown(&admin, &n).await;
}
