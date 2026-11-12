//! The bridge, against a real database with real entities.
//!
//! The claim worth testing is not that a read comes back. It is that a
//! write goes through an `ActiveModel`, so the application's validations
//! run — a data source that built SQL by hand would pass every other test
//! in this file and silently skip them.
//!
//! The entities are defined here rather than borrowed from
//! `brocanth-model`'s blog fixture, and the reason is the one constraint
//! this approach carries: `set_from_json` needs the entity's `Model` to be
//! `Serialize` and `Deserialize`, and `DeriveEntityModel` does not add
//! them. An entity that is not serde-derivable cannot be in the admin.
//!
//! That is worth stating in a test that fails to compile without it rather
//! than burying in a doc comment, so these carry the derives and the real
//! `#[validates]` rules side by side.
//!
//! Each test gets its own Postgres schema through `Harness::isolated`, and
//! skips when no server is set (`BROCANTH_TEST_DATABASE_URL` or
//! `BROCANTH_MODEL_TEST_POSTGRES`).

use std::sync::Arc;

use brocanth_admin::data::{AdminData, DataError, ListRequest, Scope, Search};
use brocanth_admin::model::{ColumnDescriptor, ColumnKind, ModelDescriptor, PrimaryKey, Value};
use brocanth_admin::policy::AllowAllUnsafeForDevelopment;
use brocanth_admin::{Admin, AdminConfig, Discovery, Principal, Submission};
use brocanth_admin_model::admin_source;
use brocanth_model::sea_orm::ConnectionTrait;
use brocanth_model::testing::{skipped, Harness};

mod blog {
    pub mod user {
        use brocanth_model::prelude::*;
        use serde::{Deserialize, Serialize};

        #[brocanth_model::validates(
            name(presence, length(min = 2, max = 80)),
            email(
                presence,
                format(r"^[^@\s]+@[^@\s]+\.[^@\s]+$", message = "is not an email address"),
            )
        )]
        #[sea_orm::model]
        // `Serialize` and `Deserialize` are not optional for an entity the
        // admin shows: they are what `set_from_json` is written in terms
        // of. Without them this file does not compile, which is where that
        // requirement belongs.
        #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
        #[sea_orm(table_name = "user")]
        pub struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub name: String,
            pub email: String,
        }
    }

    pub mod post {
        use brocanth_model::prelude::*;
        use serde::{Deserialize, Serialize};

        #[brocanth_model::validates(title(presence, length(min = 3, max = 120)))]
        #[sea_orm::model]
        #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
        #[sea_orm(table_name = "post")]
        pub struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub title: String,
        }
    }
}

admin_source! {
    /// Two entities, one line each.
    pub struct Models;
    "blog::user" => blog::user::Entity,
    "blog::post" => blog::post::Entity,
}

/// The two tables, by hand. A migration crate would be a third dependency
/// to prove something this file is not about. `user` is a reserved word on
/// Postgres, so it is quoted here; sea-query quotes it in every query.
const SCHEMA: [&str; 2] = [
    r#"CREATE TABLE "user" (id SERIAL PRIMARY KEY, name TEXT NOT NULL, email TEXT NOT NULL)"#,
    "CREATE TABLE post (id SERIAL PRIMARY KEY, title TEXT NOT NULL)",
];

/// The descriptor the admin would build, written by hand so this file tests
/// the data source and not the discovery.
fn user_model() -> ModelDescriptor {
    ModelDescriptor {
        key: "user".into(),
        module_path: "blog::user".into(),
        table: "user".into(),
        columns: vec![
            ColumnDescriptor::new("id", ColumnKind::Integer).primary_key(),
            ColumnDescriptor::new("name", ColumnKind::Text),
            ColumnDescriptor::new("email", ColumnKind::Text),
        ],
    }
}

/// A database of its own, or `None` (and a skip line) when no server is set.
async fn isolated() -> Option<Harness> {
    let harness = Harness::isolated().await.expect("connect");
    if harness.is_none() {
        skipped("no server: set BROCANTH_TEST_DATABASE_URL");
    }
    harness
}

