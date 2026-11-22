use sea_orm::entity::prelude::*;

/// One work one viewer made a favorite.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "user_favorite")]
pub struct Model {
    /// A user's id.
    #[sea_orm(primary_key, auto_increment = false)]
    pub viewer: i64,
    #[sea_orm(primary_key, auto_increment = false)]
    pub guid: String,
    pub added_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
