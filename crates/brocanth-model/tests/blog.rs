//! Proof that the model layer runs: migrations, validations, nested
//! transactions, bulk-write hook coverage, dirty tracking, N+1 detection and
//! the entity registry, against in-memory SQLite.

#[path = "../fixtures/blog.rs"]
mod blog;

use blog::{migration::Migrator, post, user};
use brocanth_model::bulk::{self, HooksBypassed};
use brocanth_model::prelude::*;
use brocanth_model::registry::{self, ModelRegistry};
use brocanth_model::testing::Harness;
use brocanth_model::{Probe, ProbeConfig, Tracked};
use sea_orm::sea_query::IntoCondition;
use sea_orm::{ConnectionTrait, PaginatorTrait, Statement};
use std::sync::{Arc, Mutex};

async fn tables(db: &impl ConnectionTrait) -> Vec<String> {
    let rows = db
        .query_all_raw(Statement::from_string(
            db.get_database_backend(),
            "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name",
        ))
        .await
        .expect("sqlite_master");
    rows.iter()
        .map(|r| r.try_get::<String>("", "name").expect("name"))
        .filter(|n| !n.starts_with("sqlite_"))
        .collect()
}

// ---------------------------------------------------------------------------
// 1. A migration up and down
// ---------------------------------------------------------------------------

#[tokio::test]
async fn migrations_go_up_and_come_back_down() {
    let harness = Harness::sqlite().await.expect("connect");

    harness.migrate::<Migrator>().await.expect("up");
    let up = tables(harness.conn()).await;
    assert!(up.contains(&"user".to_owned()));
    assert!(up.contains(&"post".to_owned()));

    harness.rollback::<Migrator>(None).await.expect("down");
    let down = tables(harness.conn()).await;
    assert!(!down.contains(&"user".to_owned()));
    assert!(!down.contains(&"post".to_owned()));
    assert!(down.contains(&"seaql_migrations".to_owned()));

    harness.migrate::<Migrator>().await.expect("up again");
    assert!(tables(harness.conn()).await.contains(&"post".to_owned()));
}

// ---------------------------------------------------------------------------
// 2. A validation rejecting a write before any SQL is issued
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_invalid_record_is_rejected_before_any_statement_is_issued() {
    let probe = Probe::default();
    let harness = Harness::sqlite_with(probe.clone()).await.expect("connect");
    harness.migrate::<Migrator>().await.expect("migrate");
    let db = harness.begin().await.expect("begin");

    // `email` is NotSet, so the uniqueness rule has nothing to look up and the
    // record fails on the in-memory rules alone. Nothing at all reaches SQL.
    let unit = probe.unit_of_work("rejected insert");
    let result = user::ActiveModel {
        name: Set("A".to_owned()),
        ..Default::default()
    }
    .insert_checked(&*db)
    .await;
    let report = unit.finish();

    let err = result.expect_err("should be rejected");
    assert!(err.is_invalid());
    let errors = err.errors().expect("field errors");

    assert!(errors.has_code("name", "too_short"));
    assert!(errors.has_code("email", "blank"));
    assert_eq!(
        errors.full_messages(),
        [
            "Name is too short (minimum is 2 characters)",
            "Email can't be blank",
        ]
    );

    assert_eq!(
        report.statements, 0,
        "the write was refused before any statement was issued"
    );
    assert_eq!(user::Entity::find().count(&*db).await.expect("count"), 0);
}

#[tokio::test]
async fn a_bad_format_is_caught_and_the_only_statement_is_the_uniqueness_lookup() {
    let probe = Probe::default();
    let harness = Harness::sqlite_with(probe.clone()).await.expect("connect");
    harness.migrate::<Migrator>().await.expect("migrate");
    let db = harness.begin().await.expect("begin");

    let unit = probe.unit_of_work("rejected insert with a set unique column");
    let err = user::ActiveModel {
        name: Set("Ada".to_owned()),
        email: Set("not-an-email".to_owned()),
        ..Default::default()
    }
    .insert_checked(&*db)
    .await
    .expect_err("should be rejected");
    let report = unit.finish();

    assert_eq!(
        err.errors().expect("errors").on("email"),
        ["is not an email address"]
    );
    assert_eq!(
        report.statements, 1,
        "only the uniqueness SELECT; a uniqueness rule has to query"
    );
    assert!(
        !report.suspects.iter().any(|s| s.shape.contains("INSERT")),
        "no INSERT was issued"
    );
    assert_eq!(user::Entity::find().count(&*db).await.expect("count"), 0);
}