async fn source() -> Option<(Models, Harness)> {
    let harness = isolated().await?;
    for statement in SCHEMA {
        harness
            .conn()
            .execute_unprepared(statement)
            .await
            .expect("schema");
    }
    let models = Models::new(harness.conn().clone());
    Some((models, harness))
}

fn no_search() -> Search<'static> {
    Search {
        term: "",
        columns: &[],
    }
}

#[tokio::test]
async fn the_registry_reports_what_the_macro_was_given() {
    let Some((models, _h)) = source().await else {
        return;
    };
    let entries: Vec<(String, String)> = models
        .registry()
        .into_iter()
        .map(|e| (e.module_path, e.table))
        .collect();
    assert_eq!(
        entries,
        [
            ("blog::user".to_string(), "user".to_string()),
            ("blog::post".to_string(), "post".to_string())
        ],
        "and the table name came off the entity, not off the string"
    );
}

#[tokio::test]
async fn a_write_goes_through_the_active_model_so_validations_run() {
    // The test this crate exists for. `email` carries `format(...)` on the
    // fixture's `user`, and the only way a bad address is refused here is
    // if the insert went through an `ActiveModel` and SeaORM called the
    // hook. A source that built SQL by hand would write the row.
    let Some((models, _h)) = source().await else {
        return;
    };
    let model = user_model();

    let bad = models
        .insert(
            &model,
            &[
                ("name".into(), Value::Text("Ada Lovelace".into())),
                ("email".into(), Value::Text("not-an-address".into())),
            ],
        )
        .await;
    assert!(bad.is_err(), "a bad email must not be written: {bad:?}");

    assert_eq!(
        models
            .count(&model, no_search(), Scope::Unscoped)
            .await
            .expect("count"),
        0,
        "and nothing was written"
    );
}

#[tokio::test]
async fn the_six_operations_round_trip() {
    let Some((models, _h)) = source().await else {
        return;
    };
    let model = user_model();

    let key = models
        .insert(
            &model,
            &[
                ("name".into(), Value::Text("Ada Lovelace".into())),
                ("email".into(), Value::Text("ada@example.com".into())),
            ],
        )
        .await
        .expect("insert");

    let row = models
        .fetch(&model, &key, Scope::Unscoped)
        .await
        .expect("fetch")
        .expect("a row");
    assert_eq!(
        row.get("name").map(|v| v.as_input()).as_deref(),
        Some("Ada Lovelace")
    );

    models
        .update(
            &model,
            &key,
            &[("name".into(), Value::Text("Augusta Ada King".into()))],
            Scope::Unscoped,
        )
        .await
        .expect("update");
    let row = models
        .fetch(&model, &key, Scope::Unscoped)
        .await
        .expect("fetch")
        .expect("a row");
    assert_eq!(
        row.get("name").map(|v| v.as_input()).as_deref(),
        Some("Augusta Ada King")
    );
    assert_eq!(
        row.get("email").map(|v| v.as_input()).as_deref(),
        Some("ada@example.com"),
        "a column the form did not send keeps its value"
    );

    models
        .delete(&model, &key, Scope::Unscoped)
        .await
        .expect("delete");
    assert!(models
        .fetch(&model, &key, Scope::Unscoped)
        .await
        .expect("fetch")
        .is_none());
}

