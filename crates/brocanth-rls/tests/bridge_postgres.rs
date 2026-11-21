//! `Policy::from_rules` on a real Postgres: the SQL it writes applies, and
//! a few rows per subject come out as the rules say. The full agreement
//! harness (docs/policy-rls-bridge.md decision 10) runs in
//! `tests/agreement.rs`.
//!
//! Also the owned-`no_force` report of `rls check` (§4, layer 1), the
//! sequence `USAGE` in the catalog check, and the `EXPLAIN` check: a
//! comparison with a setting keeps the column's index (§1, decision 5),
//! and so does a guard chain with an arm that reads no column (decision
//! 14).
//!
//! Skipped without `BROCANTH_TEST_DATABASE_URL` (a superuser URL). Each test
//! has its own schema and roles.
#![allow(missing_docs)]

mod common;

use std::str::FromStr;

use brocanth_db::sea_query::{Expr, ExprTrait, Order, Query};
use brocanth_db::sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use brocanth_db::{Db, DbError, DbIdentity, Exec};
use brocanth_policy::{FieldMask, MaskRule, Subject, Value};
use brocanth_rls::{catalog, Policy, Role, Schema};
use common::policy::{db_policy, post_rules, PostColumn, PostDb, Visitor};
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

/// `writable: only [title, status]`, `writable_on_create: only [title,
/// author_id, tenant_id]`: column grants (bridge decision 9).
pub struct Masked;
db_policy!(Masked, {
    let mut rules = post_rules();
    rules.writable = Some(MaskRule::Fixed(FieldMask::only([
        PostColumn::Title,
        PostColumn::Status,
    ])));
    rules.writable_on_create = Some(MaskRule::Fixed(FieldMask::only([
        PostColumn::Title,
        PostColumn::AuthorId,
        PostColumn::TenantId,
    ])));
    rules
});

/// The same rules, with a `writable` mask that has a guard.
pub struct GuardedMask;
db_policy!(GuardedMask, {
    let mut rules = post_rules();
    rules.writable = Some(MaskRule::PerSubject);
    rules
});

