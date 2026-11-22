use sea_orm::entity::prelude::*;

/// The record an item shows.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "item_source")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub item_id: i64,
    pub source_metadata_id: i64,
    /// What the search scored the record at; `None` for a season or an episode, which follow
    /// their series.
    pub score: Option<i32>,
    pub matched_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
