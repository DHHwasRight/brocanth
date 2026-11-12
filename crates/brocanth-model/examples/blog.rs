//! The worked example, end to end, printing what it proves.
//!
//! It needs a Postgres server. It reads the URL from
//! `BROCANTH_MODEL_TEST_POSTGRES`, `BROCANTH_TEST_DATABASE_URL` or
//! `DATABASE_URL`, in that order, and works in a schema of its own that it
//! drops at the end:
//!
//! ```sh
//! DATABASE_URL=postgres://localhost/brocanth_example \
//!   cargo run -p brocanth-model --example blog
//! ```
//!
//! With no URL it says so and exits with success, so a build that runs every
//! example does not fail on a machine with no server.

#[path = "../fixtures/blog.rs"]
mod blog;

use blog::{migration::Migrator, post, user};
use brocanth_model::bulk::{self, HooksBypassed};
use brocanth_model::prelude::*;
use brocanth_model::registry::{self, ModelRegistry};
use brocanth_model::schema_sync::{PlanOnly, SchemaPlanner};
use brocanth_model::testing::Harness;
use brocanth_model::{Probe, ProbeConfig, Tracked};
use sea_orm::sea_query::IntoCondition;
use sea_orm::{ConnectionTrait, PaginatorTrait, QuerySelect, Statement};
use std::sync::{Arc, Mutex};

fn heading(n: u8, text: &str) {
    println!("\n{n}. {text}\n{}", "-".repeat(60));
}

/// The tables of the current schema, which is the example's own.
async fn tables(db: &impl ConnectionTrait) -> Vec<String> {
    let rows = db
        .query_all_raw(Statement::from_string(
            db.get_database_backend(),
            "SELECT table_name::text AS name FROM information_schema.tables \
             WHERE table_schema = current_schema() AND table_type = 'BASE TABLE' \
             ORDER BY table_name",
        ))
        .await
        .expect("information_schema.tables");
    rows.iter()
        .map(|r| r.try_get::<String>("", "name").expect("name"))
        .filter(|n| n != "seaql_migrations")
        .collect()
}