#[tokio::test]
async fn an_unguarded_mask_is_a_column_grant_and_a_guarded_one_is_the_table() {
    let url = require_db!();
    let n = Names::unique("rls_masks");
    let admin = connect(&url, None).await;
    teardown(&admin, &n).await;
    let s = &n.schema;
    apply(
        &admin,
        &format!(
            "CREATE ROLE {login} LOGIN PASSWORD 'pw' NOINHERIT;
             CREATE SCHEMA {s};
             CREATE TABLE {s}.posts (id bigserial PRIMARY KEY, author_id bigint, tenant_id bigint NOT NULL, status text NOT NULL DEFAULT 'draft', title text NOT NULL);
             INSERT INTO {s}.posts (author_id, tenant_id, status, title) VALUES
               (1, 1, 'draft', 'd1'),
               (2, 1, 'draft', 'd2');",
            login = n.login,
        ),
    )
    .await;
    let member = Role::new(&n.member).member(&n.login);
    let masked = || {
        Schema::new()
            .role(member.clone())
            .role(Role::existing(&n.login))
            .policies(Policy::from_rules::<Masked>(&member).sequence("posts_id_seq"))
    };
    assert!(masked().warnings().is_empty());
    let sql = masked().create_sql().unwrap();
    apply(&admin, &format!("SET LOCAL search_path = {s};\n{sql}")).await;
    apply(
        &admin,
        &format!("GRANT USAGE ON SCHEMA {s} TO {}", n.member),
    )
    .await;
    let snapshot = masked().snapshot().unwrap();
    let scoped = admin_in(&url, s).await;
    let drift = catalog::drift(&scoped, &snapshot).await.unwrap();
    assert!(drift.is_empty(), "{drift:#?}");

    let login = connect(&url, Some(&n.login)).await;
    let author = identity(&n, &visitor(Some(1), false, false));
    let posts = (n.schema.clone(), "posts".to_string());
    let set = |column: &str, value: Expr| {
        Query::update()
            .table(posts.clone())
            .value(column.to_string(), value)
            // The WHERE reads `id` and `status`: the table SELECT grant
            // covers them, and the policies still filter the rows.
            .and_where(Expr::col("id").eq(1))
            .and_where(Expr::col("status").eq("draft"))
            .to_owned()
    };

    // A granted column passes the grant, then the policy.
    let title = set("title", Expr::val("edited"));
    let changed = login
        .transact_as(&author, async |tx| tx.execute(&title).await)
        .await
        .unwrap();
    assert_eq!(changed, 1);
    // Another author's draft: granted column, but the policy hides the row.
    let other = Query::update()
        .table(posts.clone())
        .value("title", "taken")
        .and_where(Expr::col("id").eq(2))
        .to_owned();
    let changed = login
        .transact_as(&author, async |tx| tx.execute(&other).await)
        .await
        .unwrap();
    assert_eq!(changed, 0);
    // A column the mask leaves out: permission denied before any row.
    let owner = set("author_id", Expr::val(1));
    let e = login
        .transact_as(&author, async |tx| tx.execute(&owner).await)
        .await
        .unwrap_err();
    assert_refused(&e, "42501", "permission denied for table posts");

    // INSERT: the `writable_on_create` columns, and the sequence default.
    let insert = |columns: &[&str]| {
        let values: Vec<Expr> = columns
            .iter()
            .map(|c| match *c {
                "author_id" => Expr::val(1),
                "tenant_id" => Expr::val(1),
                "status" => Expr::val("draft"),
                _ => Expr::val("new"),
            })
            .collect();
        Query::insert()
            .into_table(posts.clone())
            .columns(columns.iter().map(|c| c.to_string()))
            .values_panic(values)
            .to_owned()
    };
    let granted = insert(&["author_id", "tenant_id", "title"]);
    let added = login
        .transact_as(&author, async |tx| tx.execute(&granted).await)
        .await
        .unwrap();
    assert_eq!(added, 1);
    let with_status = insert(&["author_id", "tenant_id", "status", "title"]);
    let e = login
        .transact_as(&author, async |tx| tx.execute(&with_status).await)
        .await
        .unwrap_err();
    assert_refused(&e, "42501", "permission denied for table posts");

    // The catalog check sees a column grant revoked and one added.
    apply(
        &admin,
        &format!(
            "REVOKE UPDATE (title) ON {s}.posts FROM {m}; GRANT UPDATE (author_id) ON {s}.posts TO {m}",
            m = n.member
        ),
    )
    .await;
    let drift = catalog::drift(&scoped, &snapshot).await.unwrap();
    assert_eq!(
        drift,
        [
            format!("table posts: {} lacks UPDATE (title)", n.member),
            format!(
                "table posts: {} has UPDATE (author_id); the schema does not grant it",
                n.member
            ),
        ]
    );

    // The migration to a guarded mask: the column grants go, the table
    // grant comes back, and `rls make` would warn.
    apply(
        &admin,
        &format!(
            "REVOKE UPDATE (author_id) ON {s}.posts FROM {m}; GRANT UPDATE (title) ON {s}.posts TO {m}",
            m = n.member
        ),
    )
    .await;
    let guarded = Schema::new()
        .role(member.clone())
        .role(Role::existing(&n.login))
        .policies(Policy::from_rules::<GuardedMask>(&member).sequence("posts_id_seq"));
    assert_eq!(guarded.warnings().len(), 1, "{:?}", guarded.warnings());
    let next = guarded.snapshot().unwrap();
    let up = brocanth_rls::diff(&snapshot, &next).unwrap().sql();
    apply(&admin, &format!("SET LOCAL search_path = {s};\n{up}")).await;
    let drift = catalog::drift(&scoped, &next).await.unwrap();
    assert!(drift.is_empty(), "{drift:#?}");
    // The database no longer stops the owner column; the policy's
    // WITH CHECK still keeps the row the author's.
    let keep = set("author_id", Expr::val(1));
    let changed = login
        .transact_as(&author, async |tx| tx.execute(&keep).await)
        .await
        .unwrap();
    assert_eq!(changed, 1);

    scoped.close().await;
    login.close().await;
    teardown(&admin, &n).await;
}

