//! The same guarantees, on Postgres, which is the backend this crate is built
//! for.
//!
//! Compiled only with `--features postgres` (the default), and skipped at run
//! time unless `BROCANTH_MODEL_TEST_POSTGRES` or `BROCANTH_TEST_DATABASE_URL`
//! names a database:
//!
//! ```sh
//! BROCANTH_MODEL_TEST_POSTGRES=postgres://localhost/brocanth_model_test \
//!   cargo test -p brocanth-model --test postgres
//! ```
//!
//! **Skipping rather than failing**, because `cargo test -p brocanth-model`
//! has to pass on a laptop with no server running. Every skip prints a line
//! that says so: a Postgres test that passes because there was no Postgres is
//! worse than no test at all.
//!
//! The honest statement of the cost, since Postgres is now the default
//! feature: the suite that runs unattended is `blog.rs` and `eager_load.rs` on
//! SQLite, so the *default* build is the less-tested one until a server is
//! there. These tests are what closes the gap, and they cover the places where
//! the two dialects can actually differ — the migration DDL, the uniqueness
//! lookup, savepoints, the two-query eager loader, the bulk-write path and the
//! probe's metric callback — rather than re-running assertions about pure Rust.

#![cfg(feature = "postgres")]

#[path = "../fixtures/blog.rs"]
mod blog;

use std::sync::{Arc, Mutex};

use blog::{migration::Migrator, post, user};
use brocanth_model::bulk::{self, HooksBypassed};
use brocanth_model::prelude::*;
use brocanth_model::testing::Harness;
use brocanth_model::{EagerStrategy, Probe, ProbeConfig, Tracked};
use sea_orm::sea_query::IntoCondition;
use sea_orm::{ConnectionTrait, PaginatorTrait, Statement};

/// A migrated harness, or `None` when no variable named a server.
async fn harness() -> Option<Harness> {
    Harness::postgres_migrated_from_env::<Migrator>()
        .await
        .expect("connecting to Postgres and migrating")
}

/// The same, with a probe attached.
async fn harness_with(probe: Probe) -> Option<Harness> {
    let url = Harness::postgres_url()?;
    Some(
        Harness::postgres_migrated_with::<Migrator>(&url, probe)
            .await
            .expect("connecting to Postgres and migrating"),
    )
}

macro_rules! or_skip {
    ($made:expr) => {
        match $made {
            Some(harness) => harness,
            None => {
                brocanth_model::testing::skipped(
                    "no Postgres server; set BROCANTH_MODEL_TEST_POSTGRES or \
                     BROCANTH_TEST_DATABASE_URL to run this",
                );
                return;
            }
        }
    };
}

// ---------------------------------------------------------------------------
// 1. The migration DDL is valid Postgres, and the test transaction rolls back
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_migrations_build_a_usable_postgres_schema() {
    let harness = or_skip!(harness().await);
    let db = harness.begin().await.expect("begin");

    blog::seed::blog(&*db, 2, 2).await.expect("seed");
    assert_eq!(user::Entity::find().count(&*db).await.expect("count"), 2);
    assert_eq!(post::Entity::find().count(&*db).await.expect("count"), 4);

    // The foreign key is real, not decorative.
    let orphan = post::ActiveModel {
        user_id: Set(999_999),
        title: Set("No author".to_owned()),
        slug: Set("no-author-pg".to_owned()),
        status: Set("draft".to_owned()),
        views: Set(0),
        ..Default::default()
    }
    .insert_checked(&*db)
    .await;
    assert!(orphan.is_err(), "the foreign key rejected the orphan");
}

/// Every other test here relies on this, so it is written down rather than
/// assumed: the harness transaction is the only thing keeping parallel tests
/// on one shared server from seeing each other's rows.
#[tokio::test]
async fn nothing_a_test_writes_survives_its_transaction() {
    let harness = or_skip!(harness().await);
    let mine = "rollback-witness@example.com";

    {
        let db = harness.begin().await.expect("begin");
        user::ActiveModel {
            name: Set("Witness".to_owned()),
            email: Set(mine.to_owned()),
            ..Default::default()
        }
        .insert_checked(&*db)
        .await
        .expect("insert");
        assert_eq!(
            user::Entity::find()
                .filter(user::Column::Email.eq(mine))
                .count(&*db)
                .await
                .expect("count"),
            1
        );
    }

    assert_eq!(
        user::Entity::find()
            .filter(user::Column::Email.eq(mine))
            .count(harness.conn())
            .await
            .expect("count"),
        0,
        "the guard has no commit, so the row went away"
    );
}