/// The server's URL, from the first variable that names one.
fn database_url() -> Option<String> {
    Harness::postgres_url().or_else(|| {
        std::env::var("DATABASE_URL")
            .ok()
            .filter(|url| !url.trim().is_empty())
    })
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Some(url) = database_url() else {
        println!(
            "No Postgres server, so nothing ran.\n\
             help: set DATABASE_URL (or BROCANTH_TEST_DATABASE_URL) to a database \
             this example may write to, then run it again."
        );
        return Ok(());
    };

    let flagged: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&flagged);
    let probe = Probe::new(ProbeConfig {
        on_detect: Some(Arc::new(move |report| {
            sink.lock().expect("lock").push(report.to_string());
        })),
        ..Default::default()
    });

    let harness = Harness::postgres_isolated_with(&url, probe.clone()).await?;

    // -----------------------------------------------------------------------
    heading(1, "A migration, up and down");
    harness.migrate::<Migrator>().await?;
    println!("   up     -> {:?}", tables(harness.conn()).await);
    harness.rollback::<Migrator>(None).await?;
    println!("   down   -> {:?}", tables(harness.conn()).await);
    harness.migrate::<Migrator>().await?;
    println!("   up     -> {:?}", tables(harness.conn()).await);

    // Everything below runs in the harness transaction, which never commits.
    let db = harness.begin().await?;

    // -----------------------------------------------------------------------
    heading(2, "A validation rejecting a write before any SQL is issued");
    let unit = probe.unit_of_work("rejected insert");
    let rejected = user::ActiveModel {
        name: Set("A".to_owned()),
        ..Default::default()
    }
    .insert_checked(&*db)
    .await;
    let report = unit.finish();

    match rejected {
        Err(e) if e.is_invalid() => {
            let errors = e.errors().expect("field errors");
            for field in errors.fields() {
                println!("   {field:<8} {:?}", errors.on(field));
            }
            println!("   flash    {}", errors.full_messages().join("; "));
        }
        other => panic!("expected a validation failure, got {other:?}"),
    }
    println!("   statements issued: {}", report.statements);

    println!("\n   The same rule holds on a raw SeaORM insert, because the");
    println!("   before_save hook is the enforcement point:");
    let raw = user::ActiveModel {
        name: Set("".to_owned()),
        email: Set("x@example.com".to_owned()),
        ..Default::default()
    }
    .insert(&*db)
    .await
    .expect_err("rejected");
    println!("   recovered: {}", Error::from(raw));

    println!("\n   Uniqueness queries, which is why hooks taking a connection");
    println!("   is the whole reason SeaORM 2.0 was the only candidate:");
    blog::seed::blog(&*db, 10, 3).await?;
    let clash = user::ActiveModel {
        name: Set("Impostor".to_owned()),
        email: Set("author1@example.com".to_owned()),
        ..Default::default()
    }
    .insert_checked(&*db)
    .await
    .expect_err("duplicate");
    println!("   {}", clash);

    // -----------------------------------------------------------------------
    heading(3, "A nested transaction producing a real SAVEPOINT");
    let before = user::Entity::find().count(&*db).await?;
    let inner = db.begin().await?;
    user::ActiveModel {
        name: Set("Discarded".to_owned()),
        email: Set("discarded@example.com".to_owned()),
        ..Default::default()
    }
    .insert_checked(&inner)
    .await?;
    println!(
        "   inside savepoint:        {} users",
        user::Entity::find().count(&inner).await?
    );
    inner.rollback().await?;
    println!(
        "   after savepoint rollback: {} users (was {before})",
        user::Entity::find().count(&*db).await?
    );

    // -----------------------------------------------------------------------
    heading(4, "An eager load that is one query");
    let query = user::Entity::find().eager();
    println!("   strategy: {:?}", query.strategy());
    let unit = probe.unit_of_work("unbounded eager load");
    let rows = query.with_related::<post::Entity, _>(&*db).await?;
    let report = unit.finish();
    println!(
        "   {} users, {} posts, {} statement",
        rows.len(),
        rows.iter().map(|r| r.related().len()).sum::<usize>(),
        report.statements
    );

    // -----------------------------------------------------------------------
    heading(
        5,
        "A limited eager load returning the right number of parents",
    );
    let naive = user::Entity::find()
        .limit(10)
        .find_with_related(post::Entity)
        .all(&*db)
        .await?;
    println!(
        "   SeaORM  .limit(10).find_with_related() -> {} users  <- wrong",
        naive.len()
    );

    let query = user::Entity::find().eager().limit(10);
    println!("   strategy: {:?}", query.strategy());
    let unit = probe.unit_of_work("limited eager load");
    let fixed = query.with_related::<post::Entity, _>(&*db).await?;
    let report = unit.finish();
    println!(
        "   brocanth .eager().limit(10)            -> {} users, {} posts, {} statements",
        fixed.len(),
        fixed.iter().map(|r| r.related().len()).sum::<usize>(),
        report.statements
    );

    println!("\n   A relation nobody loaded is loud rather than quiet:");
    let unloaded = user::Entity::find()
        .eager()
        .limit(1)
        .without_related::<post::Entity, _>(&*db)
        .await?;
    println!("   {}", unloaded[0].try_related().expect_err("refused"));

    // -----------------------------------------------------------------------
    heading(6, "N+1 detection");
    {
        let _unit = probe.unit_of_work("GET /users (hand-rolled N+1)");
        for u in user::Entity::find().limit(5).all(&*db).await? {
            let _ = post::Entity::find()
                .filter(post::Column::UserId.eq(u.id))
                .all(&*db)
                .await?;
        }
    }
    for line in flagged.lock().expect("lock").iter() {
        for l in line.lines() {
            println!("   {l}");
        }
    }

    // -----------------------------------------------------------------------
    heading(7, "Hook coverage for set-based writes");
    let refused = bulk::update_where::<post::Entity, _, _>(
        post::Column::Status.eq("published").into_condition(),
        |p| p.status = Set("archived".to_owned()),
        &*db,
    )
    .await
    .expect_err("archived is not allowed");
    println!("   bulk::update_where  -> {refused}");
    println!(
        "   rows now archived:     {}",
        post::Entity::find()
            .filter(post::Column::Status.eq("archived"))
            .count(&*db)
            .await?
    );

    bulk::unchecked::update_many::<post::Entity>(HooksBypassed::because(
        "example: showing what the bypass costs",
    ))
    .col_expr(
        post::Column::Status,
        sea_orm::sea_query::Expr::value("archived"),
    )
    .exec(&*db)
    .await?;
    println!("   unchecked::update_many -> skipped the hook");
    println!(
        "   rows now archived:     {}",
        post::Entity::find()
            .filter(post::Column::Status.eq("archived"))
            .count(&*db)
            .await?
    );

    // -----------------------------------------------------------------------
    heading(8, "Dirty tracking");
    let row = user::Entity::find().one(&*db).await?.expect("a user");
    let mut tracked = Tracked::<user::ActiveModel>::load(row);
    println!("   changed() before any edit: {:?}", tracked.changed());
    tracked.name = Set("Ada Lovelace".to_owned());
    tracked.email = Set("author1@example.com".to_owned()); // same value
    println!("   changed()                : {:?}", tracked.changed());
    println!("   was(\"name\")              : {:?}", tracked.was("name"));
    for change in tracked.changes() {
        println!(
            "   changes()                : {} {:?} -> {:?}",
            change.field, change.from, change.to
        );
    }

    // -----------------------------------------------------------------------
    heading(9, "The entity registry enumerating the models");
    for module in registry::modules() {
        println!("   {module}");
    }
    let declared = ModelRegistry::new()
        .with("user", user::Entity, DbBackend::Postgres)
        .with("post", post::Entity, DbBackend::Postgres);
    for (name, info) in declared.models() {
        println!("\n   {name} -> table {:?}", info.table);
        for column in &info.columns {
            println!(
                "     {:<8} {:<9} {}{}{}",
                column.name,
                column.type_name,
                if column.primary_key { "pk " } else { "" },
                if column.unique { "unique " } else { "" },
                if column.nullable { "null" } else { "not null" },
            );
        }
    }
    println!(
        "\n   linked but not declared: {:?}",
        declared.unregistered()
    );

    // -----------------------------------------------------------------------
    heading(10, "The schema-sync seam");
    let plan = PlanOnly::new(DbBackend::Postgres)
        .with(user::Entity)
        .with(post::Entity)
        .plan();
    println!(
        "   additive: {}  destructive: {}",
        plan.additive.len(),
        plan.destructive.len()
    );
    for warning in &plan.warnings {
        println!("   warning: {warning}");
    }

    // -----------------------------------------------------------------------
    drop(db);
    println!(
        "\nThe harness transaction cannot commit, so the database is empty again: {} users\n",
        user::Entity::find().count(harness.conn()).await?
    );
    Ok(())
}
