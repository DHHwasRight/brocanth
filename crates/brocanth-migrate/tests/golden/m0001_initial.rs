// Written by `brocanth migrate make`. Read it before you commit it.
//
// It is a proposal: edit it when it is wrong. migrations/schema.json
// records the models, not this file.
//
// - create table `post`
// - create table `user`

use brocanth_model::sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m0001_initial"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // create table `post`
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("post"))
                    .col(
                        ColumnDef::new(Alias::new("id"))
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Alias::new("user_id")).integer().not_null())
                    .col(ColumnDef::new(Alias::new("title")).string().not_null())
                    .col(ColumnDef::new(Alias::new("views")).integer().not_null())
                    .to_owned(),
            )
            .await?;
        // create table `user`
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("user"))
                    .col(
                        ColumnDef::new(Alias::new("id"))
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Alias::new("name")).string().not_null())
                    .col(
                        ColumnDef::new(Alias::new("email"))
                            .string()
                            .not_null()
                            .unique_key(),
                    )
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // drop table `user`
        // Check: this deletes data. The inverse step does not restore the data.
        manager
            .drop_table(Table::drop().table(Alias::new("user")).to_owned())
            .await?;
        // drop table `post`
        // Check: this deletes data. The inverse step does not restore the data.
        manager
            .drop_table(Table::drop().table(Alias::new("post")).to_owned())
            .await?;
        Ok(())
    }
}