#[derive(Clone, Debug, Default, brocanth_policy::Subject)]
pub struct Reader {
    #[attr]
    pub user_id: Option<i64>,
    #[attr]
    pub team: Option<uuid::Uuid>,
    #[attr]
    pub staff: bool,
}

brocanth_policy::policy! {
    pub entity Ticket in "tickets" {
        #[key] id: i64,
        author_id: Option<i64>,
        team_id: Option<uuid::Uuid>,
    }

    #[db]
    pub policy OwnTickets(who: &Reader) for Ticket {
        list: author_id == who.user_id else deny,
    }

    #[db]
    pub policy TeamTickets(who: &Reader) for Ticket {
        list: team_id == who.team else deny,
    }

    /// A guard arm that reads no column, in an `OR` with one that does.
    #[db]
    pub policy StaffTickets(who: &Reader) for Ticket {
        list {
            who.staff => always,
            author_id == who.user_id else deny,
        }
    }
}

/// Applies `schema`'s SQL in the test's schema, with schema `USAGE`.
async fn apply_in(admin: &Db, n: &Names, sql: &str) {
    let s = &n.schema;
    apply(
        admin,
        &format!(
            "SET LOCAL search_path = {s};\n{sql}\nGRANT USAGE ON SCHEMA {s} TO {m};",
            m = n.member
        ),
    )
    .await;
}

fn ticket_schema(n: &Names, policies: impl brocanth_rls::IntoPolicies) -> Schema {
    let member = Role::new(&n.member).member(&n.login);
    Schema::new()
        .role(member)
        .role(Role::existing(&n.login))
        .policies(policies)
}

/// `P`'s policies on `tickets`, in place of the previous ones.
async fn use_policy<P: brocanth_policy::DbPolicy<Subject = Reader, Entity = Ticket>>(
    admin: &Db,
    n: &Names,
) {
    let member = Role::new(&n.member).member(&n.login);
    let sql = ticket_schema(n, Policy::from_rules::<P>(&member))
        .create_sql()
        .unwrap();
    let drop = format!(
        "DROP POLICY IF EXISTS \"tickets_{}_select\" ON tickets;",
        n.member
    );
    apply_in(admin, n, &format!("{drop}\n{sql}")).await;
}

/// The plan of `SELECT id FROM tickets` as `who`, under the policies in
/// place.
async fn plan(login: &Db, n: &Names, who: &Reader) -> String {
    let member = Role::new(&n.member).member(&n.login);
    let me = brocanth_rls::identity(&member, who);
    let explain = format!("EXPLAIN SELECT id FROM {}.tickets", n.schema);
    let lines: Vec<String> = login
        .transact_as(&me, async |tx| {
            brocanth_db::sqlx::query_scalar(brocanth_db::sqlx::AssertSqlSafe(explain))
                .fetch_all(tx.connection())
                .await
                .map_err(DbError::from)
        })
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    lines.join("\n")
}

