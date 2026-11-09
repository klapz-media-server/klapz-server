//! An extension can be switched off without losing the identifier clients know it by.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(UserExtension::Table)
                    .add_column(
                        ColumnDef::new(UserExtension::Enabled)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(UserExtension::Table)
                    .drop_column(UserExtension::Enabled)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum UserExtension {
    Table,
    Enabled,
}
