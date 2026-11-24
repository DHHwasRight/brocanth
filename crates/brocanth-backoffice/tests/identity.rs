//! The identity path on Postgres: the admin's SeaORM source with
//! `as_identity`, behind the tab data contract and a JSON codec, over a
//! table under forced row-level security (docs/policy-rls-bridge.md,
//! decision 13; docs/backoffice.md §7.1, D5).
//!
//! The claim crosses the codec, and the data side runs each read, each
//! write and each transaction as the identity it builds from the claim. The
//! source connects as a login that is neither the owner nor a superuser: a
//! superuser bypasses every policy. Skipped without
//! `BROCANTH_TEST_DATABASE_URL`. Each test makes its own schema and roles.

use std::str::FromStr as _;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use brocanth_admin::data::{AdminData, RequestCx, Scope, Search, SharedData};
use brocanth_admin::model::{ModelDescriptor, PrimaryKey, Value};
use brocanth_admin::policy::{AllowAllUnsafeForDevelopment, Principal};
use brocanth_admin::{discover, Admin, AdminConfig, Discovery, ListQuery};
use brocanth_admin_model::{admin_source, DbIdentity};
use brocanth_backoffice::wire::{ServiceSecret, WireClient, WireServer};
use brocanth_backoffice::{Claim, Contracted, InProcess, Json};
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

/// A schema, a member role, a login, and `post` under a forced policy: a
/// member sees the rows whose `author_id` is `brocanth.user_id`.
struct Forced {
    admin: DatabaseConnection,
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
            "backoffice_rls_{}_{}",
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

    /// The data side: the source as the staff member's identity, built
    /// from the claim the call carried. The identity function reads the
    /// `Claim`, not the session type: that is what crosses the contract.
    fn data_side(&self) -> InProcess {
        InProcess::new(self.source())
    }

    /// The admin's source as the staff member's identity, built from the
    /// claim the call carried.
    fn source(&self) -> SharedData {
        let member = self.member.clone();
        let source = Models::new(self.login.clone()).as_identity(move |principal| {
            let user = principal
                .downcast::<Claim>()
                .and_then(|claim| claim.subject.clone())
                .unwrap_or_default();
            DbIdentity::new().role(&member).set("user_id", user)
        });
        Arc::new(source)
    }

