//! The notifications table under row-level security, on a real Postgres.
//!
//! The policy below is the one the `notifications` module writes into a
//! project with `policy` and `rls` (`src/scaffold.rs`, `DB_POLICY`): a
//! reader sees and marks read only the rows of their own account; any
//! request may write one. `Policy::from_rules` turns it into the database
//! policies, on a forced table, and the store runs as a login role that is
//! not the owner and not a superuser. So the database alone decides which
//! rows come back.
//!
//! The `db { … }` clause also runs the agreement test of
//! docs/policy-rls-bridge.md decision 10 on the same URL.
//!
//! ```sh
//! BROCANTH_TEST_DATABASE_URL=postgres://postgres:pg@localhost:55510/postgres \
//!   cargo test -p brocanth-notifications --test rls
//! ```
//!
//! The URL's user must be a superuser: the test creates roles. Without a
//! URL, each test says `!! SKIPPED` on stderr and passes.
#![allow(missing_docs)]

use std::str::FromStr as _;
use std::sync::atomic::{AtomicU32, Ordering};

use brocanth_db::sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use brocanth_db::sqlx::{self, AssertSqlSafe};
use brocanth_db::{Db, DbConfig, DbIdentity};
use brocanth_notifications::{Notice, NotificationStore, SCHEMA};
use brocanth_rls::{Policy, Role, Schema};

/// The subject: only the attribute the notifications policy reads.
#[derive(Clone, Debug, Default, brocanth_policy::Subject)]
pub struct Visitor {
    /// The signed-in account, as text.
    #[attr]
    pub account: Option<String>,
    /// Not read by the notifications policy: an admin reads only their own
    /// inbox.
    #[attr]
    pub admin: bool,
}

brocanth_policy::policy! {
    pub entity Notification in "brocanth_notifications" {
        #[key] id: i64,
        account: String,
        kind: String,
        title: String,
    }

    #[db]
    pub policy NotificationPolicy(who: &Visitor) for Notification {
        list: account == who.account else deny,
        create: always,
        update: account == who.account else deny,
        db {
            subjects: [
                Visitor::default(),
                Visitor { account: Some("1".to_string()), ..Visitor::default() },
                Visitor { admin: true, ..Visitor::default() },
            ],
        }
    }
}

fn url() -> Option<String> {
    brocanth_rls::agreement::test_database_url()
}

fn skipped() {
    use std::io::Write as _;
    let name = std::thread::current().name().unwrap_or("a test").to_owned();
    let _ = std::io::stderr()
        .write_all(format!("!! SKIPPED {name}: set BROCANTH_TEST_DATABASE_URL\n").as_bytes());
}

