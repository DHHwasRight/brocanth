//! The admin's data source over a table under forced row-level security:
//! with `as_identity`, each request reads and writes as the staff member's
//! `DbIdentity`, and without it the table shows no row.
//!
//! docs/policy-rls-bridge.md, decision 13. The source connects as a login
//! that is neither the owner nor a superuser: a superuser bypasses every
//! policy, `FORCE` included. Skipped without `BROCANTH_TEST_DATABASE_URL`.

use std::str::FromStr as _;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use brocanth_admin::data::{AdminData, RequestCx, Scope, Search};
use brocanth_admin::model::{ColumnDescriptor, ColumnKind, ModelDescriptor, PrimaryKey, Value};
use brocanth_admin::{Admin, AdminConfig, Principal, SharedData};
use brocanth_admin_model::{admin_source, DbIdentity};
use brocanth_db::sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use brocanth_model::sea_orm::{
    ConnectionTrait, Database, DatabaseConnection, SqlxPostgresConnector,
};

mod blog {
    pub mod post {
        use brocanth_model::prelude::*;
        use serde::{Deserialize, Serialize};

        #[sea_orm::model]
        #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
        #[sea_orm(table_name = "post")]
        pub struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub author_id: i64,
            pub title: String,
        }

        impl ActiveModelBehavior for ActiveModel {}
    }
}

admin_source! {
    /// One entity, under row-level security.
    pub struct Models;
    "blog::post" => blog::post::Entity,
}

fn post_model() -> ModelDescriptor {
    ModelDescriptor {
        key: "post".into(),
        module_path: "blog::post".into(),
        table: "post".into(),
        columns: vec![
            ColumnDescriptor::new("id", ColumnKind::Integer).primary_key(),
            ColumnDescriptor::new("author_id", ColumnKind::Integer),
            ColumnDescriptor::new("title", ColumnKind::Text),
        ],
    }
}

/// A schema, a member role, a login, and `post` under a forced policy:
/// a member sees the rows whose `author_id` is `brocanth.user_id`.
struct Forced {
    admin: DatabaseConnection,
    /// The login: every privilege on `post`, and no policy.
    login: DatabaseConnection,
    schema: String,
    member: String,
    login_role: String,
}

impl Forced {
    async fn new() -> Option<Forced> {
        static N: AtomicU32 = AtomicU32::new(0);
        let Some(url) = std::env::var("BROCANTH_TEST_DATABASE_URL")
            .ok()
            .filter(|u| !u.is_empty())
        else {
            eprintln!("!! SKIPPED: set BROCANTH_TEST_DATABASE_URL");
            return None;
        };
        let schema = format!(
            "admin_rls_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        );
        let member = format!("{schema}_member");
        let login_role = format!("{schema}_login");
        let admin = Database::connect(&url).await.expect("connect");
        for statement in [
            format!("CREATE SCHEMA \"{schema}\""),
            format!(
                "CREATE TABLE \"{schema}\".post (id SERIAL PRIMARY KEY, author_id BIGINT NOT NULL, title TEXT NOT NULL)"
            ),
            format!(
                "INSERT INTO \"{schema}\".post (author_id, title) VALUES (7, 'seven'), (7, 'also seven'), (8, 'eight')"
            ),
            format!("CREATE ROLE \"{member}\" NOLOGIN"),
            format!("CREATE ROLE \"{login_role}\" LOGIN PASSWORD 'pw' IN ROLE \"{member}\""),
            format!("GRANT USAGE ON SCHEMA \"{schema}\" TO \"{member}\", \"{login_role}\""),
            format!(
                "GRANT SELECT, INSERT, UPDATE, DELETE ON \"{schema}\".post TO \"{member}\", \"{login_role}\""
            ),
            format!("GRANT USAGE ON ALL SEQUENCES IN SCHEMA \"{schema}\" TO \"{member}\""),
            format!("ALTER TABLE \"{schema}\".post ENABLE ROW LEVEL SECURITY"),
            format!("ALTER TABLE \"{schema}\".post FORCE ROW LEVEL SECURITY"),
            format!(
                "CREATE POLICY post_mine ON \"{schema}\".post TO \"{member}\" \
                 USING (author_id = CAST(NULLIF(current_setting('brocanth.user_id', TRUE), '') AS bigint)) \
                 WITH CHECK (author_id = CAST(NULLIF(current_setting('brocanth.user_id', TRUE), '') AS bigint))"
            ),
        ] {
            admin
                .execute_unprepared(&statement)
                .await
                .unwrap_or_else(|e| panic!("{e}: {statement}"));
        }
        let options = PgConnectOptions::from_str(&url)
            .expect("a URL")
            .username(&login_role)
            .password("pw")
            .options([("search_path", schema.as_str())]);
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .expect("the login connects");
        let login = SqlxPostgresConnector::from_sqlx_postgres_pool(pool);
        Some(Forced {
            admin,
            login,
            schema,
            member,
            login_role,
        })
    }

    /// The source over the login, as the staff member's identity: the
    /// member role, and `user_id` from the principal.
    fn source(&self) -> Models {
        let member = self.member.clone();
        Models::new(self.login.clone()).as_identity(move |principal| {
            let user = principal.downcast::<i64>().copied();
            DbIdentity::new()
                .role(&member)
                .set("user_id", user.map(|id| id.to_string()).unwrap_or_default())
        })
    }

    async fn drop_all(self) {
        let _ = self.login.close().await;
        for statement in [
            format!("DROP SCHEMA \"{}\" CASCADE", self.schema),
            format!("DROP ROLE \"{}\"", self.login_role),
            format!("DROP ROLE \"{}\"", self.member),
        ] {
            let _ = self.admin.execute_unprepared(&statement).await;
        }
        let _ = self.admin.close().await;
    }
}

fn everything() -> Search<'static> {
    Search {
        term: "",
        columns: &[],
    }
}