    /// The admin's end: the request's principal is a staff id, and the
    /// claim carries it.
    fn admin_side(&self) -> SharedData {
        Contracted::new(Json::new(self.data_side()))
            .claim(staff_claim)
            .shared()
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

/// The claim of a staff id, with the grant the data side checks on the
/// wire.
fn staff_claim(principal: &Principal<'_>) -> Claim {
    match principal.downcast::<i64>() {
        Some(id) => Claim::staff(id.to_string(), false, ["post.edit"]),
        None => Claim::anonymous(),
    }
}

fn everything() -> Search<'static> {
    Search {
        term: "",
        columns: &[],
    }
}

fn post(data: &dyn AdminData) -> ModelDescriptor {
    discover(data.registry(), &Discovery::new())
        .into_iter()
        .find(|m| m.key == "post")
        .expect("post")
}

/// The source one request by `who` reads through, as the admin asks for it.
fn as_staff(data: &SharedData, who: &'static i64) -> SharedData {
    data.for_request(&RequestCx::new(Principal::new(who)))
        .expect("the admin's end binds each request to its claim")
}

#[tokio::test]
async fn each_call_runs_as_the_identity_its_claim_carries() {
    let Some(db) = Forced::new().await else {
        return;
    };
    let data = db.admin_side();
    let model = post(&*data);
    static SEVEN: i64 = 7;
    static EIGHT: i64 = 8;

    let seven = as_staff(&data, &SEVEN);
    let eight = as_staff(&data, &EIGHT);
    assert_eq!(
        seven
            .count(&model, everything(), Scope::Unscoped)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        eight
            .count(&model, everything(), Scope::Unscoped)
            .await
            .unwrap(),
        1
    );
    // Nobody: the identity has no user, and the policy admits no row.
    assert_eq!(
        data.count(&model, everything(), Scope::Unscoped)
            .await
            .unwrap(),
        0
    );

    // A row of 8's is invisible to 7, and so is a write to it.
    let eights = PrimaryKey::single("id", 3);
    assert!(seven
        .fetch(&model, &eights, Scope::Unscoped)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        seven
            .update(
                &model,
                &eights,
                &[("title".into(), Value::from("stolen"))],
                Scope::Unscoped
            )
            .await
            .unwrap(),
        0,
        "the database refused the row, and zero rows is never success"
    );
    let row = eight
        .fetch(&model, &eights, Scope::Unscoped)
        .await
        .unwrap()
        .expect("8 sees its row");
    assert_eq!(row.get("title"), Some(&Value::from("eight")));

    db.drop_all().await;
}

#[tokio::test]
async fn a_transaction_runs_as_the_identity_and_keeps_all_or_nothing() {
    let Some(db) = Forced::new().await else {
        return;
    };
    let data = db.admin_side();
    let model = post(&*data);
    static SEVEN: i64 = 7;
    let seven = as_staff(&data, &SEVEN);

    // A row 7 may write, then one the policy's WITH CHECK refuses: the
    // transaction keeps neither.
    let mut tx = seven.begin().await.expect("a transaction as 7");
    tx.insert(
        &model,
        &[
            ("author_id".into(), Value::Int(7)),
            ("title".into(), Value::from("kept?")),
        ],
    )
    .await
    .expect("7 may write its own row");
    let refused = tx
        .insert(
            &model,
            &[
                ("author_id".into(), Value::Int(8)),
                ("title".into(), Value::from("forged")),
            ],
        )
        .await;
    assert!(refused.is_err(), "a row for 8 breaks the policy");
    drop(tx);
    assert_eq!(
        seven
            .count(&model, everything(), Scope::Unscoped)
            .await
            .unwrap(),
        2,
        "nothing of the dropped transaction was kept"
    );

    // Committed: kept, and seen as 7.
    let mut tx = seven.begin().await.expect("a transaction as 7");
    tx.insert(
        &model,
        &[
            ("author_id".into(), Value::Int(7)),
            ("title".into(), Value::from("kept")),
        ],
    )
    .await
    .expect("7 may write its own row");
    tx.commit().await.expect("a commit");
    assert_eq!(
        seven
            .count(&model, everything(), Scope::Unscoped)
            .await
            .unwrap(),
        3
    );

    db.drop_all().await;
}

#[tokio::test]
async fn the_admin_lists_only_the_rows_of_the_staff_member_who_asks() {
    let Some(db) = Forced::new().await else {
        return;
    };
    let config = AdminConfig::new()
        .unscoped_rows_unsafe_for_development()
        .with_policy(AllowAllUnsafeForDevelopment);
    let admin = Admin::try_new(db.admin_side(), config).expect("the admin builds");
    static EIGHT: i64 = 8;
    let page = admin
        .list(Principal::new(&EIGHT), "post", ListQuery::new())
        .await
        .expect("a page");
    let html = page.html().expect("html");
    assert!(html.contains("eight"), "{html}");
    assert!(!html.contains("also seven"), "{html}");

    db.drop_all().await;
}

/// The admin's end over HTTP: the application's half serves the source on
/// a socket, on this runtime, which owns the pool's connections.
async fn over_the_wire(db: &Forced) -> (SharedData, WireServer, brocanth_server::Stopper) {
    let secret = ServiceSecret::parse("a-service-secret-of-thirty-two-bytes-or-more").unwrap();
    let server = WireServer::new(db.source(), secret.clone());
    let listening = brocanth_server::Server::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let base = format!("http://{}", listening.local_addr().expect("an address"));
    let stopper = listening.stopper();
    let serving = server.clone();
    tokio::spawn(listening.run(move |request| {
        let server = serving.clone();
        async move { server.handle(request).await }
    }));
    let client = WireClient::connect(&base, secret)
        .await
        .expect("it connects");
    let data = Contracted::over(client.shared())
        .claim(staff_claim)
        .shared();
    (data, server, stopper)
}

#[tokio::test(flavor = "multi_thread")]
async fn over_the_wire_each_call_and_transaction_runs_as_the_claims_identity() {
    let Some(db) = Forced::new().await else {
        return;
    };
    let (data, server, stopper) = over_the_wire(&db).await;
    let model = post(&*data);
    static SEVEN: i64 = 7;
    static EIGHT: i64 = 8;
    let seven = as_staff(&data, &SEVEN);
    let eight = as_staff(&data, &EIGHT);

    let count = |data: SharedData| {
        let model = model.clone();
        async move { data.count(&model, everything(), Scope::Unscoped).await }
    };
    assert_eq!(count(Arc::clone(&seven)).await.unwrap(), 2);
    assert_eq!(count(Arc::clone(&eight)).await.unwrap(), 1);
    // Nobody: the wire's data side refuses the claim before the database
    // sees it.
    assert!(count(Arc::clone(&data)).await.is_err());
    let eights = PrimaryKey::single("id", 3);
    assert!(seven
        .fetch(&model, &eights, Scope::Unscoped)
        .await
        .unwrap()
        .is_none());

    // A transaction held by the server between requests, as 7: the row the
    // policy's WITH CHECK refuses ends it, and nothing is kept.
    let mut tx = seven.begin().await.expect("a transaction as 7");
    tx.insert(
        &model,
        &[
            ("author_id".into(), Value::Int(7)),
            ("title".into(), Value::from("kept?")),
        ],
    )
    .await
    .expect("7 may write its own row");
    let refused = tx
        .insert(
            &model,
            &[
                ("author_id".into(), Value::Int(8)),
                ("title".into(), Value::from("forged")),
            ],
        )
        .await;
    assert!(refused.is_err(), "a row for 8 breaks the policy");
    tx.rollback().await.ok();
    assert_eq!(server.open_transactions(), 0);
    assert_eq!(count(Arc::clone(&seven)).await.unwrap(), 2);

    let mut tx = seven.begin().await.expect("a transaction as 7");
    tx.insert(
        &model,
        &[
            ("author_id".into(), Value::Int(7)),
            ("title".into(), Value::from("kept")),
        ],
    )
    .await
    .expect("7 may write its own row");
    tx.commit().await.expect("a commit");
    assert_eq!(count(Arc::clone(&seven)).await.unwrap(), 3);
    assert_eq!(count(eight).await.unwrap(), 1);

    stopper.stop();
    db.drop_all().await;
}