// ---------------------------------------------------------------------------
// 2. Validation, and the uniqueness rule that has to query
// ---------------------------------------------------------------------------

#[tokio::test]
async fn validation_rejects_before_any_statement_and_uniqueness_queries() {
    let probe = Probe::default();
    let harness = or_skip!(harness_with(probe.clone()).await);
    let db = harness.begin().await.expect("begin");

    // `email` is NotSet, so the uniqueness rule has nothing to look up.
    let unit = probe.unit_of_work("an invalid insert");
    let err = user::ActiveModel {
        name: Set("".to_owned()),
        ..Default::default()
    }
    .insert_checked(&*db)
    .await
    .expect_err("blank");
    let report = unit.finish();

    assert!(err.errors().expect("errors").has_code("name", "blank"));
    assert_eq!(
        report.statements, 0,
        "the write was refused before any statement was issued"
    );

    // A set `email` does reach SQL, once, for the uniqueness lookup, and the
    // lookup is the only statement: the INSERT never happens.
    let unit = probe.unit_of_work("an invalid insert with a set unique column");
    let err = user::ActiveModel {
        name: Set("Ada".to_owned()),
        email: Set("not-an-email".to_owned()),
        ..Default::default()
    }
    .insert_checked(&*db)
    .await
    .expect_err("bad format");
    let report = unit.finish();

    assert_eq!(
        err.errors().expect("errors").on("email"),
        ["is not an email address"]
    );
    assert_eq!(report.statements, 1, "only the uniqueness SELECT");
    assert!(
        !report.suspects.iter().any(|s| s.shape.contains("INSERT")),
        "no INSERT was issued"
    );

    // And the hook fires on SeaORM's own `insert`, with no brocanth helper.
    let raw = user::ActiveModel {
        name: Set("".to_owned()),
        email: Set("raw-pg@example.com".to_owned()),
        ..Default::default()
    }
    .insert(&*db)
    .await
    .expect_err("the before_save hook rejects it");
    assert!(Error::from(raw).is_invalid());
}

#[tokio::test]
async fn uniqueness_does_not_collide_a_record_with_itself_on_postgres() {
    let harness = or_skip!(harness().await);
    let db = harness.begin().await.expect("begin");

    let ada = user::ActiveModel {
        name: Set("Ada".to_owned()),
        email: Set("ada-pg@example.com".to_owned()),
        ..Default::default()
    }
    .insert_checked(&*db)
    .await
    .expect("insert");

    let clash = user::ActiveModel {
        name: Set("Ada again".to_owned()),
        email: Set("ada-pg@example.com".to_owned()),
        ..Default::default()
    }
    .insert_checked(&*db)
    .await
    .expect_err("duplicate email");
    assert!(clash.errors().expect("errors").has_code("email", "taken"));

    let mut again = user::ActiveModel {
        id: Unchanged(ada.id),
        ..Default::default()
    };
    again.name = Set("Ada Lovelace".to_owned());
    again.email = Set("ada-pg@example.com".to_owned());
    let saved = again.update_checked(&*db).await.expect("update");
    assert_eq!(saved.name, "Ada Lovelace");
}

// ---------------------------------------------------------------------------
// 3. Savepoints, where the two dialects disagree about how to prove it
// ---------------------------------------------------------------------------