#[tokio::test]
async fn the_hook_fires_even_on_a_raw_sea_orm_insert() {
    let harness = Harness::migrated::<Migrator>().await.expect("migrate");
    let db = harness.begin().await.expect("begin");

    // No brocanth helper anywhere: SeaORM's own `insert`.
    let err = user::ActiveModel {
        name: Set("".to_owned()),
        email: Set("x@example.com".to_owned()),
        ..Default::default()
    }
    .insert(&*db)
    .await
    .expect_err("the before_save hook rejects it");

    let recovered = Error::from(err);
    assert!(recovered.is_invalid());
    assert!(recovered
        .errors()
        .expect("errors")
        .has_code("name", "blank"));
}

#[tokio::test]
async fn uniqueness_queries_and_does_not_collide_a_record_with_itself() {
    let harness = Harness::migrated::<Migrator>().await.expect("migrate");
    let db = harness.begin().await.expect("begin");

    let ada = user::ActiveModel {
        name: Set("Ada".to_owned()),
        email: Set("ada@example.com".to_owned()),
        ..Default::default()
    }
    .insert_checked(&*db)
    .await
    .expect("insert");

    let clash = user::ActiveModel {
        name: Set("Ada again".to_owned()),
        email: Set("ada@example.com".to_owned()),
        ..Default::default()
    }
    .insert_checked(&*db)
    .await
    .expect_err("duplicate email");
    assert!(clash.errors().expect("errors").has_code("email", "taken"));

    // Saving the same row again must not report itself as a duplicate.
    let mut again = user::ActiveModel {
        id: Unchanged(ada.id),
        ..Default::default()
    };
    again.name = Set("Ada Lovelace".to_owned());
    again.email = Set("ada@example.com".to_owned());
    let saved = again.update_checked(&*db).await.expect("update");
    assert_eq!(saved.name, "Ada Lovelace");
}

#[tokio::test]
async fn valid_answers_without_writing() {
    let harness = Harness::migrated::<Migrator>().await.expect("migrate");
    let db = harness.begin().await.expect("begin");

    let good = post::ActiveModel {
        user_id: Set(1),
        title: Set("A good title".to_owned()),
        slug: Set("a-good-title".to_owned()),
        status: Set("draft".to_owned()),
        views: Set(0),
        ..Default::default()
    };
    assert!(good.valid(&*db).await.expect("check"));

    let bad = post::ActiveModel {
        user_id: Set(1),
        title: Set("ok".to_owned()),
        slug: Set("Not A Slug".to_owned()),
        status: Set("archived".to_owned()),
        views: Set(-1),
        ..Default::default()
    };
    let errors = bad.errors(&*db).await.expect("check");
    assert_eq!(errors.fields(), ["title", "slug", "status", "views"]);
    assert!(errors.has_code("views", "greater_than_or_equal_to"));
    assert!(errors.has_code("status", "inclusion"));
    assert_eq!(user::Entity::find().count(&*db).await.expect("count"), 0);
}

// ---------------------------------------------------------------------------
// 3. A nested transaction producing a real SAVEPOINT
// ---------------------------------------------------------------------------

