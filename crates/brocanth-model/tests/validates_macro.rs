//! What `#[validates(..)]` generates, and what `no_hook` takes away.

use brocanth_model::prelude::*;
use brocanth_model::testing::Harness;
use sea_orm::{ConnectionTrait, PaginatorTrait, Schema};

/// `no_hook` plus a hand-written `ActiveModelBehavior` that calls `enforce`.
/// This is the escape hatch for a model that needs a hook of its own — a
/// timestamp, a counter cache — without losing validation.
mod guarded {
    use brocanth_model::prelude::*;

    #[brocanth_model::validates(
        no_hook,
        label(presence, length(max = 5)),
        code(uniqueness(column = "Code", message = "is already in use"))
    )]
    #[sea_orm::model]
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "guarded")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub label: String,
        #[sea_orm(unique)]
        pub code: String,
        pub touched: i32,
    }

    #[brocanth_model::async_trait]
    impl ActiveModelBehavior for ActiveModel {
        async fn before_save<C>(mut self, db: &C, _insert: bool) -> Result<Self, DbErr>
        where
            C: ConnectionTrait,
        {
            // The model's own hook work, then the validation bridge. Forgetting
            // the second line is the whole risk `no_hook` carries, which is why
            // it is not the default.
            self.touched = Set(1);
            brocanth_model::enforce(self, db).await
        }
    }
}

/// `no_hook` with an empty behavior: validation is declared and never enforced.
mod unguarded {
    use brocanth_model::prelude::*;

    #[brocanth_model::validates(no_hook, label(presence))]
    #[sea_orm::model]
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "unguarded")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub label: String,
    }

    impl ActiveModelBehavior for ActiveModel {}
}

async fn table<E: sea_orm::EntityTrait>(db: &impl ConnectionTrait, entity: E) {
    let schema = Schema::new(db.get_database_backend());
    db.execute(&schema.create_table_from_entity(entity))
        .await
        .expect("create table");
}

#[tokio::test]
async fn a_hand_written_hook_can_still_enforce_validation() {
    let harness = Harness::sqlite().await.expect("connect");
    table(harness.conn(), guarded::Entity).await;
    let db = harness.begin().await.expect("begin");

    let err = guarded::ActiveModel {
        label: Set("far too long".to_owned()),
        code: Set("a".to_owned()),
        touched: Set(0),
        ..Default::default()
    }
    .insert_checked(&*db)
    .await
    .expect_err("label is too long");
    assert!(err.errors().expect("errors").has_code("label", "too_long"));

    let saved = guarded::ActiveModel {
        label: Set("ok".to_owned()),
        code: Set("a".to_owned()),
        touched: Set(0),
        ..Default::default()
    }
    .insert_checked(&*db)
    .await
    .expect("insert");
    assert_eq!(saved.touched, 1, "the model's own hook work also ran");

    let clash = guarded::ActiveModel {
        label: Set("ok".to_owned()),
        code: Set("a".to_owned()),
        touched: Set(0),
        ..Default::default()
    }
    .insert_checked(&*db)
    .await
    .expect_err("duplicate code");
    assert_eq!(
        clash.errors().expect("errors").on("code"),
        ["is already in use"],
        "the per-rule message override reached the form"
    );
}

#[tokio::test]
async fn no_hook_really_removes_the_enforcement_point() {
    let harness = Harness::sqlite().await.expect("connect");
    table(harness.conn(), unguarded::Entity).await;
    let db = harness.begin().await.expect("begin");

    let active = unguarded::ActiveModel {
        label: Set("".to_owned()),
        ..Default::default()
    };

    // The rules are still declared and still answer.
    let errors = active.errors(&*db).await.expect("validate");
    assert!(errors.has_code("label", "blank"));
    assert!(!active.clone().valid(&*db).await.expect("valid"));

    // ...and nothing stops the write, because the hook was not generated.
    active.insert_checked(&*db).await.expect("writes anyway");
    assert_eq!(
        unguarded::Entity::find().count(&*db).await.expect("count"),
        1
    );
}
