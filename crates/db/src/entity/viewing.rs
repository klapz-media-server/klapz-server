use sea_orm::entity::prelude::*;

/// A playback under way, as far as its reports go.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "viewing")]
pub struct Model {
    /// A user's id.
    #[sea_orm(primary_key, auto_increment = false)]
    pub viewer: i64,
    #[sea_orm(primary_key, auto_increment = false)]
    pub playback: String,
    pub item_id: i64,
    pub position_ms: i64,
    pub completed: bool,
    pub expires_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