async fn exec(db: &Db, sql: &str) {
    sqlx::raw_sql(AssertSqlSafe(sql.to_owned()))
        .execute(db.pool())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

/// One test's schema and roles. Roles are cluster-wide, so each name
/// carries the process id and a counter.
struct Names {
    schema: String,
    member: String,
    login: String,
}

impl Names {
    fn unique() -> Names {
        static N: AtomicU32 = AtomicU32::new(0);
        let prefix = format!(
            "notif_rls_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        );
        Names {
            schema: prefix.clone(),
            member: format!("{prefix}_member"),
            login: format!("{prefix}_login"),
        }
    }

    fn role(&self) -> Role {
        Role::new(&self.member).member(&self.login)
    }

    async fn teardown(&self, admin: &Db) {
        let mut sql = format!("DROP SCHEMA IF EXISTS {} CASCADE;", self.schema);
        for role in [&self.member, &self.login] {
            sql.push_str(&format!(
                "DO $$ BEGIN IF EXISTS (SELECT FROM pg_roles WHERE rolname = '{role}') THEN
                     EXECUTE 'DROP OWNED BY {role} CASCADE'; EXECUTE 'DROP ROLE {role}';
                 END IF; END $$;"
            ));
        }
        exec(admin, &sql).await;
    }
}

/// The schema `rls make` writes for this policy, as the module's
/// `.policies(…)` line declares it.
fn rls_schema(n: &Names) -> Schema {
    Schema::new()
        .role(n.role())
        .role(Role::existing(&n.login))
        .policies(
            Policy::from_rules::<NotificationPolicy>(&n.role())
                .sequence("brocanth_notifications_id_seq"),
        )
}

/// What `brocanth_rls::identity` builds for `who` with this test's role.
fn identity(n: &Names, who: &Visitor) -> DbIdentity {
    brocanth_rls::identity(&n.role(), who)
}

fn reader(n: &Names, account: &str) -> DbIdentity {
    identity(
        n,
        &Visitor {
            account: Some(account.to_string()),
            ..Visitor::default()
        },
    )
}

/// A pool as the login role, with the test's schema on its `search_path`.
async fn login_pool(url: &str, n: &Names) -> Db {
    let options = PgConnectOptions::from_str(url)
        .expect("a Postgres URL")
        .username(&n.login)
        .password("pw")
        .options([("search_path", n.schema.as_str())]);
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .expect("connect as the login role");
    Db::from_pool(pool)
}

#[tokio::test]
async fn a_reader_sees_and_marks_only_their_own_rows_on_a_forced_table() {
    let Some(url) = url() else {
        skipped();
        return;
    };
    let n = Names::unique();
    let admin = Db::connect(&DbConfig::new(&url)).await.expect("connect");
    n.teardown(&admin).await;
    // The table belongs to the superuser; the policies force it, and the
    // login role reaches it only through the member role's grants.
    let schema = rls_schema(&n);
    let sql = schema.create_sql().expect("the policy translates");
    exec(
        &admin,
        &format!(
            "CREATE ROLE {login} LOGIN PASSWORD 'pw' NOINHERIT;
             CREATE SCHEMA {s};
             SET search_path = {s};
             {SCHEMA};
             {sql}
             GRANT USAGE ON SCHEMA {s} TO {member};
             RESET search_path;",
            login = n.login,
            member = n.member,
            s = n.schema,
        ),
    )
    .await;
    let snapshot = schema.snapshot().expect("a snapshot");
    assert!(snapshot.tables["brocanth_notifications"].force);

    let login = login_pool(&url, &n).await;
    // The writer: an anonymous request. `create` is `always`, so it writes
    // a row for any account without reading it back.
    let writer =
        NotificationStore::new(login.clone()).as_identity(identity(&n, &Visitor::default()));
    let ada_1 = writer
        .notify("ada", &Notice::new("comment", "ada 1"))
        .await
        .expect("stored");
    let ada_2 = writer
        .notify("ada", &Notice::new("comment", "ada 2"))
        .await
        .expect("stored");
    let bob_1 = writer
        .notify("bob", &Notice::new("invoice", "bob 1"))
        .await
        .expect("stored");
    assert!(ada_1 < ada_2 && ada_2 < bob_1);
    // The anonymous writer reads nothing back.
    assert_eq!(writer.inbox("ada").unread().await.unwrap(), 0);

    let as_ada = NotificationStore::new(login.clone()).as_identity(reader(&n, "ada"));
    let inbox = as_ada.inbox("ada");
    assert_eq!(inbox.unread().await.unwrap(), 2);
    let titles: Vec<String> = inbox
        .list(None, 50, None)
        .await
        .unwrap()
        .into_iter()
        .map(|item| item.title)
        .collect();
    assert_eq!(titles, ["ada 2", "ada 1"]);

    // The application asks for Bob's inbox as Ada: a bug one floor up. The
    // database returns none of his rows and marks none of them read.
    let forged = as_ada.inbox("bob");
    assert_eq!(forged.unread().await.unwrap(), 0);
    assert!(forged.list(None, 50, None).await.unwrap().is_empty());
    assert!(!forged.mark_read(bob_1).await.unwrap());
    assert_eq!(forged.mark_all_read().await.unwrap(), 0);

    // A statement with no `WHERE` at all, as Ada: her rows alone, and an
    // update of Bob's row touches nothing.
    let me = reader(&n, "ada");
    let (seen, touched) = login
        .transact_as(&me, async |tx| {
            let seen: Vec<String> =
                sqlx::query_scalar("SELECT account FROM brocanth_notifications ORDER BY id")
                    .fetch_all(tx.connection())
                    .await?;
            let touched =
                sqlx::query("UPDATE brocanth_notifications SET read_at = now() WHERE id = $1")
                    .bind(bob_1)
                    .execute(tx.connection())
                    .await?
                    .rows_affected();
            Ok((seen, touched))
        })
        .await
        .unwrap();
    assert_eq!(seen, ["ada", "ada"]);
    assert_eq!(touched, 0);

    // Ada cannot give her row to Bob: `WITH CHECK` refuses the new row.
    let given = login
        .transact_as(&me, async |tx| {
            sqlx::query("UPDATE brocanth_notifications SET account = 'bob' WHERE id = $1")
                .bind(ada_1)
                .execute(tx.connection())
                .await
                .map_err(brocanth_db::DbError::from)
        })
        .await
        .expect_err("refused");
    assert_eq!(given.code(), Some("42501"), "{given}");

    // Her own row she marks read.
    assert!(inbox.mark_read(ada_1).await.unwrap());
    assert_eq!(inbox.unread().await.unwrap(), 1);

    // Bob's row is still unread, as Bob reads it.
    let as_bob = NotificationStore::new(login.clone()).as_identity(reader(&n, "bob"));
    assert_eq!(as_bob.inbox("bob").unread().await.unwrap(), 1);

    // No identity: the login role holds no grant of its own, not even
    // `USAGE` on the schema, so the table is not there for it.
    let bare = NotificationStore::new(login.clone());
    let e = bare.inbox("ada").unread().await.expect_err("no grant");
    assert!(e.to_string().contains("does not exist"), "{e}");

    login.close().await;
    n.teardown(&admin).await;
    admin.close().await;
}
