//! The worked example: a `User` with many `Post`s, in SeaORM 2.0's dense entity
//! format, where relations are fields on the model rather than a separate enum.
//!
//! Shared by the integration tests and by `examples/blog.rs` through
//! `#[path = "../fixtures/blog.rs"] mod blog;`. It is not part of the published
//! library.

#![allow(dead_code)]

pub mod user {
    use brocanth_model::prelude::*;

    #[brocanth_model::validates(
        name(presence, length(min = 2, max = 80)),
        email(
            presence,
            format(r"^[^@\s]+@[^@\s]+\.[^@\s]+$", message = "is not an email address"),
            uniqueness,
        )
    )]
    #[sea_orm::model]
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "user")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub name: String,
        #[sea_orm(unique)]
        pub email: String,
        #[sea_orm(has_many)]
        pub posts: HasMany<super::post::Entity>,
    }
}

pub mod post {
    use brocanth_model::prelude::*;

    #[brocanth_model::validates(
        title(presence, length(min = 3, max = 120)),
        slug(presence, format(r"^[a-z0-9-]+$"), uniqueness),
        status(inclusion("draft", "published")),
        views(numericality(min = 0, only_integer))
    )]
    #[sea_orm::model]
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "post")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub user_id: i32,
        pub title: String,
        #[sea_orm(unique)]
        pub slug: String,
        pub status: String,
        pub views: i32,
        #[sea_orm(belongs_to, from = "user_id", to = "id")]
        pub author: BelongsTo<super::user::Entity>,
    }
}

/// Hand-written migrations. The plan keeps them hand-written until
/// `schema-sync` stabilises, so this is what a generated file would look like
/// after a developer read it and committed it.
pub mod migration {
    use brocanth_model::sea_orm_migration::prelude::*;

    pub struct Migrator;

    #[async_trait::async_trait]
    impl MigratorTrait for Migrator {
        fn migrations() -> Vec<Box<dyn MigrationTrait>> {
            vec![Box::new(M0001CreateUsers), Box::new(M0002CreatePosts)]
        }
    }

    #[derive(DeriveIden)]
    enum User {
        Table,
        Id,
        Name,
        Email,
    }

    #[derive(DeriveIden)]
    enum Post {
        Table,
        Id,
        UserId,
        Title,
        Slug,
        Status,
        Views,
    }

    pub struct M0001CreateUsers;

    impl MigrationName for M0001CreateUsers {
        fn name(&self) -> &str {
            "m0001_create_users"
        }
    }

    #[async_trait::async_trait]
    impl MigrationTrait for M0001CreateUsers {
        async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .create_table(
                    Table::create()
                        .table(User::Table)
                        .if_not_exists()
                        .col(
                            ColumnDef::new(User::Id)
                                .integer()
                                .not_null()
                                .auto_increment()
                                .primary_key(),
                        )
                        .col(ColumnDef::new(User::Name).string().not_null())
                        .col(ColumnDef::new(User::Email).string().not_null().unique_key())
                        .to_owned(),
                )
                .await
        }

        async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .drop_table(Table::drop().table(User::Table).to_owned())
                .await
        }
    }

    pub struct M0002CreatePosts;

    impl MigrationName for M0002CreatePosts {
        fn name(&self) -> &str {
            "m0002_create_posts"
        }
    }

    #[async_trait::async_trait]
    impl MigrationTrait for M0002CreatePosts {
        async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .create_table(
                    Table::create()
                        .table(Post::Table)
                        .if_not_exists()
                        .col(
                            ColumnDef::new(Post::Id)
                                .integer()
                                .not_null()
                                .auto_increment()
                                .primary_key(),
                        )
                        .col(ColumnDef::new(Post::UserId).integer().not_null())
                        .col(ColumnDef::new(Post::Title).string().not_null())
                        .col(ColumnDef::new(Post::Slug).string().not_null().unique_key())
                        .col(ColumnDef::new(Post::Status).string().not_null())
                        .col(ColumnDef::new(Post::Views).integer().not_null().default(0))
                        .foreign_key(
                            ForeignKey::create()
                                .name("fk_post_user")
                                .from(Post::Table, Post::UserId)
                                .to(User::Table, User::Id)
                                .on_delete(ForeignKeyAction::Cascade),
                        )
                        .to_owned(),
                )
                .await
        }

        async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .drop_table(Table::drop().table(Post::Table).to_owned())
                .await
        }
    }
}

/// Seed helpers shared by the tests and the example.
pub mod seed {
    use super::{post, user};
    use brocanth_model::prelude::*;

    /// Insert `users` users, each with `posts_each` posts.
    pub async fn blog<C>(db: &C, users: usize, posts_each: usize) -> Result<(), Error>
    where
        C: ConnectionTrait,
    {
        for u in 1..=users {
            let author = user::ActiveModel {
                name: Set(format!("Author {u}")),
                email: Set(format!("author{u}@example.com")),
                ..Default::default()
            }
            .insert_checked(db)
            .await?;

            for p in 1..=posts_each {
                post::ActiveModel {
                    user_id: Set(author.id),
                    title: Set(format!("Post {p} by {u}")),
                    slug: Set(format!("post-{p}-by-{u}")),
                    status: Set("published".to_owned()),
                    views: Set(0),
                    ..Default::default()
                }
                .insert_checked(db)
                .await?;
            }
        }
        Ok(())
    }
}