/// On SQLite the proof is that a second `BEGIN` is an error, so the nested
/// `begin()` cannot have issued one. **Postgres answers a second `BEGIN` with
/// a warning and carries on**, so that proof does not transfer and the
/// behaviour has to be checked from the other side: rolling the inner
/// transaction back keeps the outer one's row, which only a savepoint does.
#[tokio::test]
async fn a_nested_transaction_is_a_real_savepoint_on_postgres() {
    let harness = or_skip!(harness().await);
    let outer = harness.begin().await.expect("begin");

    user::ActiveModel {
        name: Set("Outer".to_owned()),
        email: Set("outer-pg@example.com".to_owned()),
        ..Default::default()
    }
    .insert_checked(&*outer)
    .await
    .expect("insert");

    let inner = outer.begin().await.expect("savepoint");
    user::ActiveModel {
        name: Set("Discarded".to_owned()),
        email: Set("discarded-pg@example.com".to_owned()),
        ..Default::default()
    }
    .insert_checked(&inner)
    .await
    .expect("insert");
    inner.rollback().await.expect("rollback savepoint");

    // A plain ROLLBACK would have taken the outer row with it, and a failed
    // statement without a savepoint would have left the session in the
    // "current transaction is aborted" state, where the next query errors.
    assert_eq!(
        user::Entity::find()
            .filter(user::Column::Name.eq("Outer"))
            .count(&*outer)
            .await
            .expect("count"),
        1,
        "the outer row survived, so the inner rollback was a savepoint"
    );

    let inner = outer.begin().await.expect("savepoint");
    user::ActiveModel {
        name: Set("Kept".to_owned()),
        email: Set("kept-pg@example.com".to_owned()),
        ..Default::default()
    }
    .insert_checked(&inner)
    .await
    .expect("insert");
    inner.commit().await.expect("release savepoint");
    assert_eq!(
        user::Entity::find()
            .filter(user::Column::Name.eq("Kept"))
            .count(&*outer)
            .await
            .expect("count"),
        1
    );
}

/// Postgres rolls back DDL, so a test can build and discard a table. SQLite
/// cannot be relied on for this, which is why [`Harness::sqlite`] hands out a
/// private database instead and why the Postgres harness migrates under an
/// advisory lock rather than per test.
#[tokio::test]
async fn ddl_inside_the_test_transaction_rolls_back_too() {
    let harness = or_skip!(harness().await);

    {
        let db = harness.begin().await.expect("begin");
        db.execute_unprepared("CREATE TABLE scratch_pg (id integer primary key)")
            .await
            .expect("create");
        db.execute_unprepared("INSERT INTO scratch_pg (id) VALUES (1)")
            .await
            .expect("insert");
    }

    let after = harness
        .conn()
        .query_all_raw(Statement::from_string(
            harness.conn().get_database_backend(),
            "SELECT to_regclass('scratch_pg') IS NULL AS gone",
        ))
        .await
        .expect("query");
    assert!(
        after[0].try_get::<bool>("", "gone").expect("gone"),
        "transactional DDL: the table went away with the transaction"
    );
}

// ---------------------------------------------------------------------------
// 4. The eager-load fix, on the backend that will actually carry the traffic
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_limited_eager_load_returns_the_right_parents_and_costs_two_queries() {
    let probe = Probe::default();
    let harness = or_skip!(harness_with(probe.clone()).await);
    let db = harness.begin().await.expect("begin");
    blog::seed::blog(&*db, 10, 3).await.expect("seed");

    let query = user::Entity::find().eager().limit(10);
    assert_eq!(query.strategy(), EagerStrategy::TwoQuery);

    let unit = probe.unit_of_work("limited eager load");
    let rows = query
        .with_related::<post::Entity, _>(&*db)
        .await
        .expect("query");
    let report = unit.finish();

    assert_eq!(rows.len(), 10, "ten users, not ten joined rows");
    assert!(rows.iter().all(|r| r.related().len() == 3));
    assert_eq!(report.statements, 2, "parents, then children by key");
    assert!(!report.has_n_plus_one());
}

/// SQLite will not accept an `OFFSET` with no `LIMIT`, so the SQLite suite can
/// only check the chosen strategy. Postgres will, so here the query runs.
#[tokio::test]
async fn a_bare_offset_eager_load_runs_on_postgres() {
    let harness = or_skip!(harness().await);
    let db = harness.begin().await.expect("begin");
    blog::seed::blog(&*db, 5, 2).await.expect("seed");

    let query = user::Entity::find().eager().offset(2);
    assert_eq!(query.strategy(), EagerStrategy::TwoQuery);

    let rows = query
        .with_related::<post::Entity, _>(&*db)
        .await
        .expect("query");
    assert_eq!(rows.len(), 3, "five users, skipping two");
    assert!(rows.iter().all(|r| r.related().len() == 2));
}