/// SQLite rejects `BEGIN` inside an open transaction with "cannot start a
/// transaction within a transaction". The nested `begin()` below therefore
/// could not have issued one: it issued a `SAVEPOINT`. The partial rollback
/// proves the same thing from the other side — rolling the inner one back keeps
/// the outer one's row, which only a savepoint does.
#[tokio::test]
async fn a_nested_transaction_is_a_real_savepoint() {
    let harness = Harness::migrated::<Migrator>().await.expect("migrate");
    let outer = harness.begin().await.expect("begin");

    user::ActiveModel {
        name: Set("Outer".to_owned()),
        email: Set("outer@example.com".to_owned()),
        ..Default::default()
    }
    .insert_checked(&*outer)
    .await
    .expect("insert");

    assert!(
        outer
            .execute_raw(Statement::from_string(
                outer.get_database_backend(),
                "BEGIN",
            ))
            .await
            .is_err(),
        "a transaction is open, so a second BEGIN must fail"
    );

    // Discarded savepoint.
    let inner = outer.begin().await.expect("savepoint");
    user::ActiveModel {
        name: Set("Discarded".to_owned()),
        email: Set("discarded@example.com".to_owned()),
        ..Default::default()
    }
    .insert_checked(&inner)
    .await
    .expect("insert");
    assert_eq!(user::Entity::find().count(&inner).await.expect("count"), 2);
    inner.rollback().await.expect("rollback savepoint");

    assert_eq!(
        user::Entity::find().count(&*outer).await.expect("count"),
        1,
        "the outer row survived, so the inner rollback was a savepoint"
    );

    // Released savepoint.
    let inner = outer.begin().await.expect("savepoint");
    user::ActiveModel {
        name: Set("Kept".to_owned()),
        email: Set("kept@example.com".to_owned()),
        ..Default::default()
    }
    .insert_checked(&inner)
    .await
    .expect("insert");
    inner.commit().await.expect("release savepoint");
    assert_eq!(user::Entity::find().count(&*outer).await.expect("count"), 2);

    // The harness guard has no commit, so everything goes away.
    drop(outer);
    assert_eq!(
        user::Entity::find()
            .count(harness.conn())
            .await
            .expect("count"),
        0,
        "the harness transaction can only roll back"
    );
}

// ---------------------------------------------------------------------------
// 4. Hook coverage for set-based writes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_fan_out_path_runs_hooks_and_the_bypass_does_not() {
    let harness = Harness::migrated::<Migrator>().await.expect("migrate");
    let db = harness.begin().await.expect("begin");
    blog::seed::blog(&*db, 3, 1).await.expect("seed");

    // Fan-out: validation rejects the whole batch, atomically.
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
        "the transaction rolled back, so nothing was written"
    );

    // Fan-out with a legal change writes every row and runs every hook.
    let changed = bulk::update_where::<post::Entity, _, _>(
        post::Column::Status.eq("published").into_condition(),
        |p| p.views = Set(10),
        &*db,
    )
    .await
    .expect("update");
    assert_eq!(changed.len(), 3);
    assert!(changed.iter().all(|p| p.views == 10));

    // The bypass writes the same invalid value, because it skips the hook. It
    // needs a written reason to compile.
    let witness = HooksBypassed::because("test: proving the documented bypass");
    assert_eq!(witness.reason(), "test: proving the documented bypass");
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
async fn insert_each_is_atomic_and_validated() {
    let harness = Harness::migrated::<Migrator>().await.expect("migrate");
    let db = harness.begin().await.expect("begin");

    let rows = vec![
        user::ActiveModel {
            name: Set("One".to_owned()),
            email: Set("one@example.com".to_owned()),
            ..Default::default()
        },
        user::ActiveModel {
            name: Set("".to_owned()),
            email: Set("two@example.com".to_owned()),
            ..Default::default()
        },
    ];
    let err = bulk::insert_each(rows, &*db)
        .await
        .expect_err("second row is blank");
    assert!(err.errors().expect("errors").has_code("name", "blank"));
    assert_eq!(
        user::Entity::find().count(&*db).await.expect("count"),
        0,
        "the first row went back too"
    );
}

// ---------------------------------------------------------------------------
// 5. Dirty tracking
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dirty_tracking_reports_what_changed_and_what_it_was() {
    let harness = Harness::migrated::<Migrator>().await.expect("migrate");
    let db = harness.begin().await.expect("begin");

    let row = user::ActiveModel {
        name: Set("Grace".to_owned()),
        email: Set("grace@example.com".to_owned()),
        ..Default::default()
    }
    .insert_checked(&*db)
    .await
    .expect("insert");

    let mut tracked = Tracked::<user::ActiveModel>::load(row);
    assert!(!tracked.is_changed());
    assert_eq!(tracked.changed(), Vec::<&str>::new());

    tracked.name = Set("Grace Hopper".to_owned());
    assert!(tracked.is_changed());
    assert_eq!(tracked.changed(), ["name"]);
    assert!(tracked.changed_field("name"));
    assert!(!tracked.changed_field("email"));
    assert_eq!(tracked.was("name"), Some("Grace".into()));

    let changes = tracked.changes();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].field, "name");
    assert_eq!(changes[0].from, Some("Grace".into()));
    assert_eq!(changes[0].to, "Grace Hopper".into());

    // Writing the value that is already there is not a change.
    tracked.email = Set("grace@example.com".to_owned());
    assert_eq!(tracked.changed(), ["name"]);

    let saved = tracked.save(&*db).await.expect("save");
    assert_eq!(saved.name, "Grace Hopper");
}

