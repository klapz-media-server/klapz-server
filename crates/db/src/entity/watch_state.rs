use sea_orm::entity::prelude::*;

/// What one viewer has watched of one work.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "watch_state")]
pub struct Model {
    /// A user's id, or 0 for the operator.
    #[sea_orm(primary_key, auto_increment = false)]
    pub viewer: i64,
    #[sea_orm(primary_key, auto_increment = false)]
    pub guid: String,
    pub view_count: i32,
    pub offset_ms: Option<i64>,
    pub last_viewed_at: Option<i64>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