// ---------------------------------------------------------------------------
// 5. Bulk writes, dirty tracking and the probe
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_fan_out_path_runs_hooks_on_postgres_and_the_bypass_does_not() {
    let harness = or_skip!(harness().await);
    let db = harness.begin().await.expect("begin");
    blog::seed::blog(&*db, 3, 1).await.expect("seed");

    let err = bulk::update_where::<post::Entity, _, _>(
        post::Column::Status.eq("published").into_condition(),
        |p| p.status = Set("archived".to_owned()),
        &*db,
    )
    .await
    .expect_err("archived is not an allowed status");
    assert!(err
        .errors()
        .expect("errors")
        .has_code("status", "inclusion"));
    assert_eq!(
        post::Entity::find()
            .filter(post::Column::Status.eq("archived"))
            .count(&*db)
            .await
            .expect("count"),
        0,
        "the savepoint rolled back, so nothing was written"
    );

    let changed = bulk::update_where::<post::Entity, _, _>(
        post::Column::Status.eq("published").into_condition(),
        |p| p.views = Set(10),
        &*db,
    )
    .await
    .expect("update");
    assert_eq!(changed.len(), 3);
    assert!(changed.iter().all(|p| p.views == 10));

    let witness = HooksBypassed::because("test: proving the documented bypass");
    bulk::unchecked::update_many::<post::Entity>(witness)
        .col_expr(
            post::Column::Status,
            sea_orm::sea_query::Expr::value("archived"),
        )
        .exec(&*db)
        .await
        .expect("bulk update");
    assert_eq!(
        post::Entity::find()
            .filter(post::Column::Status.eq("archived"))
            .count(&*db)
            .await
            .expect("count"),
        3,
        "the bypass skipped the validation, which is exactly what it says it does"
    );
}

#[tokio::test]
async fn dirty_tracking_saves_through_postgres() {
    let harness = or_skip!(harness().await);
    let db = harness.begin().await.expect("begin");

    let row = user::ActiveModel {
        name: Set("Grace".to_owned()),
        email: Set("grace-pg@example.com".to_owned()),
        ..Default::default()
    }
    .insert_checked(&*db)
    .await
    .expect("insert");

    let mut tracked = Tracked::<user::ActiveModel>::load(row);
    tracked.name = Set("Grace Hopper".to_owned());
    assert_eq!(tracked.changed(), ["name"]);
    assert_eq!(tracked.was("name"), Some("Grace".into()));

    let saved = tracked.save(&*db).await.expect("save");
    assert_eq!(saved.name, "Grace Hopper");
}

/// The probe hangs off `set_metric_callback`, which SeaORM implements per
/// driver. This is the test that it is wired up for the Postgres driver too.
#[tokio::test]
async fn the_probe_flags_a_hand_rolled_n_plus_one_on_postgres() {
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let probe = Probe::new(ProbeConfig {
        on_detect: Some(Arc::new(move |r| {
            sink.lock().expect("lock").push(r.to_string());
        })),
        ..Default::default()
    });

    let harness = or_skip!(harness_with(probe.clone()).await);
    let db = harness.begin().await.expect("begin");
    blog::seed::blog(&*db, 5, 2).await.expect("seed");

    {
        let _unit = probe.unit_of_work("GET /users (naive)");
        let users = user::Entity::find().all(&*db).await.expect("users");
        for u in &users {
            let _ = post::Entity::find()
                .filter(post::Column::UserId.eq(u.id))
                .all(&*db)
                .await
                .expect("posts");
        }
    }

    let reports = seen.lock().expect("lock");
    assert_eq!(reports.len(), 1, "one unit, one report");
    assert!(reports[0].contains("N+1 suspected"), "{}", reports[0]);
    assert!(reports[0].contains("5x"), "{}", reports[0]);
}
