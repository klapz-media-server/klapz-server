use sea_orm::entity::prelude::*;

/// The four corner colors of an item's backdrop, each `0xRRGGBB`.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "item_corner_colors")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub item_id: i64,
    pub top_left: i32,
    pub top_right: i32,
    pub bottom_right: i32,
    pub bottom_left: i32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::metadata_item::Entity",
        from = "Column::ItemId",
        to = "super::metadata_item::Column::Id",
        on_delete = "Cascade"
    )]
    MetadataItem,
}

impl ActiveModelBehavior for ActiveModel {}
