// Written by `brocanth migrate make`. Read it before you commit it.
//
// It is a proposal: edit it when it is wrong. migrations/schema.json
// records the models, not this file.
//
// - rename table `user` to `account`
// - rename column `post.title` to `headline`
// - add column `post.body` (string)
// - drop column `post.views`

use brocanth_model::sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m0002_renames"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // rename table `user` to `account`
        manager
            .rename_table(
                Table::rename()
                    .table(Alias::new("user"), Alias::new("account"))
                    .to_owned(),
            )
            .await?;
        // rename column `post.title` to `headline`
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("post"))
                    .rename_column(Alias::new("title"), Alias::new("headline"))
                    .to_owned(),
            )
            .await?;
        // add column `post.body` (string)
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("post"))
                    .add_column(ColumnDef::new(Alias::new("body")).string().null())
                    .to_owned(),
            )
            .await?;
        // drop column `post.views`
        // Check: this deletes data. The inverse step does not restore the data.
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("post"))
                    .drop_column(Alias::new("views"))
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // add column `post.views` (integer)
        // Check: NOT NULL with no default fails on a table with rows. Add a default.
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("post"))
                    .add_column(ColumnDef::new(Alias::new("views")).integer().not_null())
                    .to_owned(),
            )
            .await?;
        // drop column `post.body`
        // Check: this deletes data. The inverse step does not restore the data.
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("post"))
                    .drop_column(Alias::new("body"))
                    .to_owned(),
            )
            .await?;
        // rename column `post.headline` to `title`
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("post"))
                    .rename_column(Alias::new("headline"), Alias::new("title"))
                    .to_owned(),
            )
            .await?;
        // rename table `account` to `user`
        manager
            .rename_table(
                Table::rename()
                    .table(Alias::new("account"), Alias::new("user"))
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}
