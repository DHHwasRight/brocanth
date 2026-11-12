//! The SeaORM migration `rls make` writes in a project with `model`
//! compiles against the model layer, and names itself after its file.

use brocanth_model::sea_orm_migration::prelude::*;

#[path = "golden/m20261111_000000_rls.rs"]
mod m20261111_000000_rls;

#[test]
fn the_seaorm_migration_compiles_and_names_itself() {
    let migration: Box<dyn MigrationTrait> = Box::new(m20261111_000000_rls::Migration);
    assert_eq!(migration.name(), "m20261111_000000_rls");
}

mod post {
    use brocanth_model::prelude::*;
    use brocanth_model::sea_orm;

    #[sea_orm::model]
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "posts")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i64,
        pub tenant_id: i64,
    }

    impl ActiveModelBehavior for ActiveModel {}
}

/// A table and a column named by the entity: a typo does not compile.
#[test]
fn an_entity_names_the_table_and_its_columns() {
    use brocanth_rls::{Policy, Schema, Setting, SqlExpr, SqlType, Table};

    const TENANT: Setting = Setting::of("tenant_id", SqlType::BigInt);
    let posts = Table::of(post::Entity).force();
    assert_eq!(posts.key(), "posts");
    let mine = SqlExpr::column(post::Column::TenantId).eq(TENANT.get());
    assert!(
        mine.to_sql().starts_with(r#""tenant_id" = "#),
        "{}",
        mine.to_sql()
    );
    let sql = Schema::new()
        .table(posts.clone())
        .policy(Policy::new("posts_tenant", &posts).using(mine))
        .create_sql()
        .unwrap();
    assert!(
        sql.contains(r#"CREATE POLICY "posts_tenant" ON "posts""#),
        "{sql}"
    );
    assert_eq!(
        SqlExpr::table_column(post::Entity, post::Column::Id).to_sql(),
        r#""posts"."id""#
    );
}