#[tokio::test]
async fn search_is_case_insensitive_and_a_sort_name_never_reaches_sql() {
    let Some((models, _h)) = source().await else {
        return;
    };
    let model = user_model();
    for (name, email) in [
        ("Ada Lovelace", "ada@x.com"),
        ("Grace Hopper", "grace@x.com"),
    ] {
        models
            .insert(
                &model,
                &[
                    ("name".into(), Value::Text(name.into())),
                    ("email".into(), Value::Text(email.into())),
                ],
            )
            .await
            .expect("insert");
    }

    // Lower case against a capitalised row. A bare `LIKE` finds nothing
    // here on Postgres, which is the bug the `LOWER(CAST(...))` in
    // `search_of` exists to avoid.
    let columns = vec!["name".to_string()];
    let rows = models
        .list(ListRequest {
            model: &model,
            offset: 0,
            limit: 10,
            search: Search {
                term: "lovelace",
                columns: &columns,
            },
            sort: None,
            filter: None,
            scope: Scope::Unscoped,
        })
        .await
        .expect("list");
    assert_eq!(rows.len(), 1, "found by lower case: {rows:?}");

    // A sort column that is not a column is refused rather than built.
    let sort = brocanth_admin::query::Sort {
        column: "name; DROP TABLE user".into(),
        direction: brocanth_admin::query::Direction::Ascending,
    };
    let refused = models
        .list(ListRequest {
            model: &model,
            offset: 0,
            limit: 10,
            search: no_search(),
            sort: Some(&sort),
            filter: None,
            scope: Scope::Unscoped,
        })
        .await;
    assert!(
        matches!(refused, Err(DataError::Backend(_))),
        "a name that is not a column never becomes one: {refused:?}"
    );
    assert_eq!(
        models
            .count(&model, no_search(), Scope::Unscoped)
            .await
            .expect("count"),
        2,
        "and the table is still there"
    );
}

#[tokio::test]
async fn the_admin_renders_real_rows_through_it() {
    let Some((models, _h)) = source().await else {
        return;
    };
    let model = user_model();
    models
        .insert(
            &model,
            &[
                ("name".into(), Value::Text("Ada Lovelace".into())),
                ("email".into(), Value::Text("ada@example.com".into())),
            ],
        )
        .await
        .expect("insert");

    let admin = Admin::new(
        Arc::new(models),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog"))
            .unscoped_rows_unsafe_for_development()
            .with_policy(AllowAllUnsafeForDevelopment),
    );
    struct Anyone;
    let who = Anyone;
    // Awaited, not `rt::block_on`. That helper builds its own runtime for a
    // caller that has none, and calling it from inside `#[tokio::test]`
    // parks the thread the runtime it is waiting on would have run.
    let response = admin
        .dispatch(
            Principal::new(&who),
            "GET",
            "/admin/user",
            "",
            &Submission::new(),
        )
        .await
        .expect("dispatch");
    assert_eq!(response.status(), 200);
    let html = response.html().expect("a page");
    assert!(html.contains("Ada Lovelace"), "{html}");
}

#[tokio::test]
async fn a_module_path_the_macro_was_not_given_is_named() {
    let Some((models, _h)) = source().await else {
        return;
    };
    let mut model = user_model();
    model.module_path = "blog::comment".into();
    let out = models
        .fetch(&model, &PrimaryKey::single("id", 1), Scope::Unscoped)
        .await;
    let named = matches!(&out, Err(DataError::UnknownModel(m)) if m == "blog::comment");
    assert!(named, "{out:?}");
}

admin_source! {
    /// No entities at all, which a new project has and a panels-only
    /// backoffice always has.
    pub struct Nothing;
}

#[tokio::test]
async fn a_source_with_no_entities_is_a_source() {
    let Some(harness) = isolated().await else {
        return;
    };
    let models = Nothing::new(harness.conn().clone());
    assert!(models.registry().is_empty());

    // And every operation names the model it was not given, rather than
    // panicking on a match with no arms.
    let mut model = user_model();
    model.module_path = "anything".into();
    let out = models.count(&model, no_search(), Scope::Unscoped).await;
    assert!(matches!(out, Err(DataError::UnknownModel(_))), "{out:?}");
}

mod keyed {
    pub mod token {
        use brocanth_model::prelude::*;
        use serde::{Deserialize, Serialize};

        #[sea_orm::model]
        #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
        #[sea_orm(table_name = "token")]
        pub struct Model {
            #[sea_orm(primary_key, auto_increment = false)]
            pub id: sea_orm::prelude::Uuid,
            pub name: String,
        }

        impl ActiveModelBehavior for ActiveModel {}
    }

    pub mod note {
        use brocanth_model::prelude::*;
        use serde::{Deserialize, Serialize};