/// The source a request by `who` reads through.
fn for_staff(models: &Models, who: &'static i64) -> SharedData {
    models
        .for_request(&RequestCx::new(Principal::new(who)))
        .expect("a source with an identity serves each request as its own")
}

#[tokio::test]
async fn each_request_reads_and_writes_as_the_staff_members_identity() {
    let Some(db) = Forced::new().await else {
        return;
    };
    let models = db.source();
    let model = post_model();
    static SEVEN: i64 = 7;
    static EIGHT: i64 = 8;

    // Without an identity: the login, which no policy admits.
    let bare = Models::new(db.login.clone());
    assert!(bare
        .for_request(&RequestCx::new(Principal::new(&SEVEN)))
        .is_none());
    assert_eq!(
        bare.count(&model, everything(), Scope::Unscoped)
            .await
            .unwrap(),
        0
    );

    let seven = for_staff(&models, &SEVEN);
    assert_eq!(
        seven
            .count(&model, everything(), Scope::Unscoped)
            .await
            .unwrap(),
        2
    );
    let eight = for_staff(&models, &EIGHT);
    assert_eq!(
        eight
            .count(&model, everything(), Scope::Unscoped)
            .await
            .unwrap(),
        1
    );

    // The row of author 8 is not there for author 7: no row to update.
    let theirs = PrimaryKey::single("id", "3");
    let changed = seven
        .update(
            &model,
            &theirs,
            &[("title".into(), Value::Text("mine now".into()))],
            Scope::Unscoped,
        )
        .await
        .unwrap();
    assert_eq!(changed, 0, "a row the database hides is a lost race");
    assert!(seven
        .fetch(&model, &theirs, Scope::Unscoped)
        .await
        .unwrap()
        .is_none());

    let mine = PrimaryKey::single("id", "1");
    let changed = seven
        .update(
            &model,
            &mine,
            &[("title".into(), Value::Text("edited".into()))],
            Scope::Unscoped,
        )
        .await
        .unwrap();
    assert_eq!(changed, 1);
    let row = seven
        .fetch(&model, &mine, Scope::Unscoped)
        .await
        .unwrap()
        .expect("its own row");
    assert_eq!(row.get("title"), Some(&Value::Text("edited".into())));

    // A save with inline rows: one transaction, as the identity. The
    // database's `WITH CHECK` refuses a row for another author.
    let mut txn = seven.begin().await.unwrap();
    txn.insert(
        &model,
        &[
            ("author_id".into(), Value::Int(7)),
            ("title".into(), Value::Text("new".into())),
        ],
    )
    .await
    .expect("author 7 adds its own row");
    txn.commit().await.unwrap();
    assert_eq!(
        seven
            .count(&model, everything(), Scope::Unscoped)
            .await
            .unwrap(),
        3
    );
    let refused = seven
        .insert(
            &model,
            &[
                ("author_id".into(), Value::Int(8)),
                ("title".into(), Value::Text("theirs".into())),
            ],
        )
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("row-level security"),
        "{refused}"
    );

    assert_eq!(
        eight
            .count(&model, everything(), Scope::Unscoped)
            .await
            .unwrap(),
        1
    );
    db.drop_all().await;
}

/// The identity is checked when the admin is built, at start: a role name
/// Postgres would truncate fails there, with its `help:` line.
#[tokio::test]
async fn a_bad_identity_fails_when_the_admin_is_built() {
    let bad = Models::unconnected(brocanth_model::sea_orm::DbBackend::Postgres)
        .as_identity(|_| DbIdentity::new().role("r".repeat(64)));
    let config = AdminConfig::new();
    let error = Admin::try_new(Arc::new(bad), config)
        .unwrap_err()
        .to_string();
    assert!(error.contains("database identity"), "{error}");
    assert!(error.contains("help:"), "{error}");

    let good = Models::unconnected(brocanth_model::sea_orm::DbBackend::Postgres)
        .as_identity(|_| DbIdentity::new().role("app_user").set("user_id", ""));
    let config = AdminConfig::new();
    assert!(Admin::try_new(Arc::new(good), config).is_ok());
}