#[tokio::test]
async fn a_setting_comparison_and_a_guard_chain_keep_the_index() {
    let url = require_db!();
    let n = Names::unique("rls_explain");
    let admin = connect(&url, None).await;
    teardown(&admin, &n).await;
    let s = &n.schema;
    apply(
        &admin,
        &format!(
            "CREATE ROLE {login} LOGIN PASSWORD 'pw' NOINHERIT;
             CREATE SCHEMA {s};
             CREATE TABLE {s}.tickets (id bigint PRIMARY KEY, author_id bigint, team_id uuid);
             INSERT INTO {s}.tickets
               SELECT g, g % 5000, md5(g::text)::uuid FROM generate_series(1, 50000) AS g;
             CREATE INDEX tickets_author_id_idx ON {s}.tickets (author_id);
             CREATE INDEX tickets_team_id_idx ON {s}.tickets (team_id);
             ANALYZE {s}.tickets;",
            login = n.login,
        ),
    )
    .await;
    let login = connect(&url, Some(&n.login)).await;
    let who = Reader {
        user_id: Some(7),
        team: Some(uuid::Uuid::from_u128(7)),
        staff: false,
    };
    let staff = Reader {
        staff: true,
        ..who.clone()
    };

    // `(s IS NOT NULL AND col = s)`: the setting read is STABLE, so the
    // planner compares the column with it through the index. A cast on the
    // column (a `text` setting for a `uuid` column) would lose it.
    use_policy::<OwnTickets>(&admin, &n).await;
    let p = plan(&login, &n, &who).await;
    assert!(p.contains("Index Cond: (author_id = "), "{p}");
    assert!(p.contains("tickets_author_id_idx"), "{p}");
    use_policy::<TeamTickets>(&admin, &n).await;
    let p = plan(&login, &n, &who).await;
    assert!(p.contains("Index Cond: (team_id = "), "{p}");
    assert!(p.contains("tickets_team_id_idx"), "{p}");

    // A guard chain as the bare boolean form (`SqlExpr::from_rule`, and
    // `Policy::from_rules` before the key gate): the `staff` arm has no
    // index condition, so every subject scans the table.
    let member = Role::new(&n.member).member(&n.login);
    let rules = <StaffTickets as brocanth_policy::DbPolicy>::rules();
    let bare = brocanth_rls::SqlExpr::from_rule(
        &rules.list,
        &brocanth_rls::Table::named("tickets"),
        <Reader as Subject>::ATTRS,
    )
    .unwrap();
    let before = ticket_schema(
        &n,
        Policy::from_rules::<StaffTickets>(&member).manual(brocanth_rls::Command::Select, bare),
    );
    let drop = format!(
        "DROP POLICY IF EXISTS \"tickets_{}_select\" ON tickets;",
        n.member
    );
    apply_in(
        &admin,
        &n,
        &format!("{drop}\n{}", before.create_sql().unwrap()),
    )
    .await;
    let p = plan(&login, &n, &who).await;
    assert!(p.starts_with("Seq Scan on tickets"), "{p}");

    // `rls make` from that shape: the diff alters the one policy in place,
    // and the catalog then matches the new snapshot.
    let after = ticket_schema(&n, Policy::from_rules::<StaffTickets>(&member));
    let up = brocanth_rls::diff(&before.snapshot().unwrap(), &after.snapshot().unwrap())
        .unwrap()
        .sql();
    // The policy keeps its name, so one ALTER POLICY moves it.
    assert!(up.contains("ALTER POLICY"), "{up}");
    assert!(!up.contains("DROP POLICY"), "{up}");
    assert!(up.contains(r#""id" >= (CASE WHEN"#), "{up}");
    apply_in(&admin, &n, &up).await;
    let scoped = admin_in(&url, s).await;
    let drift = catalog::drift(&scoped, &after.snapshot().unwrap())
        .await
        .unwrap();
    assert!(drift.is_empty(), "{drift:#?}");

    // With the key gate, the `staff` arm is an index condition on the
    // primary key, NULL for a subject who is not staff: a `BitmapOr` of
    // the key (no row) and the author index. Staff read every row.
    let p = plan(&login, &n, &who).await;
    assert!(p.contains("BitmapOr"), "{p}");
    assert!(
        p.contains("Bitmap Index Scan on tickets_author_id_idx"),
        "{p}"
    );
    assert!(p.contains("Bitmap Index Scan on tickets_pkey"), "{p}");
    let p = plan(&login, &n, &staff).await;
    assert!(p.starts_with("Seq Scan on tickets"), "{p}");

    scoped.close().await;
    login.close().await;
    teardown(&admin, &n).await;
}