#[tokio::test]
async fn a_fresh_record_has_no_previous_values() {
    let tracked = Tracked::fresh(user::ActiveModel {
        name: Set("New".to_owned()),
        ..Default::default()
    });
    assert!(tracked.is_changed());
    assert_eq!(tracked.changed(), ["name"]);
    assert_eq!(tracked.was("name"), None);
    assert!(tracked.original().is_none());
}

// ---------------------------------------------------------------------------
// 6. N+1 detection
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_probe_flags_a_hand_rolled_n_plus_one() {
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let probe = Probe::new(ProbeConfig {
        on_detect: Some(Arc::new(move |r| {
            sink.lock().expect("lock").push(r.to_string());
        })),
        ..Default::default()
    });

    let harness = Harness::sqlite_with(probe.clone()).await.expect("connect");
    harness.migrate::<Migrator>().await.expect("migrate");
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
    assert!(reports[0].contains("GET /users (naive)"));
    assert!(reports[0].contains("N+1 suspected"));
    assert!(
        reports[0].contains("5x"),
        "five identical shapes: {}",
        reports[0]
    );
}

#[tokio::test]
async fn a_probe_that_is_off_can_be_switched_on_per_request() {
    let probe = Probe::off();
    let harness = Harness::sqlite_with(probe.clone()).await.expect("connect");
    harness.migrate::<Migrator>().await.expect("migrate");
    let db = harness.begin().await.expect("begin");

    let quiet = probe.unit_of_work("production request");
    user::Entity::find().all(&*db).await.expect("users");
    assert_eq!(quiet.finish().statements, 0);

    probe.set_enabled(true);
    let loud = probe.unit_of_work("traced request");
    user::Entity::find().all(&*db).await.expect("users");
    assert_eq!(loud.finish().statements, 1);
}

// ---------------------------------------------------------------------------
// 7. The entity registry
// ---------------------------------------------------------------------------

#[test]
fn the_registry_enumerates_every_model_in_the_binary() {
    let modules = registry::modules();
    assert!(
        modules.iter().any(|m| m.ends_with("blog::user")),
        "registry: {modules:?}"
    );
    assert!(modules.iter().any(|m| m.ends_with("blog::post")));
    assert_eq!(registry::modules_under("no_such_crate").len(), 0);
}

#[test]
fn the_registry_describes_columns_for_a_generator() {
    let registry = ModelRegistry::new()
        .with("user", user::Entity, DbBackend::Sqlite)
        .with("post", post::Entity, DbBackend::Sqlite);

    assert_eq!(registry.names(), ["user", "post"]);
    assert_eq!(registry.tables(), ["user", "post"]);

    let user = registry.get("user").expect("user");
    assert_eq!(
        user.columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["id", "name", "email"]
    );
    assert_eq!(user.primary_key().len(), 1);
    assert_eq!(user.primary_key()[0].name, "id");
    assert!(user.column("email").expect("email").unique);
    assert!(!user.column("email").expect("email").nullable);
    assert_eq!(user.column("name").expect("name").type_name, "string");

    let post = registry.get("post").expect("post");
    assert_eq!(post.column("views").expect("views").type_name, "integer");

    // The only model linked in but not declared by this application is SeaORM's
    // own migration bookkeeping table, which is what a generator should skip.
    assert_eq!(
        registry.unregistered(),
        ["sea_orm_migration::seaql_migrations"]
    );
}

// ---------------------------------------------------------------------------
// 8. The schema-sync seam
// ---------------------------------------------------------------------------

#[test]
fn the_schema_planner_proposes_and_never_applies() {
    use brocanth_model::schema_sync::{PlanOnly, SchemaPlanner};

    let plan = PlanOnly::new(DbBackend::Sqlite)
        .with(user::Entity)
        .with(post::Entity)
        .plan();

    assert_eq!(plan.additive.len(), 2);
    assert!(plan.additive[0].sql.contains("CREATE TABLE"));
    assert!(!plan.is_destructive());
    assert!(!plan.warnings.is_empty(), "it says it is a proposal");
}