        #[sea_orm::model]
        #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
        #[sea_orm(table_name = "note")]
        pub struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub body: String,
            #[sea_orm(column_type = "JsonBinary")]
            pub data: sea_orm::prelude::Json,
        }

        impl ActiveModelBehavior for ActiveModel {}
    }
}

admin_source! {
    /// A UUID key, and a text column beside a JSON one.
    pub struct Keyed;
    "keyed::token" => keyed::token::Entity,
    "keyed::note" => keyed::note::Entity,
}

const TOKEN: &str = "6f1c1f9e-2b1a-4c3d-9e8f-0a1b2c3d4e5f";

async fn keyed() -> Option<(Keyed, Harness)> {
    let harness = isolated().await?;
    for statement in [
        "CREATE TABLE token (id uuid PRIMARY KEY, name TEXT NOT NULL)",
        "CREATE TABLE note (id SERIAL PRIMARY KEY, body TEXT NOT NULL, data JSONB NOT NULL)",
    ] {
        harness
            .conn()
            .execute_unprepared(statement)
            .await
            .expect("schema");
    }
    let models = Keyed::new(harness.conn().clone());
    Some((models, harness))
}

fn token_model() -> ModelDescriptor {
    ModelDescriptor {
        key: "token".into(),
        module_path: "keyed::token".into(),
        table: "token".into(),
        columns: vec![
            ColumnDescriptor::new("id", ColumnKind::Uuid).primary_key(),
            ColumnDescriptor::new("name", ColumnKind::Text),
        ],
    }
}

fn note_model() -> ModelDescriptor {
    ModelDescriptor {
        key: "note".into(),
        module_path: "keyed::note".into(),
        table: "note".into(),
        columns: vec![
            ColumnDescriptor::new("id", ColumnKind::Integer).primary_key(),
            ColumnDescriptor::new("body", ColumnKind::LongText),
            ColumnDescriptor::new("data", ColumnKind::Json),
        ],
    }
}

#[tokio::test]
async fn a_uuid_key_reads_writes_and_deletes_and_a_bad_one_is_no_row() {
    let Some((models, h)) = keyed().await else {
        return;
    };
    h.conn()
        .execute_unprepared(&format!(
            "INSERT INTO token (id, name) VALUES ('{TOKEN}', 'deploy')"
        ))
        .await
        .expect("seed");
    let model = token_model();
    let key = PrimaryKey::single("id", TOKEN);
    let row = models
        .fetch(&model, &key, Scope::Unscoped)
        .await
        .expect("a uuid key binds as a uuid")
        .expect("a row");
    assert_eq!(
        row.get("name").map(|v| v.as_input()).as_deref(),
        Some("deploy")
    );

    let changed = models
        .update(
            &model,
            &key,
            &[("name".into(), Value::Text("release".into()))],
            Scope::Unscoped,
        )
        .await
        .expect("update");
    assert_eq!(changed, 1);

    let bad = models
        .fetch(
            &model,
            &PrimaryKey::single("id", "not-a-uuid"),
            Scope::Unscoped,
        )
        .await;
    assert!(
        matches!(bad, Ok(None)),
        "a malformed uuid is no row: {bad:?}"
    );

    assert_eq!(
        models
            .delete(&model, &key, Scope::Unscoped)
            .await
            .expect("delete"),
        1
    );
}

#[tokio::test]
async fn a_key_an_integer_column_cannot_hold_is_no_row_and_not_an_error() {
    let Some((models, _h)) = source().await else {
        return;
    };
    let abc = PrimaryKey::single("id", "abc");
    let out = models.fetch(&user_model(), &abc, Scope::Unscoped).await;
    assert!(matches!(out, Ok(None)), "{out:?}");
    let out = models.delete(&user_model(), &abc, Scope::Unscoped).await;
    assert!(matches!(out, Ok(0)), "{out:?}");
}

