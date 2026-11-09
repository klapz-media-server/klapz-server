use sea_orm::entity::prelude::*;

/// A user's connection tied to an account on the extension's own service.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "extension_account")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub user_id: i64,
    pub extension: String,
    pub account_id: String,
    pub account_name: String,
    /// In clear, unlike a connection's digest: the server presents it to the service.
    pub account_token: String,
    pub connection_uri: String,
    pub published_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
