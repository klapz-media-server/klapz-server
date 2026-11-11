use sea_orm::entity::prelude::*;

/// What a play queue was built from, in natural order: shuffling rebuilds it from these.
#[derive(Clone, Copy, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "play_queue_source")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub queue_id: i64,
    #[sea_orm(primary_key, auto_increment = false)]
    pub position: i64,
    pub item_id: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
