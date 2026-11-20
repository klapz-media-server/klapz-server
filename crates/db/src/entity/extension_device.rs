use sea_orm::entity::prelude::*;

/// A client a user signed in from through an extension, with its own token.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "extension_device")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub user_id: i64,
    pub extension: String,
    pub device_id: String,
    pub device_name: String,
    pub client: String,
    pub client_version: String,
    pub token_digest: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