#[tokio::test]
async fn a_text_column_that_looks_like_json_stays_text_and_a_json_column_stays_json() {
    let Some((models, _h)) = keyed().await else {
        return;
    };
    let model = note_model();
    let key = models
        .insert(
            &model,
            &[
                ("body".into(), Value::Text("[1]".into())),
                ("data".into(), Value::Text(r#"{"a":1}"#.into())),
            ],
        )
        .await
        .expect("a text column holding `[1]` saves");
    models
        .update(
            &model,
            &key,
            &[("body".into(), Value::Text("{}".into()))],
            Scope::Unscoped,
        )
        .await
        .expect("and `{}` too");
    let row = models
        .fetch(&model, &key, Scope::Unscoped)
        .await
        .expect("fetch")
        .expect("a row");
    assert_eq!(row.get("body").map(|v| v.as_input()).as_deref(), Some("{}"));
    assert_eq!(
        row.get("data").map(|v| v.as_input()).as_deref(),
        Some(r#"{"a":1}"#)
    );
}

#[tokio::test]
async fn a_validation_failure_is_invalid_with_the_field_named() {
    let Some((models, _h)) = source().await else {
        return;
    };
    let bad = models
        .insert(
            &user_model(),
            &[
                ("name".into(), Value::Text("Ada Lovelace".into())),
                ("email".into(), Value::Text("not-an-address".into())),
            ],
        )
        .await;
    let Err(DataError::Invalid(errors)) = bad else {
        panic!("a model validation is a form error: {bad:?}");
    };
    assert!(errors.on("email").next().is_some(), "{errors:?}");
}

async fn three_users(models: &Models) {
    for name in ["Ada Lovelace", "Grace Hopper", "Alan Turing"] {
        models
            .insert(
                &user_model(),
                &[
                    ("name".into(), Value::Text(name.into())),
                    ("email".into(), Value::Text("x@example.com".into())),
                ],
            )
            .await
            .expect("insert");
    }
}

#[tokio::test]
async fn a_page_with_no_sort_is_in_key_order() {
    let Some((models, h)) = source().await else {
        return;
    };
    let model = user_model();
    three_users(&models).await;
    // An update writes a new tuple at the end of the heap, so a scan with
    // no ORDER BY returns the first row last.
    h.conn()
        .execute_unprepared(r#"UPDATE "user" SET name = 'Ada King' WHERE id = 1"#)
        .await
        .expect("update");
    let rows = models
        .list(ListRequest {
            model: &model,
            offset: 0,
            limit: 10,
            search: no_search(),
            sort: None,
            filter: None,
            scope: Scope::Unscoped,
        })
        .await
        .expect("list");
    let ids: Vec<String> = rows
        .iter()
        .filter_map(|r| r.get("id").map(|v| v.as_input()))
        .collect();
    assert_eq!(ids, ["1", "2", "3"]);
}

#[tokio::test]
async fn a_percent_in_the_search_term_matches_itself() {
    let Some((models, _h)) = source().await else {
        return;
    };
    let model = user_model();
    for name in ["100% sure", "1000 sure", "a_b", "axb"] {
        models
            .insert(
                &model,
                &[
                    ("name".into(), Value::Text(name.into())),
                    ("email".into(), Value::Text("x@example.com".into())),
                ],
            )
            .await
            .expect("insert");
    }
    let columns = vec!["name".to_string()];
    for (term, expected) in [("100%", 1), ("a_b", 1), ("sure", 2)] {
        let found = models
            .count(
                &model,
                Search {
                    term,
                    columns: &columns,
                },
                Scope::Unscoped,
            )
            .await
            .expect("count");
        assert_eq!(found, expected, "term {term:?}");
    }
}

#[tokio::test]
async fn fetch_many_reads_a_batch_and_skips_a_key_the_column_cannot_hold() {
    let Some((models, _h)) = source().await else {
        return;
    };
    three_users(&models).await;
    let keys = ["1".to_string(), "3".to_string(), "abc".to_string()];
    let rows = models
        .fetch_many(&user_model(), "id", &keys, Scope::Unscoped)
        .await
        .expect("one query");
    let mut names: Vec<String> = rows
        .iter()
        .filter_map(|r| r.get("name").map(|v| v.as_input()))
        .collect();
    names.sort();
    assert_eq!(names, ["Ada Lovelace", "Alan Turing"]);
}
