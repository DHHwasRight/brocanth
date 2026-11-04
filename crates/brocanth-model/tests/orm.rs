//! `Orm` and `brocanth_db::Db` share one pool and see one database.
//!
//! Skipped, with a line that says so, unless `BROCANTH_MODEL_TEST_POSTGRES`
//! or `BROCANTH_TEST_DATABASE_URL` names a server:
//!
//! ```sh
//! BROCANTH_TEST_DATABASE_URL=postgres://localhost/brocanth_test \
//!   cargo test -p brocanth-model --test orm
//! ```
//!
//! The test works in a schema of its own, made at the start and dropped at
//! the end, so it touches no shared table.

#![cfg(feature = "db")]

use std::time::Duration;

use brocanth_db::sea_query::{Alias, Expr, ExprTrait, Query};
use brocanth_db::sqlx::{self, AssertSqlSafe};
use brocanth_db::{Db, DbConfig, Exec};
use brocanth_model::sea_orm::{
    ActiveModelTrait, DbErr, EntityTrait, PaginatorTrait, Set, TransactionTrait,
};
use brocanth_model::testing::{skipped, Harness};
use brocanth_model::Orm;

mod note {
    use brocanth_model::sea_orm::entity::prelude::*;

    #[sea_orm::model]
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "note")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub body: String,
    }

    impl ActiveModelBehavior for ActiveModel {}
}

/// The rows in `note`, counted through sea-query and `Db`.
async fn count(db: &Db) -> i64 {
    let q = Query::select()
        .expr(Expr::cust("count(*)"))
        .from(Alias::new("note"))
        .to_owned();
    let (n,): (i64,) = { db }.one(&q).await.expect("count through Db");
    n
}

#[tokio::test]
async fn the_orm_and_sea_query_share_one_pool_and_one_database() {
    let Some(url) = Harness::postgres_url() else {
        skipped(
            "no Postgres server; set BROCANTH_MODEL_TEST_POSTGRES or \
             BROCANTH_TEST_DATABASE_URL to run this",
        );
        return;
    };

    // A schema of this test's own.
    let schema = format!(
        "brocanth_model_orm_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .subsec_nanos()
    );
    let admin = Db::connect(&DbConfig::new(url.clone()))
        .await
        .expect("connect");
    sqlx::query(AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
        .execute(admin.pool())
        .await
        .expect("create schema");

    // One connection, so a held transaction proves the two handles share
    // the pool: nothing else can take a connection while it is open.
    let mut cfg = DbConfig::new(url);
    cfg.search_path = vec![schema.clone()];
    cfg.max_connections = 1;
    cfg.acquire_timeout = Duration::from_millis(300);
    let db = Db::connect(&cfg).await.expect("connect in the schema");
    sqlx::query("CREATE TABLE note (id serial PRIMARY KEY, body text NOT NULL)")
        .execute(db.pool())
        .await
        .expect("create table");

    // 1. A row written through sea-query is read through an entity.
    let insert = Query::insert()
        .into_table(Alias::new("note"))
        .columns([Alias::new("body")])
        .values_panic(["from sea-query".into()])
        .to_owned();
    assert_eq!({ &db }.execute(&insert).await.expect("insert"), 1);

    let orm = Orm::from_db(&db);
    let notes = note::Entity::find().all(&*orm).await.expect("find");
    assert_eq!(
        notes.iter().map(|n| n.body.as_str()).collect::<Vec<_>>(),
        ["from sea-query"]
    );

    // 2. A committed ORM transaction is visible through Db.
    let id = orm
        .transaction::<_, i32, DbErr>(|tx| {
            Box::pin(async move {
                let made = note::ActiveModel {
                    body: Set("from the orm".to_string()),
                    ..Default::default()
                }
                .insert(tx)
                .await?;
                Ok(made.id)
            })
        })
        .await
        .expect("transaction");
    assert_eq!(count(&db).await, 2);
    let q = Query::select()
        .column(Alias::new("body"))
        .from(Alias::new("note"))
        .and_where(Expr::col(Alias::new("id")).eq(id))
        .to_owned();
    let (body,): (String,) = { &db }.one(&q).await.expect("read back");
    assert_eq!(body, "from the orm");

    // 3. A failed ORM transaction rolls back.
    let failed = orm
        .transaction::<_, (), DbErr>(|tx| {
            Box::pin(async move {
                note::ActiveModel {
                    body: Set("rolled back".to_string()),
                    ..Default::default()
                }
                .insert(tx)
                .await?;
                Err(DbErr::Custom("stop".to_string()))
            })
        })
        .await;
    assert!(failed.is_err());
    assert_eq!(note::Entity::find().count(&*orm).await.expect("count"), 2);

    // 4. One pool: while the ORM holds its only connection, Db waits for it.
    let held = orm.begin().await.expect("begin");
    let waited = db.ping().await;
    assert!(
        waited.is_err(),
        "Db got a second connection from a pool of one"
    );
    held.commit().await.expect("commit");
    db.ping().await.expect("the connection is back in the pool");

    db.close().await;
    sqlx::query(AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
        .execute(admin.pool())
        .await
        .expect("drop schema");
    admin.close().await;
}
