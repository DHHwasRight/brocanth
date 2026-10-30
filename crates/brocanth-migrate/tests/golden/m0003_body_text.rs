// Written by `brocanth migrate make`. Read it before you commit it.
//
// It is a proposal: edit it when it is wrong. migrations/schema.json
// records the models, not this file.
//
// - alter column `post.body` (string null to text null)

use brocanth_model::sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m0003_body_text"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // alter column `post.body` (string null to text null)
        // Check: SQLite cannot alter a column. On Postgres, check the data for the new type.
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("post"))
                    .modify_column(ColumnDef::new(Alias::new("body")).text().null())
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // alter column `post.body` (text null to string null)
        // Check: SQLite cannot alter a column. On Postgres, check the data for the new type.
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("post"))
                    .modify_column(ColumnDef::new(Alias::new("body")).string().null())
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}
