use sea_orm::entity::prelude::*;

/// One chapter of one item.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "item_chapter")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub item_id: i64,
    /// From 1. The reference's `index`.
    pub position: i32,
    pub start_ms: i64,
    pub end_ms: i64,
    pub title: Option<String>,
    /// The version whose frame the reference drew as the chapter's image.
    pub image_media_id: Option<i64>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
