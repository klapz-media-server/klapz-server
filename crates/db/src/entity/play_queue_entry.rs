use sea_orm::entity::prelude::*;

/// One position in a play queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "play_queue_entry")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub queue_id: i64,
    pub position: i64,
    pub item_id: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
