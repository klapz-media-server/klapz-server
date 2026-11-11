use sea_orm::entity::prelude::*;

/// One viewing a viewer made.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "viewing_history")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    /// A user's id.
    pub viewer: i64,
    pub item_id: i64,
    pub kind: String,
    pub viewed_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
